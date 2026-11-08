//! A chat-with-tools client for a model server on this computer or a cloud
//! provider (`providers.rs`). Three dialects:
//!
//! - **Ollama**'s own `/api/chat`: the only way to raise its context
//!   window (`num_ctx`; its default can't hold the tools + the project).
//! - **OpenAI-compatible** `/chat/completions`: LM Studio, llama.cpp's
//!   server, vLLM, Jan, a custom URL, and OpenAI, Gemini, OpenRouter, xAI,
//!   DeepSeek, Mistral in the cloud.
//! - **Anthropic**'s Messages API (Claude), with prompt caching.
//!
//! Every call blocks: run it off the UI thread. A key is read from the
//! keychain only here, for a request to that provider's own host.

use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use super::keys;
use super::providers::{Dialect, preset};

#[derive(Clone, Debug, PartialEq)]
pub struct Server {
    pub dialect: Dialect,
    /// "http://localhost:11434" (Ollama), "…/v1" (OpenAI-style), a provider's API.
    pub base_url: String,
    pub model: String,
    /// The cloud provider whose stored key signs the requests.
    pub auth: Option<String>,
}

impl Server {
    pub fn cloud(&self) -> bool {
        self.auth.is_some()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Parsed arguments; empty when the model sent none or broken JSON.
    pub arguments: Map<String, Value>,
    /// Provider data that must go back with the call untouched (Gemini's
    /// thought signature, OpenAI-style `extra_content`).
    pub extra: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    System(String),
    User(String),
    Assistant {
        content: String,
        calls: Vec<ToolCall>,
    },
    Tool {
        call_id: String,
        name: String,
        content: String,
        error: bool,
    },
}

/// A JSON-schema function tool, the shape every dialect takes.
#[derive(Clone, Debug)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Debug, Default)]
pub struct Reply {
    pub content: String,
    pub calls: Vec<ToolCall>,
    /// It stopped at the token limit, mid-answer.
    pub truncated: bool,
}

/// Ollama's context window for agent turns: tools + project + a few rounds.
const OLLAMA_NUM_CTX: u32 = 16_384;
/// A tool call or a short answer fits well under this on a local model; one
/// that runs on is rambling, and every token is the user's laptop's time.
/// The longest legitimate reply is a `make_video` script (8 scenes ≈ 1.5k
/// tokens).
const MAX_LOCAL_REPLY_TOKENS: u32 = 2560;
/// Cloud models plan several edits per reply, and reasoning models spend
/// part of this thinking first.
const MAX_CLOUD_REPLY_TOKENS: u32 = 8192;
const MAX_VISION_TOKENS: u32 = 600;
const MAX_CLOUD_VISION_TOKENS: u32 = 4096;
const OLLAMA_VISION_NUM_CTX: u32 = 8192;
/// Ollama frees the model's memory this long after the last request. A
/// reload costs seconds and drops the cached prompt (instructions + tools,
/// ~4.5k tokens: 10+ s to read again on a laptop), so the model stays while
/// the user is plausibly still editing.
const KEEP_ALIVE: &str = "30m";
/// One model call longer than this is stuck (or the machine is).
const CALL_TIMEOUT: Duration = Duration::from_secs(180);

fn http() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(10)))
                .timeout_global(Some(CALL_TIMEOUT))
                // Error bodies carry the reason ("model not found").
                .http_status_as_error(false)
                .build(),
        )
    })
}

fn trim_slash(url: &str) -> &str {
    url.trim_end_matches('/')
}

fn request(url: &str, body: Option<&Value>, auth: Option<&str>) -> Result<Value> {
    let mut headers: Vec<(&str, String)> = vec![];
    if let Some(provider) = auth {
        let key =
            keys::get(provider).ok_or_else(|| anyhow!("No API key saved for this provider"))?;
        if preset(provider).dialect == Dialect::Anthropic {
            headers.push(("x-api-key", key));
            headers.push(("anthropic-version", "2023-06-01".into()));
        } else {
            headers.push(("Authorization", format!("Bearer {key}")));
        }
    }
    let result = match body {
        Some(body) => {
            let mut req = http().post(url).header("Content-Type", "application/json");
            for (k, v) in &headers {
                req = req.header(*k, v);
            }
            req.send(body.to_string().as_bytes())
        }
        None => {
            let mut req = http().get(url);
            for (k, v) in &headers {
                req = req.header(*k, v);
            }
            req.call()
        }
    };
    let mut response = result.map_err(|e| anyhow!("{e}"))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .with_config()
        .limit(64 << 20)
        .read_to_string()?;
    let json: Value = if text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).map_err(|_| {
            anyhow!(
                "HTTP {status}: {}",
                text.chars().take(200).collect::<String>()
            )
        })?
    };
    if !(200..300).contains(&status) {
        // With REEL_AGENT_LOG, the refused request is kept to look at.
        if std::env::var_os("REEL_AGENT_LOG").is_some()
            && let Some(body) = body
        {
            let path = std::env::temp_dir().join("reel-agent-failed-request.json");
            let _ = std::fs::write(&path, body.to_string());
            eprintln!(
                "[agent] HTTP {status}: {} (request in {})",
                text.chars().take(300).collect::<String>(),
                path.display()
            );
        }
        let message = match json.get("error") {
            Some(Value::String(s)) => s.clone(),
            Some(e) => e
                .get("message")
                .and_then(Value::as_str)
                .map(String::from)
                .unwrap_or_else(|| e.to_string()),
            None => text.chars().take(200).collect(),
        };
        bail!("HTTP {status}: {message}");
    }
    if !json.is_object() {
        bail!("Empty reply from the server (HTTP {status})");
    }
    Ok(json)
}

/// The models the server has (or the provider offers), by id.
pub fn list_models(dialect: Dialect, base_url: &str, auth: Option<&str>) -> Result<Vec<String>> {
    let base = trim_slash(base_url);
    let json = match dialect {
        Dialect::Ollama => request(&format!("{base}/api/tags"), None, None)?,
        Dialect::Anthropic => request(&format!("{base}/models?limit=100"), None, auth)?,
        Dialect::OpenAi => request(&format!("{base}/models"), None, auth)?,
    };
    let list = match dialect {
        Dialect::Ollama => json.get("models"),
        _ => json.get("data"),
    };
    Ok(list
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            m.get("name")
                .or_else(|| m.get("id"))
                .and_then(Value::as_str)
        })
        .map(String::from)
        .collect())
}

// ---- reading replies -----------------------------------------------------------

fn parse_arguments(raw: Option<&Value>) -> Map<String, Value> {
    match raw {
        Some(Value::Object(m)) => m.clone(),
        Some(Value::String(s)) => parse_json_object(s).unwrap_or_default(),
        _ => Map::new(),
    }
}

/// A JSON object from a model's text, forgiving the usual slips (a code
/// fence around it, trailing commas).
pub fn parse_json_object(text: &str) -> Option<Map<String, Value>> {
    let t = text.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .map(|s| s.trim_end().trim_end_matches("```"))
        .unwrap_or(t)
        .trim();
    if let Ok(Value::Object(m)) = serde_json::from_str(t) {
        return Some(m);
    }
    // Trailing commas before } or ].
    let mut fixed = String::with_capacity(t.len());
    let chars: Vec<char> = t.chars().collect();
    let mut in_str = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' && (i == 0 || chars[i - 1] != '\\') {
            in_str = !in_str;
        }
        if !in_str && c == ',' {
            let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                i += 1;
                continue;
            }
        }
        fixed.push(c);
        i += 1;
    }
    if let Ok(Value::Object(m)) = serde_json::from_str(&fixed) {
        return Some(m);
    }
    // Brackets (or a string) left open: small models drop the last `}`.
    let mut open = vec![];
    let mut in_str = false;
    let mut escaped = false;
    for c in fixed.chars() {
        if in_str {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => open.push('}'),
            '[' => open.push(']'),
            '}' | ']' if open.last() == Some(&c) => {
                open.pop();
            }
            '}' | ']' => return None,
            _ => {}
        }
    }
    if open.is_empty() && !in_str {
        return None;
    }
    if in_str {
        fixed.push('"');
    }
    fixed.extend(open.iter().rev());
    match serde_json::from_str(&fixed) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    }
}

static CALL_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn call_id() -> String {
    format!(
        "call_{}",
        CALL_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// A `{"name": …, "arguments": …}` object as a call.
fn call_from_object(obj: &Map<String, Value>) -> Option<ToolCall> {
    let name = obj.get("name").and_then(Value::as_str)?;
    let args = obj.get("arguments").or_else(|| obj.get("parameters"));
    Some(ToolCall {
        id: call_id(),
        name: name.to_string(),
        arguments: parse_arguments(args),
        extra: None,
    })
}

/// Small models sometimes write the call as text instead of using the tool
/// API: `<tool_call>{…}</tool_call>` (Qwen / Hermes style), a fenced JSON
/// block, or the whole answer one JSON object. Pulls those out so the turn
/// still works.
pub fn extract_text_tool_calls(content: &str) -> (String, Vec<ToolCall>) {
    let mut calls = vec![];
    let mut rest = String::new();
    let mut s = content;
    while let Some(start) = s.find("<tool_call>") {
        rest.push_str(&s[..start]);
        let after = &s[start + "<tool_call>".len()..];
        let (body, tail) = match after.find("</tool_call>") {
            Some(end) => (&after[..end], &after[end + "</tool_call>".len()..]),
            None => (after, ""),
        };
        match parse_json_object(body).as_ref().and_then(call_from_object) {
            Some(call) => calls.push(call),
            None => rest.push_str(body),
        }
        s = tail;
    }
    rest.push_str(s);
    if calls.is_empty() {
        let trimmed = rest.trim();
        // The whole answer is a call (or a list of them).
        if (trimmed.starts_with("```") || trimmed.starts_with('{'))
            && let Some(call) = parse_json_object(trimmed)
                .as_ref()
                .and_then(call_from_object)
        {
            return (String::new(), vec![call]);
        }
        if trimmed.starts_with('[')
            && let Ok(Value::Array(items)) = serde_json::from_str::<Value>(trimmed)
        {
            let found: Vec<ToolCall> = items
                .iter()
                .filter_map(Value::as_object)
                .filter_map(call_from_object)
                .collect();
            if !found.is_empty() && found.len() == items.len() {
                return (String::new(), found);
            }
        }
    }
    (rest.trim().to_string(), calls)
}

/// Reasoning models' hidden thoughts, when the server leaves them in the
/// text: a `<think>…</think>` block, or everything up to a lone `</think>`
/// (templates that open the block themselves).
pub fn strip_thinking(content: &str) -> String {
    let mut s = content.to_string();
    if let Some(closed) = s.rfind("</think>")
        && !s[..closed].contains("<think>")
    {
        s = s[closed + "</think>".len()..].to_string();
    }
    while let Some(start) = s.find("<think>") {
        match s[start..].find("</think>") {
            Some(end) => s.replace_range(start..start + end + "</think>".len(), ""),
            None => s.truncate(start),
        }
    }
    s.trim().to_string()
}

fn finish(content: &str, calls: Vec<ToolCall>, truncated: bool) -> Reply {
    let visible = strip_thinking(content);
    if !calls.is_empty() {
        return Reply {
            content: visible,
            calls,
            truncated,
        };
    }
    let (content, calls) = extract_text_tool_calls(&visible);
    Reply {
        content,
        calls,
        truncated,
    }
}

// ---- message shapes ------------------------------------------------------------

fn to_ollama(m: &Message) -> Value {
    match m {
        Message::System(s) => json!({ "role": "system", "content": s }),
        Message::User(s) => json!({ "role": "user", "content": s }),
        Message::Assistant { content, calls } => {
            let mut v = json!({ "role": "assistant", "content": content });
            if !calls.is_empty() {
                v["tool_calls"] = calls
                    .iter()
                    .map(|c| json!({ "function": { "name": c.name, "arguments": c.arguments } }))
                    .collect();
            }
            v
        }
        Message::Tool { name, content, .. } => {
            json!({ "role": "tool", "content": content, "tool_name": name })
        }
    }
}

fn to_openai(m: &Message) -> Value {
    match m {
        Message::System(s) => json!({ "role": "system", "content": s }),
        Message::User(s) => json!({ "role": "user", "content": s }),
        Message::Assistant { content, calls } => {
            let mut v = json!({ "role": "assistant", "content": content });
            if !calls.is_empty() {
                v["tool_calls"] = calls
                    .iter()
                    .map(|c| {
                        let mut call = json!({
                            "id": c.id,
                            "type": "function",
                            "function": {
                                "name": c.name,
                                "arguments": Value::Object(c.arguments.clone()).to_string(),
                            },
                        });
                        if let Some(extra) = &c.extra {
                            call["extra_content"] = extra.clone();
                        }
                        call
                    })
                    .collect();
            }
            v
        }
        Message::Tool {
            call_id, content, ..
        } => json!({ "role": "tool", "tool_call_id": call_id, "content": content }),
    }
}

/// Our messages in Claude's shape: the system text apart, tool calls as
/// `tool_use` blocks, results as `tool_result` blocks in a user turn, and
/// runs of one role merged.
fn to_anthropic(messages: &[Message]) -> (String, Vec<Value>) {
    let mut system = vec![];
    let mut out: Vec<(String, Vec<Value>)> = vec![];
    let mut add = |role: &str, blocks: Vec<Value>| {
        if blocks.is_empty() {
            return;
        }
        match out.last_mut() {
            Some((r, b)) if r == role => b.extend(blocks),
            _ => out.push((role.to_string(), blocks)),
        }
    };
    for m in messages {
        match m {
            Message::System(s) => system.push(s.clone()),
            Message::User(s) if !s.is_empty() => {
                add("user", vec![json!({ "type": "text", "text": s })])
            }
            Message::User(_) => {}
            Message::Assistant { content, calls } => {
                let mut blocks = vec![];
                if !content.trim().is_empty() {
                    blocks.push(json!({ "type": "text", "text": content }));
                }
                for c in calls {
                    blocks.push(json!({ "type": "tool_use", "id": c.id, "name": c.name, "input": c.arguments }));
                }
                add("assistant", blocks);
            }
            Message::Tool {
                call_id,
                content,
                error,
                ..
            } => {
                let mut block =
                    json!({ "type": "tool_result", "tool_use_id": call_id, "content": content });
                if *error {
                    block["is_error"] = Value::Bool(true);
                }
                add("user", vec![block]);
            }
        }
    }
    // Cache breakpoint on the newest block: each round re-reads only what's new.
    if let Some((_, blocks)) = out.last_mut()
        && let Some(last) = blocks.last_mut()
    {
        last["cache_control"] = json!({ "type": "ephemeral" });
    }
    (
        system.join("\n\n"),
        out.into_iter()
            .map(|(role, content)| json!({ "role": role, "content": content }))
            .collect(),
    )
}

/// The OpenAI-style body's dialect quirks: local servers get Qwen-style
/// no-thinking (OpenAI refuses unknown fields); OpenAI's reasoning models
/// refuse `temperature` and `max_tokens`.
fn openai_options(server: &Server, max_tokens: u32) -> Map<String, Value> {
    let mut o = Map::new();
    match server.auth.as_deref() {
        None => {
            o.insert("temperature".into(), json!(0.2));
            o.insert("max_tokens".into(), json!(max_tokens));
            o.insert(
                "chat_template_kwargs".into(),
                json!({ "enable_thinking": false }),
            );
        }
        Some("openai") => {
            o.insert("max_completion_tokens".into(), json!(max_tokens));
        }
        Some(_) => {
            o.insert("temperature".into(), json!(0.2));
            o.insert("max_tokens".into(), json!(max_tokens));
        }
    }
    o
}

/// Ollama's /api/chat, asked not to think unless the model refuses the flag.
fn ollama_chat(base: &str, mut body: Value) -> Result<Value> {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static THINKERS: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let model = body["model"].as_str().unwrap_or("").to_string();
    let url = format!("{base}/api/chat");
    let known = THINKERS
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|s| s.contains(&model)))
        .unwrap_or(false);
    if known {
        return request(&url, Some(&body), None);
    }
    body["think"] = Value::Bool(false);
    match request(&url, Some(&body), None) {
        Err(e) if e.to_string().to_lowercase().contains("think") => {
            if let Ok(mut g) = THINKERS.lock() {
                g.get_or_insert_with(HashSet::new).insert(model);
            }
            if let Some(o) = body.as_object_mut() {
                o.remove("think");
            }
            request(&url, Some(&body), None)
        }
        r => r,
    }
}

// ---- chat ---------------------------------------------------------------------

/// Tries after Ollama refuses a reply over a malformed tool call.
const BAD_CALL_RETRIES: u32 = 2;
const BAD_CALL_NOTE: &str = "Your last tool call was not valid JSON and was lost. \
    Make the call again: one JSON object per <tool_call>, every quote, ] and } closed.";

/// Ollama's error for a tool call it could not read: Go's JSON errors
/// ("unexpected end of JSON input", "invalid character '}' …") as an HTTP 500.
fn is_bad_tool_call(e: &anyhow::Error) -> bool {
    let msg = e.to_string();
    msg.starts_with("HTTP 500")
        && [
            "unexpected end of JSON input",
            "invalid character",
            "cannot unmarshal",
            "tool call",
        ]
        .iter()
        .any(|m| msg.contains(m))
}

/// The agent's Ollama options. Warm-up and chat must agree on `num_ctx`:
/// another size reloads the model.
fn ollama_options(num_predict: u32) -> Value {
    json!({
        "num_ctx": OLLAMA_NUM_CTX,
        "num_predict": num_predict,
        "temperature": 0.2,
    })
}

/// Loads a local model and has it read the fixed start of every request
/// (the instructions and the tools), so the first real request skips the
/// load and that long read: the server keeps the prompt's start cached and
/// reuses it. A one-token answer; cloud providers are left alone (it would
/// cost the user money).
pub fn warm_up(server: &Server, system: &str, tools: &[ToolDef]) -> Result<()> {
    if server.cloud() {
        return Ok(());
    }
    let base = trim_slash(&server.base_url);
    // Some chat templates refuse a conversation without a user turn.
    let messages = json!([
        { "role": "system", "content": system },
        { "role": "user", "content": "Hi" },
    ]);
    match server.dialect {
        Dialect::Ollama => {
            let body = json!({
                "model": server.model,
                "messages": messages,
                "tools": tool_defs(tools),
                "stream": false,
                "keep_alive": KEEP_ALIVE,
                "options": ollama_options(1),
            });
            let json = ollama_chat(base, body)?;
            log_ollama_timing(&json);
        }
        Dialect::OpenAi => {
            let mut body = json!({
                "model": server.model,
                "messages": messages,
                "tools": tool_defs(tools),
                "stream": false,
            });
            body.as_object_mut()
                .unwrap()
                .extend(openai_options(server, 1));
            request(&format!("{base}/chat/completions"), Some(&body), None)?;
        }
        Dialect::Anthropic => {}
    }
    Ok(())
}

/// One model reply (no streaming): its text and the tools it wants run.
pub fn chat(server: &Server, messages: &[Message], tools: &[ToolDef]) -> Result<Reply> {
    let base = trim_slash(&server.base_url);
    let auth = server.auth.as_deref();
    match server.dialect {
        Dialect::Anthropic => {
            let (system, msgs) = to_anthropic(messages);
            let body = json!({
                "model": server.model,
                "max_tokens": MAX_CLOUD_REPLY_TOKENS,
                "system": [{ "type": "text", "text": system, "cache_control": { "type": "ephemeral" } }],
                "messages": msgs,
                "tools": tools.iter().map(|t| json!({
                    "name": t.name, "description": t.description, "input_schema": t.parameters,
                })).collect::<Vec<_>>(),
            });
            let json = request(&format!("{base}/messages"), Some(&body), auth)?;
            Ok(from_anthropic(&json))
        }
        Dialect::Ollama => {
            let mut body = json!({
                "model": server.model,
                "messages": messages.iter().map(to_ollama).collect::<Vec<_>>(),
                "tools": tool_defs(tools),
                "stream": false,
                "keep_alive": KEEP_ALIVE,
                "options": ollama_options(MAX_LOCAL_REPLY_TOKENS),
            });
            // Ollama reads the model's tool calls itself and answers a
            // broken one (an unclosed brace…) with a bare HTTP 500, the
            // reply thrown away. Ask again, warmer, with a note: the same
            // request at 0.2 would write the same broken call.
            let mut attempt = 0;
            let json = loop {
                match ollama_chat(base, body.clone()) {
                    Err(e) if attempt < BAD_CALL_RETRIES && is_bad_tool_call(&e) => {
                        attempt += 1;
                        if std::env::var_os("REEL_AGENT_LOG").is_some() {
                            eprintln!("[agent] broken tool call ({e:#}), asking again");
                        }
                        if attempt == 1
                            && let Some(msgs) = body["messages"].as_array_mut()
                        {
                            msgs.push(json!({ "role": "user", "content": BAD_CALL_NOTE }));
                        }
                        body["options"]["temperature"] = json!(0.2 + 0.3 * attempt as f64);
                    }
                    r => break r?,
                }
            };
            log_ollama_timing(&json);
            let msg = &json["message"];
            let calls = msg["tool_calls"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| {
                    let f = c.get("function")?;
                    Some(ToolCall {
                        id: call_id(),
                        name: f.get("name")?.as_str()?.to_string(),
                        arguments: parse_arguments(f.get("arguments")),
                        extra: None,
                    })
                })
                .collect();
            Ok(finish(
                msg["content"].as_str().unwrap_or(""),
                calls,
                json["done_reason"].as_str() == Some("length"),
            ))
        }
        Dialect::OpenAi => {
            let max = if server.cloud() {
                MAX_CLOUD_REPLY_TOKENS
            } else {
                MAX_LOCAL_REPLY_TOKENS
            };
            let mut body = json!({
                "model": server.model,
                "messages": messages.iter().map(to_openai).collect::<Vec<_>>(),
                "tools": tool_defs(tools),
                "tool_choice": "auto",
                "stream": false,
            });
            body.as_object_mut()
                .unwrap()
                .extend(openai_options(server, max));
            let json = request(&format!("{base}/chat/completions"), Some(&body), auth)?;
            let choice = &json["choices"][0];
            let msg = &choice["message"];
            let calls = msg["tool_calls"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| {
                    let f = c.get("function")?;
                    Some(ToolCall {
                        id: c
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(String::from)
                            .unwrap_or_else(call_id),
                        name: f.get("name")?.as_str()?.to_string(),
                        arguments: parse_arguments(f.get("arguments")),
                        extra: c.get("extra_content").cloned(),
                    })
                })
                .collect();
            Ok(finish(
                msg["content"].as_str().unwrap_or(""),
                calls,
                choice["finish_reason"].as_str() == Some("length"),
            ))
        }
    }
}

/// Where a local call's time went, on stderr when `REEL_AGENT_LOG` is set:
/// loading the model, reading the prompt, writing the answer.
fn log_ollama_timing(json: &Value) {
    if std::env::var_os("REEL_AGENT_LOG").is_none() {
        return;
    }
    let ms = |k: &str| json[k].as_u64().unwrap_or(0) / 1_000_000;
    let n = |k: &str| json[k].as_u64().unwrap_or(0);
    eprintln!(
        "[agent] load {} ms · prompt {} tok in {} ms · reply {} tok in {} ms · total {} ms",
        ms("load_duration"),
        n("prompt_eval_count"),
        ms("prompt_eval_duration"),
        n("eval_count"),
        ms("eval_duration"),
        ms("total_duration"),
    );
}

fn tool_defs(tools: &[ToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": { "name": t.name, "description": t.description, "parameters": t.parameters },
            })
        })
        .collect()
}

fn from_anthropic(json: &Value) -> Reply {
    let mut text = String::new();
    let mut calls = vec![];
    for block in json["content"].as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("text") => text.push_str(block["text"].as_str().unwrap_or("")),
            Some("tool_use") => calls.push(ToolCall {
                id: block["id"].as_str().unwrap_or("").to_string(),
                name: block["name"].as_str().unwrap_or("").to_string(),
                arguments: parse_arguments(block.get("input")),
                extra: None,
            }),
            _ => {}
        }
    }
    finish(
        &text,
        calls,
        json["stop_reason"].as_str() == Some("max_tokens"),
    )
}

// ---- vision -------------------------------------------------------------------

/// Asks a vision model about a picture (JPEG bytes) and returns its answer.
/// The chat model never gets the image, only this text, so a text-only chat
/// model can still "see" through a separate vision model.
pub fn ask_about_image(server: &Server, prompt: &str, jpeg: &[u8]) -> Result<String> {
    let base = trim_slash(&server.base_url);
    let b64 = base64(jpeg);
    let auth = server.auth.as_deref();
    match server.dialect {
        Dialect::Anthropic => {
            let body = json!({
                "model": server.model,
                "max_tokens": MAX_CLOUD_VISION_TOKENS,
                "messages": [{ "role": "user", "content": [
                    { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": b64 } },
                    { "type": "text", "text": prompt },
                ]}],
            });
            let json = request(&format!("{base}/messages"), Some(&body), auth)?;
            Ok(from_anthropic(&json).content)
        }
        Dialect::Ollama => {
            let body = json!({
                "model": server.model,
                "messages": [{ "role": "user", "content": prompt, "images": [b64] }],
                "stream": false,
                "keep_alive": KEEP_ALIVE,
                "options": {
                    "num_ctx": OLLAMA_VISION_NUM_CTX,
                    "num_predict": MAX_VISION_TOKENS,
                    "temperature": 0.2,
                },
            });
            let json = ollama_chat(base, body)?;
            Ok(strip_thinking(
                json["message"]["content"].as_str().unwrap_or(""),
            ))
        }
        Dialect::OpenAi => {
            let max = if server.cloud() {
                MAX_CLOUD_VISION_TOKENS
            } else {
                MAX_VISION_TOKENS
            };
            let mut body = json!({
                "model": server.model,
                "messages": [{ "role": "user", "content": [
                    { "type": "text", "text": prompt },
                    { "type": "image_url", "image_url": { "url": format!("data:image/jpeg;base64,{b64}") } },
                ]}],
                "stream": false,
            });
            body.as_object_mut()
                .unwrap()
                .extend(openai_options(server, max));
            let json = request(&format!("{base}/chat/completions"), Some(&body), auth)?;
            Ok(strip_thinking(
                json["choices"][0]["message"]["content"]
                    .as_str()
                    .unwrap_or(""),
            ))
        }
    }
}

/// Standard base64 (with padding).
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulls_text_tool_calls() {
        let (rest, calls) = extract_text_tool_calls(
            "Sure.<tool_call>{\"name\": \"seek\", \"arguments\": {\"time\": 3}}</tool_call>",
        );
        assert_eq!(rest, "Sure.");
        assert_eq!(calls[0].name, "seek");
        assert_eq!(calls[0].arguments["time"], 3);

        let (_, calls) = extract_text_tool_calls(
            "```json\n{\"name\":\"split_clip\",\"arguments\":{\"clip\":\"c1\",}}\n```",
        );
        assert_eq!(calls[0].name, "split_clip");
        assert_eq!(calls[0].arguments["clip"], "c1");

        // The closing brace dropped (what Ollama answers with HTTP 500).
        let (_, calls) = extract_text_tool_calls(
            "<tool_call>\n{\"name\": \"seek\", \"arguments\": {\"time\": 5}\n</tool_call>",
        );
        assert_eq!(calls[0].arguments["time"], 5);
        let (_, calls) = extract_text_tool_calls(
            "<tool_call>{\"name\": \"add_text\", \"arguments\": {\"text\": \"a } b",
        );
        assert_eq!(calls[0].arguments["text"], "a } b");
        assert!(parse_json_object("{\"a\": 1}}").is_none());
        let (rest, calls) = extract_text_tool_calls("Done, I added it.");
        assert!(calls.is_empty());
        assert_eq!(rest, "Done, I added it.");
    }

    #[test]
    fn strips_thinking() {
        assert_eq!(strip_thinking("<think>hmm</think> Hi"), "Hi");
        assert_eq!(strip_thinking("planning…</think>Answer"), "Answer");
        assert_eq!(strip_thinking("<think>cut off"), "");
    }

    #[test]
    fn claude_messages_merge_tool_results() {
        let call = ToolCall {
            id: "a".into(),
            name: "seek".into(),
            arguments: Map::new(),
            extra: None,
        };
        let (system, msgs) = to_anthropic(&[
            Message::System("rules".into()),
            Message::User("go".into()),
            Message::Assistant {
                content: String::new(),
                calls: vec![
                    call.clone(),
                    ToolCall {
                        id: "b".into(),
                        ..call
                    },
                ],
            },
            Message::Tool {
                call_id: "a".into(),
                name: "seek".into(),
                content: "{}".into(),
                error: false,
            },
            Message::Tool {
                call_id: "b".into(),
                name: "seek".into(),
                content: "{\"error\":1}".into(),
                error: true,
            },
        ]);
        assert_eq!(system, "rules");
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[2]["content"].as_array().unwrap().len(), 2);
        assert_eq!(msgs[2]["content"][1]["is_error"], true);
        assert!(msgs[2]["content"][1].get("cache_control").is_some());
    }

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }
}
