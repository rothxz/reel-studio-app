//! Project storage: each project is one JSON file (the app's document
//! format) in the data folder's `projects/`, named by its id. Media stays
//! where it is on disk; clips point at it by absolute path.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::{Micros, Project, TrackType};
use crate::prefs::data_dir;

/// One row of the projects list.
#[derive(Clone, Debug)]
pub struct Summary {
    pub id: String,
    pub name: String,
    pub updated_at: String,
    pub duration: Micros,
    /// The first picture clip's file and source time, for the row's thumbnail.
    pub preview: Option<(String, Micros)>,
}

pub fn dir() -> PathBuf {
    let dir = data_dir().join("projects");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn file(id: &str) -> PathBuf {
    // Ids are ours ("proj-…"), but never let one step outside the folder.
    let safe: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    dir().join(format!("{safe}.json"))
}

/// Writes the project (stamping `updatedAt`). Written to a temporary file
/// first, so a crash mid-write never leaves half a project.
pub fn save(project: &mut Project) -> anyhow::Result<()> {
    project.updated_at = now_iso();
    let path = file(&project.id);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(project)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn load(id: &str) -> anyhow::Result<Project> {
    Ok(serde_json::from_str(&std::fs::read_to_string(file(id))?)?)
}

pub fn delete(id: &str) -> anyhow::Result<()> {
    std::fs::remove_file(file(id))?;
    Ok(())
}

/// Every saved project, newest edit first. Files that don't parse are
/// skipped.
pub fn list() -> Vec<Summary> {
    let Ok(entries) = std::fs::read_dir(dir()) else {
        return vec![];
    };
    let mut out: Vec<Summary> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|s| serde_json::from_str::<Project>(&s).ok())
        .map(|p| summary(&p))
        .collect();
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

pub fn summary(p: &Project) -> Summary {
    let preview = p
        .tracks
        .iter()
        .find(|t| t.kind == TrackType::Video)
        .and_then(|t| {
            t.clips
                .iter()
                .filter(|c| c.kind == "video" || c.kind == "image")
                .min_by_key(|c| c.timeline_start)
        })
        .and_then(|c| Some((c.source.clone()?, c.source_start.unwrap_or(0))));
    Summary {
        id: p.id.clone(),
        name: p.name.clone(),
        updated_at: p.updated_at.clone(),
        duration: p.duration,
        preview,
    }
}

// ---- time ------------------------------------------------------------------

fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Days since 1970-01-01 → (year, month, day), proleptic Gregorian.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn iso(secs: i64) -> String {
    let (y, m, d) = civil(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        s / 3600,
        (s / 60) % 60,
        s % 60
    )
}

/// Now as an ISO-8601 UTC timestamp, as the app writes them.
pub fn now_iso() -> String {
    iso(unix_secs())
}

/// Seconds since 1970 for "2026-11-06T17:00:00.000Z" (None if it isn't one).
fn parse_iso(s: &str) -> Option<i64> {
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    Some(days_from_civil(y, mo as u32, d as u32) * 86_400 + h * 3600 + mi * 60 + se)
}

/// "just now", "4m ago", "3h ago", "2d ago", else the date.
pub fn relative_time(iso_ts: &str) -> String {
    relative_time_at(iso_ts, unix_secs())
}

use crate::i18n::tk;

fn relative_time_at(iso_ts: &str, now: i64) -> String {
    let Some(then) = parse_iso(iso_ts) else {
        return String::new();
    };
    let diff = (now - then).max(0);
    match diff {
        d if d < 60 => tk("time.justNow", "just now", &[]),
        d if d < 3600 => tk("time.minutesAgo", "{count}m ago", &[("count", &(d / 60))]),
        d if d < 86_400 => tk("time.hoursAgo", "{count}h ago", &[("count", &(d / 3600))]),
        d if d < 7 * 86_400 => tk("time.daysAgo", "{count}d ago", &[("count", &(d / 86_400))]),
        _ => iso_ts.get(0..10).unwrap_or_default().to_string(),
    }
}

/// A running time as "m:ss" (or "h:mm:ss").
pub fn clock(t: Micros) -> String {
    let s = t.max(0) / 1_000_000;
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_round_trips() {
        for secs in [0, 951_782_400, 1_793_984_400, 1_700_000_123] {
            assert_eq!(parse_iso(&iso(secs)), Some(secs));
        }
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn relative_labels() {
        let t = "2026-11-06T12:00:00.000Z";
        let base = parse_iso(t).unwrap();
        assert_eq!(relative_time_at(t, base + 30), "just now");
        assert_eq!(relative_time_at(t, base + 4 * 60), "4m ago");
        assert_eq!(relative_time_at(t, base + 3 * 3600), "3h ago");
        assert_eq!(relative_time_at(t, base + 2 * 86_400), "2d ago");
        assert_eq!(relative_time_at(t, base + 30 * 86_400), "2026-11-06");
    }

    #[test]
    fn clock_format() {
        assert_eq!(clock(65_000_000), "1:05");
        assert_eq!(clock(3_725_000_000), "1:02:05");
    }
}
