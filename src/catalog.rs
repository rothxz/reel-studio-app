//! What the browser's Effects and Filter tabs list.
//!
//! **Video effects** are served: a table in the app's catalog database
//! holds each effect's tab ranks, mood category, sliders and tile key, so
//! new, fixed or withdrawn effects reach the app without a build. Nothing
//! is fetched up front: a tab asks for `PAGE_SIZE` rows at a time as it's
//! shown and scrolled. Every row seen is kept on disk
//! (`<data>/effect-catalog.json`), so a tab that can't be fetched (offline)
//! lists what this machine has seen.
//!
//! **Filters** are a fixed list compiled in from the app checkout at build
//! time (build.rs), with each look's tile key.
//!
//! Tile pictures of both come from the asset host (`assets.rs`).

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Instant;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::assets;

/// A clip effect's type prefix for a video effect / a filter.
pub const EFFECT_PREFIX: &str = "Overlays:";
pub const FILTER_PREFIX: &str = "Filters:";

/// A freshly picked filter starts below full strength.
pub const DEFAULT_FILTER_INTENSITY: f64 = 80.;

/// Rows per request: about two screens of tiles.
pub const PAGE_SIZE: usize = 30;
/// Lists fetched longer ago start over when the Effects tab is opened.
const STALE_AFTER_SECS: u64 = 10 * 60;
/// The newest effect package format this app understands.
const PACKAGE_FORMAT: i64 = 1;

const COLUMNS: &str = "name,kind,category,sort_order,trending_rank,classic_rank,params,phase,version,format,package_key,preview_key,enabled";

/// The Effects tab's categories (key, label), in order.
pub const EFFECT_TABS: &[(&str, &str)] = &[
    ("all", "All"),
    ("trending", "Trending"),
    ("classic", "Classic"),
    ("light", "Light"),
    ("weather", "Weather & Grain"),
    ("blur", "Blur & Glitch"),
    ("glitch", "Glitch"),
    ("warp", "Warp & Mirror"),
    ("motion", "Shake & Zoom"),
    ("color", "Color & Tone"),
    ("retro", "Retro"),
    ("party", "Party & Sparkle"),
];

/// The Filter tab's categories (key, label), in order.
pub const FILTER_TABS: &[(&str, &str)] = &[
    ("all", "All"),
    ("featured", "Featured"),
    ("portrait", "Portrait"),
    ("landscape", "Landscape"),
    ("food", "Food"),
    ("film", "Film"),
    ("cinematic", "Cinematic"),
    ("retro", "Retro"),
    ("night", "Night"),
    ("style", "Style"),
    ("mono", "B&W"),
    ("classic", "Classic"),
];

/// One slider of an effect (0–100).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ParamSpec {
    pub id: String,
    pub label: String,
    pub def: f64,
}

/// One served video effect (a row of the effects table).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EffectEntry {
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub sort_order: i64,
    #[serde(default)]
    pub trending_rank: Option<i64>,
    #[serde(default)]
    pub classic_rank: Option<i64>,
    /// None: the default Intensity slider; empty: nothing to adjust.
    #[serde(default)]
    pub params: Option<Vec<ParamSpec>>,
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub version: i64,
    #[serde(default)]
    pub format: i64,
    #[serde(default)]
    pub package_key: String,
    #[serde(default)]
    pub preview_key: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

impl EffectEntry {
    /// Its sliders (an effect that declares none gets Intensity).
    pub fn params(&self) -> Vec<ParamSpec> {
        self.params.clone().unwrap_or_else(default_effect_params)
    }

    fn in_tab(&self, tab: &str) -> bool {
        self.enabled
            && match tab {
                "all" => true,
                "trending" => self.trending_rank.is_some(),
                "classic" => self.classic_rank.is_some(),
                cat => self.category == cat,
            }
    }

    fn rank(&self, tab: &str) -> i64 {
        match tab {
            "trending" => self.trending_rank.unwrap_or(i64::MAX),
            "classic" => self.classic_rank.unwrap_or(i64::MAX),
            _ => self.sort_order,
        }
    }
}

/// The one slider of an effect that declares none.
pub fn default_effect_params() -> Vec<ParamSpec> {
    vec![ParamSpec {
        id: "intensity".into(),
        label: "Intensity".into(),
        def: 42.,
    }]
}

/// A filter's one slider.
pub fn filter_params() -> Vec<ParamSpec> {
    vec![ParamSpec {
        id: "intensity".into(),
        label: "Intensity".into(),
        def: DEFAULT_FILTER_INTENSITY,
    }]
}

/// The parameters a freshly picked effect starts with: every slider at its
/// default, and always an intensity (100 when it has no such slider: the
/// renderers treat 0 as off).
pub fn default_parameters(params: &[ParamSpec]) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = params.iter().map(|p| (p.id.clone(), p.def)).collect();
    if !out.iter().any(|(id, _)| id == "intensity") {
        out.push(("intensity".into(), 100.));
    }
    out
}

// ---- the served table ----------------------------------------------------

/// The catalog database's REST endpoint + publishable key, if configured.
fn database() -> Option<(String, String)> {
    let url = assets::setting("REEL_CATALOG_URL", env!("REEL_CATALOG_URL"))?;
    let key = assets::setting("REEL_CATALOG_KEY", env!("REEL_CATALOG_KEY"))?;
    Some((url.trim_end_matches('/').to_string(), key))
}

/// Escapes a value for a PostgREST filter inside a URL query.
fn query_value(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The request for a tab's rows from `offset`.
fn page_query(tab: &str, offset: usize) -> String {
    let mut q = format!("select={COLUMNS}&enabled=eq.true&format=lte.{PACKAGE_FORMAT}");
    let order = match tab {
        "trending" => {
            q.push_str("&trending_rank=not.is.null");
            "trending_rank"
        }
        "classic" => {
            q.push_str("&classic_rank=not.is.null");
            "classic_rank"
        }
        "all" => "sort_order",
        cat => {
            q.push_str(&format!("&category=eq.{}", query_value(cat)));
            "sort_order"
        }
    };
    q.push_str(&format!("&order={order}&offset={offset}&limit={PAGE_SIZE}"));
    q
}

fn get_rows(query: &str) -> Result<Vec<EffectEntry>> {
    let Some((base, key)) = database() else {
        bail!("no catalog database configured");
    };
    let url = format!("{base}/rest/v1/effects?{query}");
    let body = assets::agent()
        .get(&url)
        .header("apikey", &key)
        .header("Authorization", &format!("Bearer {key}"))
        .call()?
        .body_mut()
        .read_to_string()?;
    let rows: Vec<EffectEntry> = serde_json::from_str(&body)?;
    Ok(rows
        .into_iter()
        .filter(|r| r.format <= PACKAGE_FORMAT && !r.name.is_empty())
        .collect())
}

/// Fetches one page of a tab (blocking: run it off the UI thread).
pub fn fetch_page(tab: &str, offset: usize) -> Result<Vec<EffectEntry>> {
    get_rows(&page_query(tab, offset))
}

/// Fetches the rows of these effects (a project's), withdrawn ones too.
pub fn fetch_named(names: &[String]) -> Result<Vec<EffectEntry>> {
    let list: Vec<String> = names
        .iter()
        .map(|n| format!("\"{}\"", n.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    get_rows(&format!(
        "select={COLUMNS}&name=in.({})",
        query_value(&list.join(","))
    ))
}

/// Every enabled effect's row (the shader check test).
#[cfg(test)]
pub fn fetch_all_effects() -> Result<Vec<EffectEntry>> {
    get_rows(&format!(
        "select={COLUMNS}&enabled=eq.true&format=lte.{PACKAGE_FORMAT}&order=sort_order"
    ))
}

/// One tab's list as fetched so far.
#[derive(Clone, Debug, Default)]
pub struct Page {
    pub names: Vec<String>,
    /// Rows fetched (the next request's offset).
    pub offset: usize,
    /// Every row is in (or the list is the offline copy).
    pub done: bool,
    pub loading: bool,
    /// The first request failed: `names` is what this machine has seen.
    pub offline: bool,
    pub fetched_at: Option<Instant>,
}

/// The effects catalog as the editor holds it.
#[derive(Default)]
pub struct EffectCatalog {
    pub entries: HashMap<String, EffectEntry>,
    pub pages: HashMap<String, Page>,
}

fn cache_file() -> std::path::PathBuf {
    crate::prefs::data_dir().join("effect-catalog.json")
}

impl EffectCatalog {
    /// Starts from the rows this machine has seen.
    pub fn load() -> Self {
        let entries = std::fs::read_to_string(cache_file())
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<EffectEntry>>(&s).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.name.clone(), e))
            .collect();
        EffectCatalog {
            entries,
            pages: HashMap::new(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&EffectEntry> {
        self.entries.get(name)
    }

    pub fn page(&self, tab: &str) -> Option<&Page> {
        self.pages.get(tab)
    }

    /// Fetched rows join the catalog (and its disk copy).
    pub fn remember(&mut self, rows: &[EffectEntry]) {
        if rows.is_empty() {
            return;
        }
        for r in rows {
            self.entries.insert(r.name.clone(), r.clone());
        }
        if cfg!(test) {
            return;
        }
        let mut all: Vec<&EffectEntry> = self.entries.values().collect();
        all.sort_by_key(|e| e.sort_order);
        if let Ok(json) = serde_json::to_string(&all) {
            let tmp = cache_file().with_extension("json.part");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(tmp, cache_file());
            }
        }
    }

    /// The next request for `tab`, marking it loading; None when it's
    /// complete or already in flight.
    pub fn begin_page(&mut self, tab: &str) -> Option<usize> {
        let page = self.pages.entry(tab.to_string()).or_default();
        if page.done || page.loading {
            return None;
        }
        page.loading = true;
        Some(page.offset)
    }

    /// A page came back (`Err`: it couldn't be fetched).
    pub fn end_page(&mut self, tab: &str, offset: usize, rows: Result<Vec<EffectEntry>>) {
        match rows {
            Ok(rows) => {
                self.remember(&rows);
                let page = self.pages.entry(tab.to_string()).or_default();
                if page.offset != offset {
                    // Reset meanwhile (stale lists start over).
                    page.loading = false;
                    return;
                }
                for r in &rows {
                    if !page.names.contains(&r.name) {
                        page.names.push(r.name.clone());
                    }
                }
                page.offset += rows.len();
                page.done = rows.len() < PAGE_SIZE;
                page.loading = false;
                page.offline = false;
                if offset == 0 {
                    page.fetched_at = Some(Instant::now());
                }
            }
            Err(e) => {
                eprintln!("effects tab {tab}: {e:#}");
                let seen = self.seen_in(tab);
                let page = self.pages.entry(tab.to_string()).or_default();
                page.loading = false;
                if offset == 0 {
                    // What this machine has seen, until a retry.
                    page.names = seen;
                    page.done = true;
                    page.offline = true;
                }
            }
        }
    }

    /// A tab's effects among the rows seen, in the tab's order.
    fn seen_in(&self, tab: &str) -> Vec<String> {
        let mut list: Vec<&EffectEntry> = self.entries.values().filter(|e| e.in_tab(tab)).collect();
        list.sort_by_key(|e| (e.rank(tab), e.sort_order));
        list.into_iter().map(|e| e.name.clone()).collect()
    }

    /// Lists fetched long enough ago (or offline ones) start over.
    pub fn refresh_stale(&mut self) {
        let stale = self.pages.values().any(|p| {
            !p.loading
                && (p.offline
                    || p.fetched_at
                        .is_some_and(|t| t.elapsed().as_secs() > STALE_AFTER_SECS))
        });
        if stale {
            self.pages.retain(|_, p| p.loading);
        }
    }

    /// Forget a tab's list (Retry).
    pub fn reset(&mut self, tab: &str) {
        if self.pages.get(tab).is_some_and(|p| !p.loading) {
            self.pages.remove(tab);
        }
    }
}

// ---- filters (compiled in) ---------------------------------------------

/// One filter look.
#[derive(Clone, Debug, Deserialize)]
pub struct FilterMeta {
    pub name: String,
    pub cat: String,
    /// A Classic look: a colour matrix built into the engine, no package.
    #[serde(default)]
    pub builtin: bool,
}

static FILTERS: LazyLock<Vec<FilterMeta>> = LazyLock::new(|| {
    serde_json::from_str(include_str!(concat!(
        env!("OUT_DIR"),
        "/filter_catalog.json"
    )))
    .unwrap_or_else(|e| {
        eprintln!("filter catalog: {e}");
        vec![]
    })
});

#[derive(Deserialize)]
struct PreviewManifest {
    #[serde(default)]
    filters: HashMap<String, PreviewItem>,
}

#[derive(Deserialize)]
struct PreviewItem {
    image: String,
}

static FILTER_PREVIEWS: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str::<PreviewManifest>(include_str!(concat!(
        env!("OUT_DIR"),
        "/filter_previews.json"
    )))
    .map(|m| m.filters.into_iter().map(|(k, v)| (k, v.image)).collect())
    .unwrap_or_default()
});

/// The filters of a tab ("all": every one), in catalog order.
pub fn filters_in(tab: &str) -> Vec<&'static FilterMeta> {
    FILTERS
        .iter()
        .filter(|f| tab == "all" || f.cat == tab)
        .collect()
}

pub fn has_filters() -> bool {
    !FILTERS.is_empty()
}

/// A filter's tile key in the effect-previews bucket.
pub fn filter_preview_key(name: &str) -> Option<&'static str> {
    FILTER_PREVIEWS.get(name).map(String::as_str)
}

static FILTER_PACKAGES: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!(concat!(
        env!("OUT_DIR"),
        "/filter_packages.json"
    )))
    .unwrap_or_default()
});

/// A Classic filter: the engine's colour matrix, no package.
pub fn is_classic_filter(name: &str) -> bool {
    FILTERS.iter().any(|f| f.name == name && f.builtin)
}

/// A packaged filter's shader key in the effect-packages bucket.
pub fn filter_package_key(name: &str) -> Option<&'static str> {
    FILTER_PACKAGES.get(name).map(String::as_str)
}

/// Every packaged filter (name, package key): the shader check test.
#[cfg(test)]
pub fn filter_packages() -> Vec<(String, String)> {
    FILTER_PACKAGES
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, cat: &str, order: i64, trending: Option<i64>) -> EffectEntry {
        EffectEntry {
            name: name.into(),
            kind: "blend".into(),
            category: cat.into(),
            sort_order: order,
            trending_rank: trending,
            classic_rank: None,
            params: None,
            phase: None,
            version: 1,
            format: 1,
            package_key: String::new(),
            preview_key: None,
            enabled: true,
        }
    }

    #[test]
    fn tab_queries() {
        assert!(page_query("trending", 30).contains("trending_rank=not.is.null"));
        assert!(page_query("trending", 30).contains("order=trending_rank&offset=30&limit=30"));
        assert!(page_query("glitch", 0).contains("category=eq.glitch&order=sort_order"));
        assert!(!page_query("all", 0).contains("category=eq"));
    }

    #[test]
    fn offline_tab_lists_what_was_seen_in_order() {
        let mut c = EffectCatalog::default();
        c.entries
            .insert("B".into(), entry("B", "glitch", 2, Some(0)));
        c.entries.insert("A".into(), entry("A", "glitch", 1, None));
        c.entries
            .insert("C".into(), entry("C", "light", 0, Some(1)));
        assert_eq!(c.begin_page("glitch"), Some(0));
        c.end_page("glitch", 0, Err(anyhow::anyhow!("offline")));
        let p = c.page("glitch").unwrap();
        assert_eq!(p.names, ["A", "B"]);
        assert!(p.offline && p.done && !p.loading);
        assert_eq!(c.seen_in("trending"), ["B", "C"]);
        // Opening the tab again retries.
        c.refresh_stale();
        assert!(c.page("glitch").is_none());
    }

    #[test]
    fn pages_append_until_short() {
        let mut c = EffectCatalog::default();
        let rows: Vec<EffectEntry> = (0..PAGE_SIZE as i64)
            .map(|i| entry(&format!("E{i}"), "glow", i, None))
            .collect();
        let off = c.begin_page("all").unwrap();
        assert_eq!(c.begin_page("all"), None, "in flight");
        c.end_page("all", off, Ok(rows));
        assert_eq!(c.page("all").unwrap().names.len(), PAGE_SIZE);
        let off = c.begin_page("all").unwrap();
        assert_eq!(off, PAGE_SIZE);
        c.end_page("all", off, Ok(vec![entry("Last", "glow", 99, None)]));
        let p = c.page("all").unwrap();
        assert!(p.done);
        assert_eq!(p.names.last().unwrap(), "Last");
    }

    /// With an app checkout, the compiled-in list parses (it's empty
    /// without one).
    #[test]
    fn compiled_in_filters_parse() {
        let raw = include_str!(concat!(env!("OUT_DIR"), "/filter_catalog.json"));
        let parsed: Vec<FilterMeta> = serde_json::from_str(raw).expect("filter list is JSON");
        assert_eq!(parsed.len(), FILTERS.len());
        if has_filters() {
            assert!(
                filters_in("classic")
                    .iter()
                    .all(|f| is_classic_filter(&f.name))
            );
            let packaged = filters_in("featured");
            assert!(
                packaged
                    .iter()
                    .all(|f| filter_package_key(&f.name).is_some())
            );
        }
    }

    #[test]
    fn default_parameters_always_have_intensity() {
        let p = default_parameters(&[ParamSpec {
            id: "speed".into(),
            label: "Speed".into(),
            def: 50.,
        }]);
        assert_eq!(p, [("speed".into(), 50.), ("intensity".into(), 100.)]);
        assert_eq!(
            default_parameters(&default_effect_params()),
            [("intensity".into(), 42.)]
        );
    }
}
