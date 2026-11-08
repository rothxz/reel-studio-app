//! Where the agent's model runs: a model server on this computer (nothing
//! leaves the machine) or a cloud provider with the user's own API key.
//!
//! Each speaks one of `llm.rs`'s dialects: Ollama its own `/api/chat` (the
//! only way to raise its context window), Claude the Messages API, and
//! everyone else an OpenAI-compatible `/chat/completions` (Gemini's
//! included, through its `/v1beta/openai` endpoint).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dialect {
    Ollama,
    OpenAi,
    Anthropic,
}

/// A server kind the setup offers.
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    /// Stored in the settings (`"ollama"`, `"anthropic"`…).
    pub id: &'static str,
    /// What the user knows it as (brand names stay as they are).
    pub label: &'static str,
    pub dialect: Dialect,
    pub base_url: &'static str,
    /// A cloud provider: signed with the stored key, gets every tool.
    pub cloud: bool,
    /// Where the user gets a key (cloud), or the server (local).
    pub help_url: &'static str,
    /// What a key looks like (the field's placeholder).
    pub key_hint: &'static str,
    /// Model ids worth offering start with one of these (none: all).
    pub chat_prefixes: &'static [&'static str],
    /// …and contain none of these (embeddings, speech, pictures).
    pub skip: &'static [&'static str],
    /// Picked by default when the list has one, best first (prefixes).
    pub preferred: &'static [&'static str],
}

const NOT_CHAT: &[&str] = &[
    "embed",
    "audio",
    "realtime",
    "transcribe",
    "tts",
    "image",
    "search",
    "instruct-",
    "moderation",
    "whisper",
    "dall-e",
    "live",
    "robotics",
    "computer-use",
];

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "ollama",
        label: "Ollama",
        dialect: Dialect::Ollama,
        base_url: "http://localhost:11434",
        cloud: false,
        help_url: "https://ollama.com/download",
        key_hint: "",
        chat_prefixes: &[],
        skip: &["embed"],
        preferred: &[
            // Edits and sees in one model: `look` needs no second model
            // (one that evicts this one and its cached prompt).
            "qwen3-vl:8b-instruct",
            "qwen3-vl:4b-instruct",
            "qwen3:8b",
            "qwen3:4b-instruct",
            "qwen3",
            "qwen2.5:7b",
            "llama3.1",
        ],
    },
    Preset {
        id: "lmstudio",
        label: "LM Studio",
        dialect: Dialect::OpenAi,
        base_url: "http://localhost:1234/v1",
        cloud: false,
        help_url: "https://lmstudio.ai",
        key_hint: "",
        chat_prefixes: &[],
        skip: &["embed"],
        preferred: &["qwen3-vl", "qwen3", "qwen2.5"],
    },
    Preset {
        id: "llamacpp",
        label: "llama.cpp",
        dialect: Dialect::OpenAi,
        base_url: "http://localhost:8080/v1",
        cloud: false,
        help_url: "https://github.com/ggml-org/llama.cpp",
        key_hint: "",
        chat_prefixes: &[],
        skip: &[],
        preferred: &[],
    },
    Preset {
        id: "custom",
        label: "Custom (OpenAI-compatible)",
        dialect: Dialect::OpenAi,
        base_url: "http://localhost:8000/v1",
        cloud: false,
        help_url: "",
        key_hint: "",
        chat_prefixes: &[],
        skip: &[],
        preferred: &[],
    },
    Preset {
        id: "anthropic",
        label: "Claude (Anthropic)",
        dialect: Dialect::Anthropic,
        base_url: "https://api.anthropic.com/v1",
        cloud: true,
        help_url: "https://console.anthropic.com/settings/keys",
        key_hint: "sk-ant-…",
        chat_prefixes: &["claude-"],
        skip: &[],
        preferred: &[
            "claude-sonnet-5",
            "claude-opus-5",
            "claude-sonnet",
            "claude-haiku",
        ],
    },
    Preset {
        id: "openai",
        label: "ChatGPT (OpenAI)",
        dialect: Dialect::OpenAi,
        base_url: "https://api.openai.com/v1",
        cloud: true,
        help_url: "https://platform.openai.com/api-keys",
        key_hint: "sk-…",
        chat_prefixes: &["gpt-", "o1", "o3", "o4", "chatgpt-"],
        skip: NOT_CHAT,
        preferred: &["gpt-5.1", "gpt-5", "gpt-4.1", "gpt-4o"],
    },
    Preset {
        id: "gemini",
        label: "Gemini (Google)",
        dialect: Dialect::OpenAi,
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        cloud: true,
        help_url: "https://aistudio.google.com/apikey",
        key_hint: "AIza…",
        chat_prefixes: &["gemini-"],
        skip: NOT_CHAT,
        preferred: &[
            "gemini-3-flash",
            "gemini-2.5-flash",
            "gemini-3-pro",
            "gemini-2.5-pro",
        ],
    },
    Preset {
        id: "openrouter",
        label: "OpenRouter",
        dialect: Dialect::OpenAi,
        base_url: "https://openrouter.ai/api/v1",
        cloud: true,
        help_url: "https://openrouter.ai/keys",
        key_hint: "sk-or-…",
        chat_prefixes: &[],
        skip: &[],
        preferred: &["anthropic/claude-sonnet", "openai/gpt-5", "google/gemini"],
    },
    Preset {
        id: "xai",
        label: "Grok (xAI)",
        dialect: Dialect::OpenAi,
        base_url: "https://api.x.ai/v1",
        cloud: true,
        help_url: "https://console.x.ai",
        key_hint: "xai-…",
        chat_prefixes: &["grok-"],
        skip: &["image"],
        preferred: &["grok-4", "grok"],
    },
    Preset {
        id: "deepseek",
        label: "DeepSeek",
        dialect: Dialect::OpenAi,
        base_url: "https://api.deepseek.com/v1",
        cloud: true,
        help_url: "https://platform.deepseek.com/api_keys",
        key_hint: "sk-…",
        chat_prefixes: &[],
        skip: &[],
        preferred: &["deepseek-chat"],
    },
    Preset {
        id: "mistral",
        label: "Mistral",
        dialect: Dialect::OpenAi,
        base_url: "https://api.mistral.ai/v1",
        cloud: true,
        help_url: "https://console.mistral.ai/api-keys",
        key_hint: "…",
        chat_prefixes: &["mistral", "magistral", "codestral", "pixtral", "ministral"],
        skip: &["embed"],
        preferred: &["mistral-large-latest", "mistral-medium-latest"],
    },
];

pub fn preset(id: &str) -> &'static Preset {
    PRESETS.iter().find(|p| p.id == id).unwrap_or(&PRESETS[0])
}

impl Preset {
    /// The chat models of a `/models` list, best guesses first; within a
    /// rank the highest version first (gpt-5.1 before gpt-5).
    pub fn chat_models(&self, ids: Vec<String>) -> Vec<String> {
        let mut models: Vec<String> = ids
            .into_iter()
            // Gemini lists "models/gemini-…"; its chat endpoint wants the bare id.
            .map(|id| id.strip_prefix("models/").map(String::from).unwrap_or(id))
            .filter(|id| {
                let lower = id.to_lowercase();
                (self.chat_prefixes.is_empty()
                    || self.chat_prefixes.iter().any(|p| lower.starts_with(p)))
                    && !self.skip.iter().any(|s| lower.contains(s))
            })
            .collect();
        models.sort();
        models.dedup();
        let rank = |id: &str| {
            self.preferred
                .iter()
                .position(|p| id.starts_with(p))
                .unwrap_or(self.preferred.len())
        };
        models.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| b.cmp(a)));
        models
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_and_filters_models() {
        let openai = preset("openai");
        let got = openai.chat_models(
            [
                "gpt-4o",
                "gpt-5",
                "text-embedding-3",
                "gpt-5.1",
                "gpt-4o-audio-preview",
                "dall-e-3",
            ]
            .map(String::from)
            .to_vec(),
        );
        assert_eq!(got, ["gpt-5.1", "gpt-5", "gpt-4o"]);
        let gemini = preset("gemini");
        let got = gemini.chat_models(vec![
            "models/gemini-2.5-flash".into(),
            "models/embedding-001".into(),
        ]);
        assert_eq!(got, ["gemini-2.5-flash"]);
    }
}
