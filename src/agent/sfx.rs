//! Real recorded sound effects for the agent: found, kept, measured.
//!
//! Two sources, searched together:
//! - **the app's own sound library** (the browser's Audio › Sound effects
//!   table, `library.rs`), curated and cleared for use;
//! - **Openverse**, the open catalogue of openly licensed media, whose audio
//!   is mostly Freesound recordings. Only CC0 and CC BY are taken (free for
//!   any use; BY needs the credit, which the clip carries in `attribution`).
//!   It answers without a key (20 searches a minute, 200 a day, at most
//!   20 results a page: more needs a key), so each search is kept on disk
//!   for two weeks.
//!
//! A sound used in a project is kept for good in `<data>/library/openverse/`
//! with its licence beside it, so a saved project plays offline.
//!
//! `analyze` measures where a recording is audible: its onset, its loudest
//! moment and where its tail dies away, so a cue can put the right part of
//! a sound on the frame (a whoosh's peak on the cut, a hit's attack on the
//! title, a riser's end on the reveal) and drop the silence around it.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::library::{self, SoundEffect, SoundQuery};
use crate::model::Micros;

const OPENVERSE: &str = "https://api.openverse.org/v1/audio/";
/// Search results kept this long (the anonymous daily quota is small).
const SEARCH_TTL: Duration = Duration::from_secs(14 * 24 * 3600);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SoundHit {
    /// "ov:<uuid>" or "lib:<id>": stable across searches.
    pub key: String,
    pub title: String,
    /// "Openverse · freesound" or "Sound library".
    pub source: String,
    /// Seconds, when the source says.
    pub duration: Option<f64>,
    /// "cc0", "by", or "" for the app's own library.
    pub license: String,
    pub creator: String,
    /// The credit line to keep with the clip ("" when none is needed).
    pub attribution: String,
    pub url: String,
    pub tags: Vec<String>,
    /// The app library's row (to download it the library's way).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<SoundEffect>,
}

impl SoundHit {
    /// What the model reads about it.
    pub fn summary(&self, alias: &str) -> Value {
        let mut v = serde_json::json!({
            "id": alias,
            "name": self.title,
            "source": self.source,
        });
        if let Some(d) = self.duration {
            v["duration"] = serde_json::json!((d * 100.).round() / 100.);
        }
        if !self.license.is_empty() {
            v["license"] = self.license.to_uppercase().into();
        }
        if !self.tags.is_empty() {
            v["tags"] = self.tags.iter().take(6).cloned().collect::<Vec<_>>().into();
        }
        v
    }
}

fn data_dir() -> PathBuf {
    crate::prefs::data_dir().join("library").join("openverse")
}

/// Sounds the agent used lately, newest last: a new video passes over them
/// when it has others, so videos don't all sound alike.
fn recent_path() -> PathBuf {
    crate::prefs::data_dir()
        .join("agent")
        .join("sfx-recent.json")
}

/// How many recent sounds are remembered.
const RECENT: usize = 80;

pub fn recently_used() -> Vec<String> {
    std::fs::read(recent_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Remembers `keys` as just used (best effort).
pub fn mark_used<'a>(keys: impl IntoIterator<Item = &'a str>) {
    let mut list = recently_used();
    for k in keys {
        list.retain(|x| x != k);
        list.push(k.to_string());
    }
    let from = list.len().saturating_sub(RECENT);
    let path = recent_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(bytes) = serde_json::to_vec(&list[from..]) {
        let _ = std::fs::write(path, bytes);
    }
}

fn cache_path(query: &str) -> PathBuf {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in query.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    crate::prefs::data_dir()
        .join("cache")
        .join("openverse")
        .join(format!("{h:016x}.json"))
}

fn query_value(s: &str) -> String {
    crate::catalog::query_value(s)
}

/// One Openverse result row as a hit (None: not a usable sound).
fn openverse_hit(r: &Value) -> Option<SoundHit> {
    let license = r.get("license")?.as_str()?.to_lowercase();
    if license != "cc0" && license != "by" {
        return None;
    }
    let url = r.get("url")?.as_str()?.to_string();
    let id = r.get("id")?.as_str()?;
    let s = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let provider = s("provider");
    Some(SoundHit {
        key: format!("ov:{id}"),
        title: s("title").chars().take(80).collect(),
        source: if provider.is_empty() {
            "Openverse".into()
        } else {
            format!("Openverse · {provider}")
        },
        duration: r
            .get("duration")
            .and_then(Value::as_f64)
            .map(|ms| ms / 1000.),
        license: license.clone(),
        creator: s("creator"),
        attribution: if license == "cc0" {
            String::new()
        } else {
            s("attribution")
        },
        url,
        tags: r
            .get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|t| t.get("name").and_then(Value::as_str).map(String::from))
            .take(12)
            .collect(),
        library: None,
    })
}

/// Openverse's short CC0 / CC BY sounds for `query` (kept on disk).
pub fn openverse(query: &str) -> Result<Vec<SoundHit>> {
    let q = query.trim().to_lowercase();
    let cache = cache_path(&q);
    let fresh = std::fs::metadata(&cache)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < SEARCH_TTL);
    let body = if fresh {
        std::fs::read_to_string(&cache)?
    } else {
        let url = format!(
            "{OPENVERSE}?q={}&license=cc0,by&length=shortest&page_size=20&mature=false",
            query_value(&q)
        );
        let mut response = crate::assets::agent()
            .get(&url)
            .header("User-Agent", "ReelStudio/desktop (video editor)")
            .call()
            .context("Openverse search")?;
        let body = response.body_mut().read_to_string()?;
        if let Some(dir) = cache.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&cache, &body);
        body
    };
    let json: Value = serde_json::from_str(&body)?;
    if let Some(detail) = json.get("detail").and_then(Value::as_str) {
        bail!("Openverse: {detail}");
    }
    Ok(json
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(openverse_hit)
        .collect())
}

/// The app library's sounds whose name matches `query`.
pub fn app_library(query: &str) -> Result<Vec<SoundHit>> {
    let words: Vec<&str> = query.split_whitespace().collect();
    // The table matches names: the most telling word works best.
    let word = words
        .iter()
        .max_by_key(|w| w.len())
        .copied()
        .unwrap_or(query);
    Ok(library::sounds(&SoundQuery::Search(word.to_string()))?
        .into_iter()
        .map(|e| SoundHit {
            key: format!("lib:{}", e.id),
            title: e.name.clone(),
            source: "Sound library".into(),
            duration: None,
            license: String::new(),
            creator: String::new(),
            attribution: String::new(),
            url: library::sound_url(&e).unwrap_or_default(),
            tags: vec![e.category.clone()],
            library: Some(e),
        })
        .collect())
}

/// Both sources for `query`, the best fits first: the words in the name or
/// tags, a length under `max_secs`, the app's library before Openverse.
pub fn search(query: &str, max_secs: f64) -> Result<Vec<SoundHit>> {
    let mut hits = vec![];
    let mut errors = vec![];
    match app_library(query) {
        Ok(h) => hits.extend(h),
        Err(e) => errors.push(format!("{e:#}")),
    }
    match openverse(query) {
        Ok(h) => hits.extend(h),
        Err(e) => errors.push(format!("{e:#}")),
    }
    if hits.is_empty() && !errors.is_empty() {
        bail!("{}", errors.join("; "));
    }
    let words: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(String::from)
        .collect();
    let score = |h: &SoundHit| {
        let text = format!(
            "{} {}",
            h.title.to_lowercase(),
            h.tags.join(" ").to_lowercase()
        );
        let matched = words.iter().filter(|w| text.contains(w.as_str())).count() as f64;
        let fit = match h.duration {
            Some(d) if d > max_secs * 3. => -3.,
            Some(d) if d > max_secs => -1.,
            Some(d) if d < 0.08 => -2.,
            Some(_) => 0.5,
            None => 0.,
        };
        let library = if h.library.is_some() { 0.75 } else { 0. };
        // Loop / ambience files rarely work as one-shots.
        let loopy = ["loop", "ambience", "ambient", "music", "song"]
            .iter()
            .any(|w| text.contains(w) && !words.iter().any(|q| q == w));
        matched * 2. + fit + library - if loopy { 3. } else { 0. }
    };
    hits.sort_by(|a, b| score(b).total_cmp(&score(a)));
    hits.dedup_by(|a, b| a.key == b.key);
    Ok(hits)
}

/// Sound effects are small: a server that stalls longer than this is
/// skipped for the next result, never waited on (one stuck download held
/// a whole `make_video` for minutes).
fn download_agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(8)))
                .timeout_global(Some(Duration::from_secs(30)))
                .build(),
        )
    })
}

/// The sound's local file (downloaded once, kept with its licence).
pub fn download(hit: &SoundHit) -> Result<PathBuf> {
    if let Some(e) = &hit.library {
        return library::download_sound(e);
    }
    let id: String = hit
        .key
        .trim_start_matches("ov:")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let ext = hit
        .url
        .rsplit('.')
        .next()
        .filter(|e| e.len() <= 4 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("mp3")
        .to_lowercase();
    let path = data_dir().join(format!("{id}.{ext}"));
    if path.exists() {
        return Ok(path);
    }
    let mut response = download_agent()
        .get(&hit.url)
        .header("User-Agent", "ReelStudio/desktop (video editor)")
        .call()
        .with_context(|| hit.url.clone())?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(40 << 20)
        .read_to_vec()?;
    crate::data_usage::record(crate::data_usage::Kind::Sounds, bytes.len() as u64);
    std::fs::create_dir_all(data_dir())?;
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    if let Ok(json) = serde_json::to_string_pretty(hit) {
        let _ = std::fs::write(path.with_extension("json"), json);
    }
    Ok(path)
}

/// Where a recording is audible, in its own time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub length: Micros,
    /// The attack: first moment within 20 dB of the peak.
    pub onset: Micros,
    pub peak: Micros,
    /// Where the tail falls 40 dB under the peak.
    pub tail: Micros,
}

/// Measures `peaks` (loudest sample per slice, `per_second` slices).
pub fn shape_of(peaks: &[f32], per_second: f64) -> Option<Shape> {
    let (peak_i, &max) = peaks.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    if max <= 1e-4 {
        return None;
    }
    let at = |i: usize| (i as f64 / per_second * 1e6) as Micros;
    let onset = peaks.iter().position(|&p| p >= max * 0.1).unwrap_or(0);
    let tail = peaks
        .iter()
        .rposition(|&p| p >= max * 0.01)
        .unwrap_or(peaks.len() - 1);
    Some(Shape {
        length: at(peaks.len()),
        onset: at(onset),
        peak: at(peak_i),
        tail: at(tail + 1),
    })
}

pub fn analyze(path: &std::path::Path) -> Result<Shape> {
    let wave = crate::decoder::waveform(&path.to_string_lossy())?;
    shape_of(&wave.peaks, wave.per_second).context("the file is silent")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_a_whoosh() {
        // 1 s: silence, a rise to a peak at 0.5 s, a tail to 0.8 s.
        let mut peaks = vec![0.0f32; 100];
        for (i, p) in peaks.iter_mut().enumerate().take(80).skip(20) {
            *p = if i <= 50 {
                (i - 20) as f32 / 30.
            } else {
                (80 - i) as f32 / 30.
            };
        }
        let s = shape_of(&peaks, 100.).unwrap();
        assert_eq!(s.length, 1_000_000);
        assert_eq!(s.peak, 500_000);
        assert!(s.onset > 200_000 && s.onset < 260_000, "{s:?}");
        assert!(s.tail >= 790_000, "{s:?}");
        assert!(shape_of(&[0.; 10], 100.).is_none());
    }

    #[test]
    fn reads_openverse_rows() {
        let row = serde_json::json!({
            "id": "af28", "title": "Swipe Whoosh", "url": "https://cdn.example/a.mp3",
            "creator": "someone", "license": "by", "provider": "freesound",
            "attribution": "\"Swipe Whoosh\" by someone is licensed under CC BY 4.0.",
            "duration": 447, "tags": [{ "name": "whoosh" }],
        });
        let hit = openverse_hit(&row).unwrap();
        assert_eq!(hit.key, "ov:af28");
        assert_eq!(hit.duration, Some(0.447));
        assert!(hit.attribution.contains("CC BY"));
        let nc = serde_json::json!({ "id": "x", "url": "u", "license": "by-nc" });
        assert!(openverse_hit(&nc).is_none());
    }

    /// Live: Openverse answers a search (needs the network).
    #[test]
    #[ignore]
    fn openverse_answers() {
        let hits = openverse("whoosh").unwrap();
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| h.license == "cc0" || h.license == "by"));
    }
}
