//! What Reel Studio keeps on this computer, by category (Settings → Data
//! and Storage → Storage Usage). Cache categories hold files the app
//! downloads or makes again on its own, so they can be cleared and trimmed;
//! the rest is the user's work and downloads, only measured here.
//!
//! ```text
//! effects    <data>/cache/effect-packages   effect and filter shaders
//! previews   <data>/cache/effect-previews   tile pictures,
//!            <data>/cache/remote            sticker / GIF files, covers
//! other      <data>/cache/sound-effects     the host copies of sounds
//!            <data>/cache/openverse         sound search answers
//!            <data>/cache/captions          auto-caption scratch audio
//!            <data>/cache/face-swap         processed face swaps (made again)
//!
//! projects   <data>/projects, <data>/agent/scenes, <data>/agent/voice
//! sounds     <data>/library/sound-effects, <data>/library/openverse
//! music      <data>/library/music           (clips point at these files)
//! aiModels   <data>/models (speech to text, face swap), <data>/agent/speech (voice)
//! ```
//!
//! The data folder is shared with the older desktop build, whose own files
//! (`Cache/`, `IndexedDB/`…) are never counted or touched: on a
//! case-insensitive disk its `Cache/` is our `cache/`, so only our own
//! subfolders are ever deleted.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::prefs::data_dir;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Projects,
    Sounds,
    Music,
    AiModels,
    Effects,
    Previews,
    Other,
}

/// The cache categories, in the order the list shows them.
pub const CACHE: [Category; 3] = [Category::Effects, Category::Previews, Category::Other];
/// The user's own data, in the order the list shows them.
pub const KEPT: [Category; 4] = [
    Category::Projects,
    Category::Sounds,
    Category::Music,
    Category::AiModels,
];

impl Category {
    /// The English label (also the dictionary key, see i18n.rs).
    pub fn label(self) -> &'static str {
        match self {
            Category::Projects => "Projects & media",
            Category::Sounds => "Sound effects",
            Category::Music => "Music",
            Category::AiModels => "AI models",
            Category::Effects => "Effects & filters",
            Category::Previews => "Previews",
            Category::Other => "Other",
        }
    }

    /// The folders this category owns, under the data folder.
    fn dirs(self) -> Vec<PathBuf> {
        let d = data_dir();
        let cache = d.join("cache");
        match self {
            Category::Projects => vec![
                d.join("projects"),
                d.join("agent").join("scenes"),
                d.join("agent").join("voice"),
            ],
            Category::Sounds => vec![
                d.join("library").join("sound-effects"),
                d.join("library").join("openverse"),
            ],
            Category::Music => vec![d.join("library").join("music")],
            Category::AiModels => vec![d.join("models"), d.join("agent").join("speech")],
            Category::Effects => vec![cache.join(crate::assets::EFFECT_PACKAGES)],
            Category::Previews => vec![
                cache.join(crate::assets::EFFECT_PREVIEWS),
                cache.join(crate::assets::REMOTE),
            ],
            Category::Other => vec![
                cache.join(crate::assets::SOUND_EFFECTS),
                cache.join("openverse"),
                cache.join("captions"),
                cache.join("face-swap"),
            ],
        }
    }

    pub fn is_cache(self) -> bool {
        CACHE.contains(&self)
    }
}

/// Bytes per category, and the free space on the data folder's disk.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub bytes: Vec<(Category, u64)>,
    /// 0 when unknown.
    pub free: u64,
}

impl Report {
    pub fn get(&self, c: Category) -> u64 {
        self.bytes
            .iter()
            .find(|(k, _)| *k == c)
            .map_or(0, |(_, b)| *b)
    }

    pub fn total(&self) -> u64 {
        self.bytes.iter().map(|(_, b)| b).sum()
    }
}

/// Measures every category. Walks the disk: run it off the UI thread.
pub fn measure() -> Report {
    let bytes = KEPT
        .iter()
        .chain(CACHE.iter())
        .map(|&c| (c, c.dirs().iter().map(|d| dir_bytes(d)).sum()))
        .collect();
    Report {
        bytes,
        free: free_space(&data_dir()),
    }
}

/// Bytes under `path` (symlinks are not followed: the voice's Python
/// environment links to the system's).
pub fn dir_bytes(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return if meta.is_file() { meta.len() } else { 0 };
    }
    std::fs::read_dir(path)
        .map(|entries| entries.flatten().map(|e| dir_bytes(&e.path())).sum())
        .unwrap_or(0)
}

#[cfg(unix)]
fn free_space(path: &Path) -> u64 {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return 0;
    };
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return 0;
    }
    (s.f_bavail as u64).saturating_mul(s.f_frsize as u64)
}

#[cfg(not(unix))]
fn free_space(_: &Path) -> u64 {
    0
}

/// Deletes the given categories' files (cache categories only; the
/// caller refuses while a job may be reading them).
pub fn clear(categories: &[Category]) {
    for c in categories.iter().filter(|c| c.is_cache()) {
        for dir in c.dirs() {
            remove(&dir);
        }
    }
}

/// Deletes a file or folder, quietly (gone already is fine).
pub fn remove(path: &Path) {
    let r = match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(_) => return,
    };
    if let Err(e) = r {
        eprintln!("storage: couldn't delete {}: {e}", path.display());
    }
}

// ---- auto-remove ----------------------------------------------------------

/// Settings → Storage Usage → Auto-remove cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeepCache {
    Week,
    Month,
    #[default]
    Forever,
}

impl KeepCache {
    pub const ALL: [KeepCache; 3] = [KeepCache::Week, KeepCache::Month, KeepCache::Forever];

    pub fn label(self) -> &'static str {
        match self {
            KeepCache::Week => "After 1 week",
            KeepCache::Month => "After 1 month",
            KeepCache::Forever => "Never",
        }
    }

    pub fn max_age(self) -> Option<Duration> {
        const DAY: u64 = 24 * 60 * 60;
        match self {
            KeepCache::Week => Some(Duration::from_secs(7 * DAY)),
            KeepCache::Month => Some(Duration::from_secs(30 * DAY)),
            KeepCache::Forever => None,
        }
    }
}

const GB: u64 = 1 << 30;

/// The Maximum cache size stops; None = no limit (the default, last).
pub const LIMITS: [Option<u64>; 4] = [Some(GB), Some(5 * GB), Some(20 * GB), None];

/// A cached file, for trimming.
#[derive(Clone, Debug, PartialEq)]
pub struct Cached {
    pub path: PathBuf,
    pub bytes: u64,
    /// Last write (None: unknown, counts as new).
    pub modified: Option<SystemTime>,
}

fn walk(dir: &Path, out: &mut Vec<Cached>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_dir() {
            walk(&e.path(), out);
        } else if meta.is_file() {
            out.push(Cached {
                path: e.path(),
                bytes: meta.len(),
                modified: meta.modified().ok(),
            });
        }
    }
}

/// The files a trim deletes: everything older than `max_age`, then the
/// oldest of the rest until what's left fits in `max_bytes`.
pub fn pick_victims(
    files: &[Cached],
    now: SystemTime,
    max_age: Option<Duration>,
    max_bytes: Option<u64>,
) -> Vec<PathBuf> {
    let age = |f: &Cached| {
        f.modified
            .and_then(|m| now.duration_since(m).ok())
            .unwrap_or_default()
    };
    let (mut victims, mut kept): (Vec<&Cached>, Vec<&Cached>) = files
        .iter()
        .partition(|f| max_age.is_some_and(|max| age(f) > max));
    if let Some(max) = max_bytes {
        let mut total: u64 = kept.iter().map(|f| f.bytes).sum();
        // Oldest first; a file with no time counts as new.
        kept.sort_by_key(|f| f.modified.unwrap_or(now));
        for f in kept {
            if total <= max {
                break;
            }
            victims.push(f);
            total -= f.bytes;
        }
    }
    victims.into_iter().map(|f| f.path.clone()).collect()
}

/// At launch, before anything reads the cache: applies Auto-remove cache
/// and Maximum cache size. Returns the bytes freed.
pub fn trim(keep: KeepCache, limit: Option<u64>) -> u64 {
    let max_age = keep.max_age();
    if max_age.is_none() && limit.is_none() {
        return 0;
    }
    let mut files = vec![];
    for c in CACHE {
        for dir in c.dirs() {
            walk(&dir, &mut files);
        }
    }
    let victims = pick_victims(&files, SystemTime::now(), max_age, limit);
    let mut freed = 0;
    for f in files.iter().filter(|f| victims.contains(&f.path)) {
        if std::fs::remove_file(&f.path).is_ok() {
            freed += f.bytes;
        }
    }
    freed
}

// ---- labels -----------------------------------------------------------------

/// `6.1 GB`, `203.1 MB`, `6 MB`, `0 B`: one decimal, dropped when it's .0.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".into();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    format!("{} {}", one_decimal(value), UNITS[unit])
}

/// A part's share of `total`: `94.4%`, `<0.1%`, `0%`.
pub fn format_share(part: u64, total: u64) -> String {
    if total == 0 || part == 0 {
        return "0%".into();
    }
    let percent = part as f64 / total as f64 * 100.;
    if percent < 0.1 {
        return "<0.1%".into();
    }
    format!("{}%", one_decimal(percent))
}

fn one_decimal(v: f64) -> String {
    let r = (v * 10.).round() / 10.;
    if r.fract() == 0. {
        format!("{r:.0}")
    } else {
        format!("{r:.1}")
    }
}

/// One slice of the donut, in degrees clockwise from 12 o'clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slice {
    pub start: f32,
    pub sweep: f32,
}

/// Slice angles for `values`. A non-zero value gets at least `min_sweep`
/// degrees so a sliver stays visible, the room coming out of the bigger
/// slices in proportion (unless there are too many slivers to lift).
pub fn pie_slices(values: &[u64], min_sweep: f32) -> Vec<Slice> {
    let total: u64 = values.iter().sum();
    if total == 0 {
        return values
            .iter()
            .map(|_| Slice {
                start: 0.,
                sweep: 0.,
            })
            .collect();
    }
    let raw: Vec<f32> = values
        .iter()
        .map(|&v| v as f32 / total as f32 * 360.)
        .collect();
    let small = |s: f32| s > 0. && s < min_sweep;
    let borrowed: f32 = raw
        .iter()
        .filter(|&&s| small(s))
        .map(|s| min_sweep - s)
        .sum();
    let large: f32 = raw.iter().filter(|&&s| s >= min_sweep).sum();
    let lift = large > borrowed * 2.;
    let mut start = 0.;
    raw.iter()
        .map(|&s| {
            let sweep = if !lift || s == 0. {
                s
            } else if s < min_sweep {
                min_sweep
            } else {
                s - s / large * borrowed
            };
            let slice = Slice { start, sweep };
            start += sweep;
            slice
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_labels() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1000), "1 KB");
        assert_eq!(format_bytes(6 << 20), "6 MB");
        assert_eq!(format_bytes(203 * (1 << 20) + (1 << 17)), "203.1 MB");
        assert_eq!(format_share(0, 10), "0%");
        assert_eq!(format_share(1, 10_000), "<0.1%");
        assert_eq!(format_share(944, 1000), "94.4%");
        assert_eq!(format_share(5, 10), "50%");
    }

    #[test]
    fn slivers_stay_visible() {
        let s = pie_slices(&[1000, 1, 0], 2.5);
        assert_eq!(s[1].sweep, 2.5);
        assert_eq!(s[2].sweep, 0.);
        let sum: f32 = s.iter().map(|s| s.sweep).sum();
        assert!((sum - 360.).abs() < 0.01);
        assert!(pie_slices(&[0, 0], 2.5).iter().all(|s| s.sweep == 0.));
    }

    #[test]
    fn trim_picks_old_then_oldest() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100 * 86_400);
        let day = |d: u64| Some(now - Duration::from_secs(d * 86_400));
        let f = |name: &str, bytes, modified| Cached {
            path: PathBuf::from(name),
            bytes,
            modified,
        };
        let files = [
            f("old", 10, day(40)),
            f("mid", 10, day(5)),
            f("new", 10, day(1)),
            f("unknown", 10, None),
        ];
        let week = KeepCache::Week.max_age();
        assert_eq!(
            pick_victims(&files, now, week, None),
            vec![PathBuf::from("old")]
        );
        assert_eq!(
            pick_victims(&files, now, week, Some(15)),
            vec![
                PathBuf::from("old"),
                PathBuf::from("mid"),
                PathBuf::from("new")
            ]
        );
        assert!(pick_victims(&files, now, None, None).is_empty());
    }
}
