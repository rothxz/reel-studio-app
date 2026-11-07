//! The online libraries the browser's Audio and Sticker tabs list.
//!
//! - **Stickers and GIFs**, and **music**, come from third-party media APIs
//!   through the app's media proxy (an edge function next to the catalog
//!   database, which holds the API keys; this app only knows the
//!   database's public URL and key, as the web build does).
//! - **Sound effects** are a table in the catalog database; their files
//!   are on the asset host (`sound-effects/<storage_path>`).
//!
//! Every call blocks: run them off the UI thread. A failure is an error
//! (the view shows it with Retry), an empty answer an empty list.
//!
//! What a project uses is kept for good in `<data>/library/` (music,
//! sound effects), so a saved project plays offline; sticker files are
//! cached by URL (`assets::REMOTE`), the clip keeps the URL.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{assets, catalog};

/// Results per request (a couple of screens of tiles).
pub const PAGE_SIZE: usize = 30;

const CLIENT_ID: &str = "reel-studio-app";

/// A GET through the media proxy: `api` is the upstream ("klipy" for
/// stickers / GIFs, "jamendo" for music), `path` its endpoint.
fn proxy_get(api: &str, path: &str, params: &[(&str, String)]) -> Result<Value> {
    let Some((base, key)) = catalog::database() else {
        bail!("no catalog database configured");
    };
    let query: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{k}={}", catalog::query_value(v)))
        .collect();
    let url = format!(
        "{base}/functions/v1/media-proxy/{api}/{path}?{}",
        query.join("&")
    );
    let body = assets::agent()
        .get(&url)
        .header("apikey", &key)
        .call()
        .with_context(|| format!("media proxy {api}/{path}"))?
        .body_mut()
        .read_to_string()?;
    Ok(serde_json::from_str(&body)?)
}

// ---- stickers + GIFs ----------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StickerKind {
    Stickers,
    Gifs,
}

impl StickerKind {
    fn path(self) -> &'static str {
        match self {
            StickerKind::Stickers => "stickers",
            StickerKind::Gifs => "gifs",
        }
    }
}

/// One sticker or GIF.
#[derive(Clone, Debug, PartialEq)]
pub struct StickerItem {
    pub id: String,
    pub title: String,
    /// The file a clip plays (GIF or WebP).
    pub url: String,
    /// A smaller copy for the tile.
    pub preview_url: String,
    pub width: u32,
    pub height: u32,
}

/// The Stickers view's quick picks (searches), then the API's own
/// categories; Trending last, as the old picker. (label, query; None =
/// trending.)
pub const STICKER_PICKS: &[(&str, Option<&str>)] = &[
    ("Arrows", Some("arrow")),
    ("Circles", Some("red circle")),
    ("Highlight", Some("highlight")),
    ("Check", Some("check mark")),
    ("Cross", Some("cross mark")),
    ("Stars", Some("star")),
    ("Hearts", Some("heart")),
    ("Cats", Some("cat")),
    ("Emoji", Some("emoji")),
    ("Subscribe", Some("subscribe")),
    ("Trending", None),
];

fn sticker_item(v: &Value) -> Option<StickerItem> {
    let file = v.get("file")?;
    // A size's file: the GIF, else the WebP.
    let pick = |size: &str| {
        let s = file.get(size)?;
        s.get("gif")
            .or_else(|| s.get("webp"))
            .filter(|f| f.get("url").is_some())
    };
    let md = pick("md");
    let sm = pick("sm");
    let main = md.or(sm)?;
    let url_of = |f: &Value| f.get("url").and_then(Value::as_str).map(str::to_string);
    let dim = |k: &str| main.get(k).and_then(Value::as_u64).unwrap_or(200) as u32;
    Some(StickerItem {
        id: match v.get("id")? {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        },
        title: v
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        url: url_of(main)?,
        preview_url: sm.and_then(url_of).or_else(|| url_of(main))?,
        width: dim("width").max(1),
        height: dim("height").max(1),
    })
}

/// A page (from 1) of stickers or GIFs: a search, or what's trending.
pub fn stickers(kind: StickerKind, query: Option<&str>, page: usize) -> Result<Vec<StickerItem>> {
    let mut params = vec![
        ("media_filter", "minimal".to_string()),
        ("contentfilter", "medium".to_string()),
        ("locale", "en".to_string()),
        ("customer_id", CLIENT_ID.to_string()),
        ("per_page", PAGE_SIZE.to_string()),
        ("page", page.max(1).to_string()),
    ];
    let action = match query.map(str::trim).filter(|q| !q.is_empty()) {
        Some(q) => {
            params.push(("q", q.to_string()));
            "search"
        }
        None => "trending",
    };
    let json = proxy_get("klipy", &format!("{}/{action}", kind.path()), &params)?;
    Ok(json
        .pointer("/data/data")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().filter_map(sticker_item).collect())
        .unwrap_or_default())
}

/// The sticker API's own categories (label, query).
pub fn sticker_categories() -> Result<Vec<(String, String)>> {
    let json = proxy_get(
        "klipy",
        "stickers/categories",
        &[
            ("locale", "en".to_string()),
            ("customer_id", CLIENT_ID.to_string()),
        ],
    )?;
    Ok(json
        .pointer("/data/categories")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|c| {
                    let label = c.get("category")?.as_str()?.to_string();
                    let query = c
                        .get("query")
                        .and_then(Value::as_str)
                        .unwrap_or(&label)
                        .to_string();
                    Some((label, query))
                })
                .collect()
        })
        .unwrap_or_default())
}

// ---- music ----------------------------------------------------------------------

/// The Music view's moods (label, tag).
pub const MUSIC_MOODS: &[(&str, &str)] = &[
    ("Chill", "chillout"),
    ("Upbeat", "upbeat"),
    ("Cinematic", "cinematic"),
    ("Corporate", "corporate"),
    ("Electronic", "electronic"),
    ("Acoustic", "acoustic"),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MusicTrack {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub duration_secs: f64,
    /// Streamable; good for a while (it carries a token), so a pick is
    /// downloaded rather than referenced.
    pub audio_url: String,
    pub image_url: String,
}

impl MusicTrack {
    /// The clip's title: "Song – Artist".
    pub fn clip_title(&self) -> String {
        if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!("{} – {}", self.title, self.artist)
        }
    }
}

/// What a music list shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MusicQuery {
    Mood(&'static str),
    Search(String),
}

fn music_track(v: &Value) -> Option<MusicTrack> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let audio_url = s("audio");
    if audio_url.is_empty() {
        return None;
    }
    Some(MusicTrack {
        id: match v.get("id")? {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        },
        title: s("name"),
        artist: s("artist_name"),
        duration_secs: v
            .get("duration")
            .and_then(|d| d.as_f64().or_else(|| d.as_str()?.parse().ok()))
            .unwrap_or(0.),
        audio_url,
        image_url: match s("album_image") {
            x if x.is_empty() => s("image"),
            x => x,
        },
    })
}

/// A music list. The upstream now and then answers an empty "success",
/// so an empty or failed answer is asked for again (twice).
pub fn music(query: &MusicQuery) -> Result<Vec<MusicTrack>> {
    let mut params = vec![
        ("format", "json".to_string()),
        ("include", "musicinfo".to_string()),
        ("audioformat", "mp32".to_string()),
        ("limit", PAGE_SIZE.to_string()),
    ];
    match query {
        MusicQuery::Mood(tag) => params.push(("tags", tag.to_string())),
        MusicQuery::Search(q) => params.push(("search", q.clone())),
    }
    let mut last = None;
    for attempt in 0..3 {
        if attempt > 0 {
            // A different URL each time: no cache hands the empty one back.
            params.retain(|(k, _)| *k != "_");
            params.push(("_", attempt.to_string()));
        }
        match proxy_get("jamendo", "tracks", &params) {
            Ok(json) => {
                let tracks: Vec<MusicTrack> = json
                    .get("results")
                    .and_then(Value::as_array)
                    .map(|rows| rows.iter().filter_map(music_track).collect())
                    .unwrap_or_default();
                if !tracks.is_empty() {
                    return Ok(tracks);
                }
                last = None;
            }
            Err(e) => last = Some(e),
        }
    }
    match last {
        Some(e) => Err(e),
        None => Ok(vec![]),
    }
}

/// Downloads a track for a clip (kept: `<data>/library/music/<id>.mp3`).
pub fn download_track(track: &MusicTrack) -> Result<PathBuf> {
    let dir = crate::prefs::data_dir().join("library").join("music");
    let safe: String = track
        .id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let path = dir.join(format!("{safe}.mp3"));
    if path.exists() {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir)?;
    let mut response = assets::download_agent()
        .get(&track.audio_url)
        .call()
        .context("downloading the track")?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(200 << 20)
        .read_to_vec()?;
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

// ---- sound effects ------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SoundEffect {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub storage_path: String,
    #[serde(default)]
    pub format: String,
}

/// What a sound list shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SoundQuery {
    /// The newest (no categories).
    Newest,
    Category(String),
    Search(String),
}

fn table_get(query: &str) -> Result<String> {
    let Some((base, key)) = catalog::database() else {
        bail!("no catalog database configured");
    };
    let url = format!("{base}/rest/v1/sound_effects?{query}");
    Ok(assets::agent()
        .get(&url)
        .header("apikey", &key)
        .header("Authorization", &format!("Bearer {key}"))
        .call()
        .context("sound effects")?
        .body_mut()
        .read_to_string()?)
}

/// The sound categories, sorted.
pub fn sound_categories() -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Row {
        category: String,
    }
    let rows: Vec<Row> = serde_json::from_str(&table_get("select=category")?)?;
    let mut list: Vec<String> = rows.into_iter().map(|r| r.category).collect();
    list.sort_by_key(|c| c.to_lowercase());
    list.dedup();
    Ok(list)
}

pub fn sounds(query: &SoundQuery) -> Result<Vec<SoundEffect>> {
    let filter = match query {
        SoundQuery::Newest => "order=created_at.desc".to_string(),
        SoundQuery::Category(c) => format!("category=eq.{}&order=name", catalog::query_value(c)),
        SoundQuery::Search(q) => format!(
            "name=ilike.{}&order=name",
            catalog::query_value(&format!("*{}*", q.trim()))
        ),
    };
    let body = table_get(&format!(
        "select=id,name,category,storage_path,format&{filter}&limit=60"
    ))?;
    Ok(serde_json::from_str(&body)?)
}

fn sounds_dir() -> PathBuf {
    crate::prefs::data_dir()
        .join("library")
        .join("sound-effects")
}

/// Where a sound effect is kept once downloaded.
fn sound_path(effect: &SoundEffect) -> PathBuf {
    let safe: String = effect
        .id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let ext = if effect.format.is_empty() {
        "mp3"
    } else {
        effect.format.as_str()
    };
    sounds_dir().join(format!("{safe}.{ext}"))
}

/// The local copy of a sound effect, if it's been downloaded.
pub fn downloaded_sound(effect: &SoundEffect) -> Option<PathBuf> {
    let p = sound_path(effect);
    p.exists().then_some(p)
}

/// Downloads a sound effect for a clip (kept: `<data>/library/
/// sound-effects/<id>.<format>`, with its row beside it for My sounds).
pub fn download_sound(effect: &SoundEffect) -> Result<PathBuf> {
    let path = sound_path(effect);
    if path.exists() {
        return Ok(path);
    }
    let cached = assets::fetch_file(assets::SOUND_EFFECTS, &effect.storage_path)?;
    std::fs::create_dir_all(sounds_dir())?;
    std::fs::copy(&cached, &path)?;
    if let Ok(json) = serde_json::to_string(effect) {
        let _ = std::fs::write(path.with_extension("json"), json);
    }
    Ok(path)
}

/// Every sound effect downloaded on this machine, by name.
pub fn my_sounds() -> Vec<SoundEffect> {
    let Ok(dir) = std::fs::read_dir(sounds_dir()) else {
        return vec![];
    };
    let mut list: Vec<SoundEffect> = dir
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
        .filter(|s| downloaded_sound(s).is_some())
        .collect();
    list.sort_by_key(|s| s.name.to_lowercase());
    list
}

/// A sound effect's file URL (for a preview listen before it's added).
pub fn sound_url(effect: &SoundEffect) -> Option<String> {
    assets::url(assets::SOUND_EFFECTS, &effect.storage_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_sticker_row() {
        let row: Value = serde_json::json!({
            "id": 47, "title": "Dance",
            "file": {
                "md": { "gif": { "url": "https://x/md.gif", "width": 480, "height": 360 } },
                "sm": { "webp": { "url": "https://x/sm.webp", "width": 200, "height": 150 } }
            }
        });
        let s = sticker_item(&row).unwrap();
        assert_eq!(s.id, "47");
        assert_eq!(s.url, "https://x/md.gif");
        assert_eq!(s.preview_url, "https://x/sm.webp");
        assert_eq!((s.width, s.height), (480, 360));
    }

    #[test]
    fn reads_a_track_row() {
        let row: Value = serde_json::json!({
            "id": "2034080", "name": "Soothe", "artist_name": "Someone",
            "duration": 216, "audio": "https://x/a", "album_image": "https://x/i"
        });
        let t = music_track(&row).unwrap();
        assert_eq!(t.duration_secs, 216.);
        assert_eq!(t.clip_title(), "Soothe – Someone");
        assert!(music_track(&serde_json::json!({ "id": 1 })).is_none());
    }

    /// The live services answer (needs the network and the catalog).
    #[test]
    #[ignore]
    fn live_libraries_answer() {
        assert!(!stickers(StickerKind::Stickers, None, 1).unwrap().is_empty());
        assert!(
            !stickers(StickerKind::Gifs, Some("cat"), 1)
                .unwrap()
                .is_empty()
        );
        assert!(!sticker_categories().unwrap().is_empty());
        assert!(!music(&MusicQuery::Mood("chillout")).unwrap().is_empty());
        let cats = sound_categories().unwrap();
        assert!(!cats.is_empty());
        assert!(
            !sounds(&SoundQuery::Category(cats[0].clone()))
                .unwrap()
                .is_empty()
        );
    }
}
