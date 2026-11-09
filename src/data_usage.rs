//! Bytes Reel Studio's own features downloaded, per category, since the
//! counts were last reset (Settings → Data and Storage → Data Usage). The
//! downloads call `record`; previews streamed while browsing (tile
//! pictures, sticker thumbnails) aren't counted, as the page says.
//!
//! Kept in `<data>/data-usage.json`, written by a small thread a few
//! seconds after the last count (a burst of effect packages is one write).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// What a download was for: the Data Usage rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Effects,
    Stickers,
    Music,
    Sounds,
    AiModels,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Effects,
        Kind::Stickers,
        Kind::Music,
        Kind::Sounds,
        Kind::AiModels,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Effects => "Effects & filters",
            Kind::Stickers => "Stickers & GIFs",
            Kind::Music => "Music",
            Kind::Sounds => "Sound effects",
            Kind::AiModels => "AI models",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Usage {
    /// When counting started, ms since the epoch.
    pub since: u64,
    pub bytes: BTreeMap<Kind, u64>,
}

impl Usage {
    fn new() -> Usage {
        Usage {
            since: now_ms(),
            bytes: BTreeMap::new(),
        }
    }

    pub fn get(&self, k: Kind) -> u64 {
        self.bytes.get(&k).copied().unwrap_or(0)
    }

    pub fn total(&self) -> u64 {
        self.bytes.values().sum()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn path() -> PathBuf {
    crate::prefs::data_dir().join("data-usage.json")
}

struct State {
    usage: Option<Usage>,
    dirty: bool,
    writer: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    usage: None,
    dirty: false,
    writer: false,
});
static WAKE: Condvar = Condvar::new();

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if s.usage.is_none() {
        let saved = std::fs::read(path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let fresh = saved.is_none();
        s.usage = Some(saved.unwrap_or_else(Usage::new));
        // Counting starts now: keep that date, downloads or not.
        if fresh {
            mark_dirty(&mut s);
        }
    }
    f(&mut s)
}

/// The counts so far.
pub fn snapshot() -> Usage {
    with(|s| s.usage.clone().unwrap_or_else(Usage::new))
}

/// Counts a download (never fails, never waits on the disk).
pub fn record(kind: Kind, bytes: u64) {
    if bytes == 0 {
        return;
    }
    with(|s| {
        if let Some(u) = &mut s.usage {
            *u.bytes.entry(kind).or_default() += bytes;
        }
        mark_dirty(s);
    });
}

/// Starts the counts again from zero, from now.
pub fn reset() {
    with(|s| {
        s.usage = Some(Usage::new());
        mark_dirty(s);
    });
}

fn mark_dirty(s: &mut State) {
    s.dirty = true;
    if !s.writer {
        s.writer = true;
        let _ = std::thread::Builder::new()
            .name("data-usage".into())
            .spawn(writer);
    }
    WAKE.notify_one();
}

/// Writes the counts a moment after they last changed.
fn writer() {
    loop {
        let usage = {
            let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
            while !s.dirty {
                s = WAKE.wait(s).unwrap_or_else(|e| e.into_inner());
            }
            drop(s);
            std::thread::sleep(Duration::from_secs(2));
            let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
            s.dirty = false;
            s.usage.clone()
        };
        if let Some(json) = usage.and_then(|u| serde_json::to_vec_pretty(&u).ok()) {
            let p = path();
            let tmp = p.with_extension("json.part");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }
}

/// "Nov 9, 2026, 3:04 PM", in local time.
pub fn format_since(ms: u64) -> String {
    use chrono::TimeZone as _;
    match chrono::Local.timestamp_millis_opt(ms as i64).single() {
        Some(t) => t.format("%b %-d, %Y, %-I:%M %p").to_string(),
        None => String::new(),
    }
}
