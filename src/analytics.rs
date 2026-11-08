//! Product analytics: anonymous usage events to PostHog, the same project
//! and event names as the phone app so both land on the same dashboards
//! (told apart by `platform`).
//!
//! The rest of the app only calls [`track`] (and [`EditorSession`] for the
//! events derived from edits). Nothing here can block or fail an edit:
//! events go over a channel to one background thread, which keeps a
//! bounded queue (200), sends batches (≤ 50) every 30 s or once 20 are
//! waiting, retries a failed batch twice, then drops it.
//!
//! Sent only when all of these hold:
//! - a project key was compiled in (`REEL_POSTHOG_KEY`, from the app
//!   checkout's `.env` or the release workflow; none = analytics off);
//! - it's a release build (`REEL_RELEASE_BUILD`, set when the release
//!   workflow stamps a version), or `REEL_ANALYTICS_IN_DEV=1` at run time;
//! - `REEL_ANALYTICS` isn't `0` at run time;
//! - Settings → "Share anonymous usage data" is on (the default).
//!
//! Local builds print `[analytics] <event> {props}` to stderr instead
//! (`REEL_ANALYTICS_LOG=0` silences it). What leaves the machine: a random
//! id kept in the data folder, the app version, OS and CPU, and the event's
//! own counts and catalog names. Never paths, URLs, project names, typed
//! text or keys: [`sanitize`] drops those as a backstop. GeoIP is off and
//! no person profile is made.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::catalog::{EFFECT_PREFIX, FILTER_PREFIX};
use crate::model::{Project, TrackType};

const KEY: &str = env!("REEL_POSTHOG_KEY");
const HOST: &str = env!("REEL_POSTHOG_HOST");
const RELEASE_BUILD: &str = env!("REEL_RELEASE_BUILD");
const DEFAULT_HOST: &str = "https://us.i.posthog.com";

const MAX_QUEUE: usize = 200;
const MAX_BATCH: usize = 50;
const FLUSH_AT: usize = 20;
const FLUSH_EVERY: Duration = Duration::from_secs(30);
const RETRIES: u32 = 2;
/// A timeline gesture (move, trim, split) is reported at most this often.
const GESTURE_EVERY: Duration = Duration::from_secs(5);

enum Msg {
    Event(Value),
    Flush(Sender<()>),
    Clear,
}

struct Client {
    tx: Sender<Msg>,
    /// Events go to PostHog (else only the log, if on).
    send: bool,
    log: bool,
}

static CLIENT: OnceLock<Client> = OnceLock::new();
/// The Settings switch.
static ENABLED: AtomicBool = AtomicBool::new(true);

fn env_flag(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_lowercase())
}

/// Starts the sender (once, at launch) with the user's choice.
pub fn init(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    let release = !RELEASE_BUILD.is_empty();
    let send = !KEY.is_empty()
        && env_flag("REEL_ANALYTICS").as_deref() != Some("0")
        && (release || env_flag("REEL_ANALYTICS_IN_DEV").as_deref() == Some("1"));
    let log = !release && env_flag("REEL_ANALYTICS_LOG").as_deref() != Some("0");
    let (tx, rx) = mpsc::channel();
    if send {
        let spawned = std::thread::Builder::new()
            .name("analytics".into())
            .spawn(move || worker(rx));
        if spawned.is_err() {
            return;
        }
    }
    let _ = CLIENT.set(Client { tx, send, log });
}

/// The Settings switch: off drops whatever is still waiting.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    if !on && let Some(c) = CLIENT.get() {
        let _ = c.tx.send(Msg::Clear);
    }
}

/// Records `event` with `props` (a JSON object of counts and catalog
/// names). Returns at once; never fails.
pub fn track(event: &'static str, props: Value) {
    let Some(client) = CLIENT.get() else { return };
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let props = sanitize(props);
    if client.log {
        eprintln!("[analytics] {event} {}", Value::Object(props.clone()));
    }
    if client.send {
        let _ = client.tx.send(Msg::Event(json!({
            "event": event,
            "properties": props,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        })));
    }
}

/// Sends what's waiting, waiting at most `timeout` (on quit).
pub fn flush(timeout: Duration) {
    let Some(client) = CLIENT.get().filter(|c| c.send) else {
        return;
    };
    let (ack, done) = mpsc::channel();
    if client.tx.send(Msg::Flush(ack)).is_ok() {
        let _ = done.recv_timeout(timeout);
    }
}

// ---- privacy ------------------------------------------------------------

const MAX_STRING: usize = 64;
const BLOCKED_KEYS: [&str; 14] = [
    "path",
    "uri",
    "url",
    "token",
    "secret",
    "password",
    "email",
    "phone",
    "caption",
    "content",
    "json",
    "thumbnail",
    "apikey",
    "api_key",
];

/// Looks like a path or URL: `/x`, `~/x`, `./x`, `C:\x`, `scheme://`.
fn path_or_url(s: &str) -> bool {
    s.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '('))
        .any(|w| {
            let b = w.as_bytes();
            w.starts_with('/')
                || w.starts_with("~/")
                || w.starts_with("./")
                || w.starts_with("../")
                || w.contains("://")
                || (b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\')
        })
}

fn email(s: &str) -> bool {
    s.split_whitespace().any(|w| {
        w.split_once('@')
            .is_some_and(|(a, b)| !a.is_empty() && b.contains('.') && !b.ends_with('.'))
    })
}

/// Keeps only flat numbers, booleans and short plain strings, under keys
/// that don't name content: the last line of defence, whatever a call
/// site passes.
pub fn sanitize(props: Value) -> Map<String, Value> {
    let Value::Object(map) = props else {
        return Map::new();
    };
    map.into_iter()
        .filter(|(k, _)| {
            let k = k.to_lowercase();
            !BLOCKED_KEYS.iter().any(|b| k.contains(b))
        })
        .filter_map(|(k, v)| {
            let keep = match &v {
                Value::Bool(_) => true,
                Value::Number(n) => n.as_f64().is_some_and(f64::is_finite),
                Value::String(s) => s.len() <= MAX_STRING && !path_or_url(s) && !email(s),
                _ => false,
            };
            keep.then_some((k, v))
        })
        .collect()
}

// ---- the sender -----------------------------------------------------------

/// Properties on every event.
fn super_props(distinct_id: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("distinct_id".into(), distinct_id.into());
    // "macos" | "windows" | "linux" (the phone app sends "ios" / "android").
    m.insert("platform".into(), std::env::consts::OS.into());
    if let Some(v) = os_version() {
        m.insert("os_version".into(), v.into());
    }
    m.insert("app_version".into(), crate::updater::VERSION.into());
    m.insert("device_class".into(), "desktop".into());
    m.insert("architecture".into(), std::env::consts::ARCH.into());
    m.insert("$lib".into(), "reel-studio-desktop".into());
    m.insert("$lib_version".into(), crate::updater::VERSION.into());
    m.insert("$geoip_disable".into(), true.into());
    m.insert("$process_person_profile".into(), false.into());
    m
}

fn os_version() -> Option<String> {
    let out = |cmd: &str, args: &[&str]| {
        let o = std::process::Command::new(cmd).args(args).output().ok()?;
        let s = String::from_utf8(o.stdout).ok()?.trim().to_string();
        (!s.is_empty()).then_some(s)
    };
    if cfg!(target_os = "macos") {
        out("sw_vers", &["-productVersion"])
    } else if cfg!(target_os = "linux") {
        let text = std::fs::read_to_string("/etc/os-release").ok()?;
        let field = |key: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(key))
                .map(|v| v.trim_matches('"').to_string())
        };
        Some(format!(
            "{} {}",
            field("ID=")?,
            field("VERSION_ID=").unwrap_or_default()
        ))
        .map(|s| s.trim().to_string())
    } else if cfg!(target_os = "windows") {
        // "Microsoft Windows [Version 10.0.22631.4317]" → the number.
        let v = out("cmd", &["/C", "ver"])?;
        let start = v.find("Version ")? + "Version ".len();
        Some(v[start..].trim_end_matches(']').to_string())
    } else {
        None
    }
}

/// A random id for this install, kept in the data folder. Not tied to the
/// machine or the user: deleting the file starts a new one.
fn install_id() -> String {
    let file = crate::prefs::data_dir().join("analytics-id");
    if let Ok(id) = std::fs::read_to_string(&file) {
        let id = id.trim();
        if id.len() == 36 {
            return id.to_string();
        }
    }
    let id = random_uuid();
    let _ = std::fs::write(&file, &id);
    id
}

/// A version-4 UUID from the OS-seeded hasher keys (no rand crate).
fn random_uuid() -> String {
    use std::hash::{BuildHasher, Hasher};
    let word = || {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.finish()
    };
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&word().to_le_bytes());
    b[8..].copy_from_slice(&word().to_le_bytes());
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn worker(rx: Receiver<Msg>) {
    let id = install_id();
    let common = super_props(&id);
    let host = if HOST.trim().is_empty() {
        DEFAULT_HOST
    } else {
        HOST.trim()
    };
    let url = format!("{}/batch/", host.trim_end_matches('/'));
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_global(Some(Duration::from_secs(10)))
            .build(),
    );
    let mut queue: Vec<Value> = Vec::new();
    let mut failures = 0;
    let mut next = Instant::now() + FLUSH_EVERY;
    let send = |queue: &mut Vec<Value>, failures: &mut u32| {
        while !queue.is_empty() {
            let n = queue.len().min(MAX_BATCH);
            let body = json!({ "api_key": KEY, "batch": &queue[..n] });
            let ok = agent
                .post(&url)
                .header("Content-Type", "application/json")
                .send(body.to_string().as_bytes())
                .is_ok();
            if ok || *failures >= RETRIES {
                queue.drain(..n);
                *failures = 0;
            } else {
                *failures += 1;
                return; // offline: try again on the next tick
            }
        }
    };
    loop {
        let wait = next.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(Msg::Event(mut e)) => {
                if let Some(p) = e.get_mut("properties").and_then(Value::as_object_mut) {
                    for (k, v) in &common {
                        p.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
                if queue.len() >= MAX_QUEUE {
                    queue.remove(0);
                }
                queue.push(e);
                if queue.len() >= FLUSH_AT {
                    send(&mut queue, &mut failures);
                }
            }
            Ok(Msg::Flush(ack)) => {
                send(&mut queue, &mut failures);
                let _ = ack.send(());
            }
            Ok(Msg::Clear) => queue.clear(),
            Err(RecvTimeoutError::Timeout) => {
                send(&mut queue, &mut failures);
                next = Instant::now() + FLUSH_EVERY;
            }
            Err(RecvTimeoutError::Disconnected) => {
                send(&mut queue, &mut failures);
                return;
            }
        }
    }
}

// ---- events derived from edits --------------------------------------------

/// A clip as the edit diff sees it.
struct ClipInfo {
    kind: String,
    lane: TrackType,
    /// (instance id, type) of each effect.
    effects: Vec<(String, String)>,
    transition: Option<String>,
    font: Option<String>,
    /// From the online library (an `http` source or one downloaded into
    /// `<data>/library/`), else a local file.
    from_library: bool,
}

fn index(project: &Project) -> HashMap<String, ClipInfo> {
    let mut map = HashMap::new();
    for track in &project.tracks {
        for c in &track.clips {
            let source = c.source.as_deref().unwrap_or("");
            map.insert(
                c.id.clone(),
                ClipInfo {
                    kind: c.kind.clone(),
                    lane: track.kind,
                    effects: c.effects().into_iter().map(|e| (e.id, e.kind)).collect(),
                    transition: c
                        .extra
                        .get("outTransition")
                        .and_then(|t| t.get("type"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    font: c
                        .extra
                        .get("fontFamily")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    from_library: source.starts_with("http")
                        || source.contains("/library/")
                        || source.contains("\\library\\"),
                },
            );
        }
    }
    map
}

/// Edits that reshuffle or clone clips without adding or removing one in
/// the user's eyes (a split half carries the original's effects).
const RESHUFFLES: [&str; 7] = [
    "Split Clip",
    "Freeze Frame",
    "Replace Clip",
    "Move Clip",
    "Trim Clip",
    "Delete Left",
    "Delete Right",
];

fn gesture(label: &str) -> Option<&'static str> {
    match label {
        "Move Clip" => Some("timeline_clip_moved"),
        "Trim Clip" | "Delete Left" | "Delete Right" => Some("timeline_clip_trimmed"),
        "Split Clip" => Some("timeline_clip_split"),
        _ => None,
    }
}

type Event = (&'static str, Value);

fn effect_events(added: bool, effects: &[&(String, String)], global: bool, out: &mut Vec<Event>) {
    for (_, kind) in effects {
        if let Some(id) = kind.strip_prefix(FILTER_PREFIX) {
            out.push(if added {
                ("filter_applied", json!({ "filterId": id }))
            } else {
                ("filter_removed", json!({ "filterId": id }))
            });
        } else if let Some(id) = kind.strip_prefix(EFFECT_PREFIX) {
            out.push(if added {
                (
                    "effect_applied",
                    json!({ "effectId": id, "scope": if global { "global" } else { "clip" } }),
                )
            } else {
                ("effect_removed", json!({ "effectId": id }))
            });
        }
        // Adjust sliders and the rest: not a feature-adoption signal.
    }
}

fn lifecycle(added: bool, label: &str, info: &ClipInfo, out: &mut Vec<Event>) {
    match info.kind.as_str() {
        "video" | "image" => out.push((
            if added { "clip_added" } else { "clip_removed" },
            json!({ "mediaType": info.kind }),
        )),
        "audio" if added => {
            let source = match label {
                "Extract Audio" => "extracted",
                _ if info.from_library => "library",
                _ => "local",
            };
            out.push(("audio_added", json!({ "source": source })));
        }
        "audio" => out.push(("audio_removed", json!({}))),
        "text" => out.push(if added {
            ("text_added", json!({ "fontFamily": info.font }))
        } else {
            ("text_removed", json!({}))
        }),
        "sticker" => out.push((
            if added {
                "sticker_added"
            } else {
                "sticker_removed"
            },
            json!({}),
        )),
        _ => {} // effect-lane clips are reported through their effect
    }
    let all: Vec<_> = info.effects.iter().collect();
    effect_events(added, &all, info.lane == TrackType::Effect, out);
}

/// The product events one edit (`label`, the undo label) stands for: clip,
/// text, sticker, audio, effect, filter and transition changes, plus the
/// timeline gesture. Pure: counts and catalog names only.
pub fn edit_events(label: &str, before: &Project, after: &Project) -> Vec<Event> {
    let mut out = Vec::new();
    if let Some(g) = gesture(label) {
        out.push((g, json!({})));
    }
    let structural = !RESHUFFLES.contains(&label);
    let prev = index(before);
    let next = index(after);
    for (id, info) in &next {
        let Some(old) = prev.get(id) else {
            if structural {
                lifecycle(true, label, info, &mut out);
                if let Some(t) = &info.transition {
                    out.push(("transition_added", json!({ "transitionId": t })));
                }
            }
            continue;
        };
        let global = info.lane == TrackType::Effect;
        let added: Vec<_> = info
            .effects
            .iter()
            .filter(|e| !old.effects.iter().any(|o| o.0 == e.0))
            .collect();
        let removed: Vec<_> = old
            .effects
            .iter()
            .filter(|o| !info.effects.iter().any(|e| e.0 == o.0))
            .collect();
        effect_events(true, &added, global, &mut out);
        effect_events(false, &removed, global, &mut out);
        if info.transition != old.transition {
            match (&info.transition, &old.transition) {
                (Some(t), _) => out.push(("transition_added", json!({ "transitionId": t }))),
                (None, Some(t)) => out.push(("transition_removed", json!({ "transitionId": t }))),
                _ => {}
            }
        }
    }
    if structural {
        for (id, info) in &prev {
            if !next.contains_key(id) {
                lifecycle(false, label, info, &mut out);
            }
        }
    }
    out
}

fn counts(project: &Project) -> (usize, usize) {
    let clips = project.tracks.iter().map(|t| t.clips.len()).sum();
    let effects = project
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .map(|c| c.effects().len())
        .sum();
    (clips, effects)
}

/// One open project in the editor: the first edit (`editing_started`),
/// gesture rate limits, and the totals of `editor_session_summary`.
#[derive(Default)]
pub struct EditorSession {
    started: Option<Instant>,
    edits: u32,
    undos: u32,
    redos: u32,
    last_gesture: HashMap<&'static str, Instant>,
}

impl EditorSession {
    /// A project opened in the editor (ends the previous session).
    pub fn begin(&mut self, current: Option<&Project>) {
        self.end(current);
        self.started = Some(Instant::now());
    }

    /// One edit went through.
    pub fn edited(&mut self, label: &str, before: &Project, after: &Project) {
        if self.started.is_none() {
            self.started = Some(Instant::now());
        }
        if self.edits == 0 {
            track("editing_started", json!({ "clipsCount": counts(before).0 }));
        }
        self.edits += 1;
        for (name, props) in edit_events(label, before, after) {
            if name.starts_with("timeline_clip_") {
                let now = Instant::now();
                if self
                    .last_gesture
                    .get(name)
                    .is_some_and(|t| now.duration_since(*t) < GESTURE_EVERY)
                {
                    continue;
                }
                self.last_gesture.insert(name, now);
            }
            track(name, props);
        }
    }

    pub fn undone(&mut self) {
        self.undos += 1;
        track("undo", json!({}));
    }

    pub fn redone(&mut self) {
        self.redos += 1;
        track("redo", json!({}));
    }

    /// The project left the editor (closed, another opened, quit).
    pub fn end(&mut self, project: Option<&Project>) {
        let Some(started) = self.started.take() else {
            return;
        };
        let (clips, effects) = project.map(counts).unwrap_or_default();
        track(
            "editor_session_summary",
            json!({
                "sessionMs": started.elapsed().as_millis() as u64,
                "editsCount": self.edits,
                "undoCount": self.undos,
                "redoCount": self.redos,
                "clipsCount": clips,
                "effectsCount": effects,
            }),
        );
        *self = EditorSession::default();
    }
}

/// `export_*` properties of a project at an export's settings.
pub fn export_props(project: &Project, resolution: &str, fps: u32) -> Value {
    let (clips, effects) = counts(project);
    json!({
        "durationMs": project.duration / 1000,
        "resolution": resolution.to_lowercase(),
        "fps": fps,
        "effectsCount": effects,
        "clipsCount": clips,
    })
}

/// Adds `extra`'s fields to the object `base`.
pub fn with(mut base: Value, extra: Value) -> Value {
    if let (Some(b), Value::Object(e)) = (base.as_object_mut(), extra) {
        b.extend(e);
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Clip, EffectInstance};

    #[test]
    fn sanitize_drops_paths_urls_text_and_blocked_keys() {
        let out = sanitize(json!({
            "count": 3,
            "ok": true,
            "effectId": "Glitch",
            "where": "/Users/me/Movies/a.mp4",
            "win": "C:\\Users\\me\\a.mp4",
            "home": "~/a.mp4",
            "link": "see https://example.com",
            "who": "me@example.com",
            "filePath": "x",
            "projectJson": "{}",
            "caption": "hello",
            "long": "x".repeat(65),
            "nested": { "a": 1 },
            "list": [1],
            "nothing": null,
        }));
        let keys: Vec<_> = out.keys().map(String::as_str).collect();
        assert_eq!(keys.len(), 3, "{keys:?}");
        assert!(out.contains_key("count") && out.contains_key("ok"));
        assert_eq!(out["effectId"], "Glitch");
    }

    #[test]
    fn uuids_are_v4_and_differ() {
        let a = random_uuid();
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
        assert_ne!(a, random_uuid());
    }

    fn project_with(clips: Vec<(TrackType, Clip)>) -> Project {
        let mut p = Project::empty();
        for (lane, clip) in clips {
            match p.tracks.iter_mut().find(|t| t.kind == lane) {
                Some(t) => t.clips.push(clip),
                None => {
                    let mut t = p.tracks[0].clone();
                    t.id = format!("t-{lane:?}");
                    t.kind = lane;
                    t.clips = vec![clip];
                    p.tracks.push(t);
                }
            }
        }
        p
    }

    fn clip(id: &str, kind: &str) -> Clip {
        serde_json::from_value(json!({
            "id": id, "kind": kind, "timelineStart": 0, "timelineDuration": 1_000_000,
        }))
        .unwrap()
    }

    fn names(events: &[Event]) -> Vec<&'static str> {
        let mut n: Vec<_> = events.iter().map(|e| e.0).collect();
        n.sort();
        n
    }

    #[test]
    fn adds_and_removes_are_reported_by_kind() {
        let before = project_with(vec![]);
        let mut text = clip("t1", "text");
        text.extra.insert("fontFamily".into(), "Nunito".into());
        let after = project_with(vec![
            (TrackType::Video, clip("v1", "video")),
            (TrackType::Text, text),
        ]);
        let ev = edit_events("Add Clip", &before, &after);
        assert_eq!(names(&ev), ["clip_added", "text_added"]);
        let text = ev.iter().find(|e| e.0 == "text_added").unwrap();
        assert_eq!(text.1["fontFamily"], "Nunito");
        assert_eq!(
            names(&edit_events("Delete Clips", &after, &before)),
            ["clip_removed", "text_removed"]
        );
    }

    #[test]
    fn a_split_is_a_gesture_not_an_add() {
        let before = project_with(vec![(TrackType::Video, clip("v1", "video"))]);
        let after = project_with(vec![
            (TrackType::Video, clip("v1", "video")),
            (TrackType::Video, clip("v2", "video")),
        ]);
        assert_eq!(
            names(&edit_events("Split Clip", &before, &after)),
            ["timeline_clip_split"]
        );
    }

    #[test]
    fn effects_filters_and_transitions_on_a_kept_clip() {
        let before = project_with(vec![(TrackType::Video, clip("v1", "video"))]);
        let mut c = clip("v1", "video");
        c.set_effects(&[
            EffectInstance::new("Overlays:Glitch".into(), &[]),
            EffectInstance::new("Filters:Honey".into(), &[]),
        ]);
        c.extra.insert(
            "outTransition".into(),
            json!({ "type": "fade", "durationMicros": 1 }),
        );
        let after = project_with(vec![(TrackType::Video, c)]);
        let ev = edit_events("Add Effect", &before, &after);
        assert_eq!(
            names(&ev),
            ["effect_applied", "filter_applied", "transition_added"]
        );
        let fx = ev.iter().find(|e| e.0 == "effect_applied").unwrap();
        assert_eq!(fx.1, json!({ "effectId": "Glitch", "scope": "clip" }));
        assert_eq!(
            names(&edit_events("Delete Effect", &after, &before)),
            ["effect_removed", "filter_removed", "transition_removed"]
        );
    }

    #[test]
    fn audio_names_its_source() {
        let before = project_with(vec![]);
        let mut lib = clip("a1", "audio");
        lib.source = Some("https://media.example/x.mp3".into());
        let after = project_with(vec![(TrackType::Audio, lib)]);
        let ev = edit_events("Add Audio", &before, &after);
        assert_eq!(ev, vec![("audio_added", json!({ "source": "library" }))]);
        let ev = edit_events("Extract Audio", &before, &after);
        assert_eq!(ev[0].1["source"], "extracted");
    }
}
