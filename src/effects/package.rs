//! Effect and filter packages: each look's shader as data, downloaded from
//! the asset host on first use (`effect-packages/<key>`) and kept on disk.
//!
//! A package is a small JSON object: `format`, and `body`, a function body
//! in the effect dialect (GLSL-like: `vec2`/`vec3`, `S(uv)` samples the
//! frame, `MOD(` / `ATAN2(` for the two calls that differ per shading
//! language). An effect also says how it combines with the picture: a
//! `distortion` replaces the frame's colour (it may resample it anywhere), a
//! `blend` effect is laid over it (`plus`, `screen` or `multiply`). A
//! filter's body grades one pixel (`c`, at `uv`) and is mixed in by its
//! intensity.
//!
//! Keys end in the FNV-1a hash of the exact text (`…-1a2b3c4d.json`), so a
//! download or a cached copy is accepted only if it's exactly the package
//! the key names, and a new version is a new key.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, bail};
use serde_json::Value;

use crate::assets;

/// The newest package format this app runs.
const FORMAT: i64 = 1;

/// How an effect combines with the picture under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EffectKind {
    Distortion,
    Plus,
    Screen,
    Multiply,
}

/// A parsed effect package.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct EffectPackage {
    pub kind: EffectKind,
    pub body: String,
}

/// FNV-1a (32-bit) over the text's UTF-16 code units, as 8 hex digits: the
/// hash the publisher puts in each key.
pub fn package_hash(text: &str) -> String {
    let mut h: u32 = 0x811c9dc5;
    for unit in text.encode_utf16() {
        h ^= unit as u32;
        h = h.wrapping_mul(0x01000193);
    }
    format!("{h:08x}")
}

/// `effects/old-tv/v1-1a2b3c4d.json` → `1a2b3c4d`.
fn hash_of_key(key: &str) -> Option<&str> {
    let stem = key.strip_suffix(".json")?;
    let (_, hash) = stem.rsplit_once('-')?;
    (hash.len() == 8 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hash)
}

fn object(text: &str) -> Result<serde_json::Map<String, Value>> {
    let Value::Object(o) = serde_json::from_str(text)? else {
        bail!("not an object");
    };
    if o.get("format").and_then(Value::as_i64) != Some(FORMAT) {
        bail!("unknown package format");
    }
    Ok(o)
}

fn body(o: &serde_json::Map<String, Value>) -> Result<String> {
    match o.get("body").and_then(Value::as_str) {
        Some(b) => Ok(b.to_string()),
        None => bail!("no body"),
    }
}

pub fn parse_effect(text: &str) -> Result<EffectPackage> {
    let o = object(text)?;
    let kind = match (
        o.get("kind").and_then(Value::as_str),
        o.get("blendMode").and_then(Value::as_str),
    ) {
        (Some("distortion"), _) => EffectKind::Distortion,
        (Some("blend"), Some("plus") | None) => EffectKind::Plus,
        (Some("blend"), Some("screen")) => EffectKind::Screen,
        (Some("blend"), Some("multiply")) => EffectKind::Multiply,
        (kind, mode) => bail!("unknown effect kind {kind:?} / {mode:?}"),
    };
    Ok(EffectPackage {
        kind,
        body: body(&o)?,
    })
}

pub fn parse_filter(text: &str) -> Result<String> {
    body(&object(text)?)
}

/// A transition's reveal package: like a filter's, only a body.
pub fn parse_reveal(text: &str) -> Result<String> {
    body(&object(text)?)
}

/// Downloads (or reads the cached copy of) `key` and checks it is exactly
/// the package the key names. Blocking: run it off the UI thread.
pub fn fetch_text(key: &str) -> Result<String> {
    let bytes = assets::fetch(assets::EFFECT_PACKAGES, key)?;
    let text = String::from_utf8(bytes)?;
    if hash_of_key(key).is_some_and(|h| h != package_hash(&text)) {
        bail!("{key}: content doesn't match its key");
    }
    Ok(text)
}

/// What a lookup found.
pub enum Lookup<T> {
    Ready(T),
    /// Not here yet: being fetched, or a fetch should start.
    Missing,
    /// It couldn't be had (offline, bad package): drawn without it.
    Failed,
}

/// The packages this session has, by key.
#[derive(Clone, Default)]
pub struct Packages {
    effects: HashMap<String, Arc<EffectPackage>>,
    filters: HashMap<String, Arc<str>>,
    reveals: HashMap<String, Arc<str>>,
    pending: HashSet<String>,
    failed: HashSet<String>,
}

/// A fetched package on its way to the store.
pub enum Fetched {
    Effect(String, Result<EffectPackage>),
    Filter(String, Result<String>),
    Reveal(String, Result<String>),
}

/// Which kind of package a key holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageKind {
    Effect,
    Filter,
    Reveal,
}

impl Packages {
    pub fn effect(&self, key: &str) -> Lookup<Arc<EffectPackage>> {
        match self.effects.get(key) {
            Some(p) => Lookup::Ready(p.clone()),
            None if self.failed.contains(key) => Lookup::Failed,
            None => Lookup::Missing,
        }
    }

    pub fn filter(&self, key: &str) -> Lookup<Arc<str>> {
        match self.filters.get(key) {
            Some(p) => Lookup::Ready(p.clone()),
            None if self.failed.contains(key) => Lookup::Failed,
            None => Lookup::Missing,
        }
    }

    pub fn reveal(&self, key: &str) -> Lookup<Arc<str>> {
        match self.reveals.get(key) {
            Some(p) => Lookup::Ready(p.clone()),
            None if self.failed.contains(key) => Lookup::Failed,
            None => Lookup::Missing,
        }
    }

    /// Marks `key` as on its way; false when it already is (or failed).
    pub fn begin(&mut self, key: &str) -> bool {
        !self.failed.contains(key) && self.pending.insert(key.to_string())
    }

    pub fn end(&mut self, fetched: Fetched) {
        let (key, error) = match fetched {
            Fetched::Effect(key, Ok(p)) => {
                self.effects.insert(key.clone(), Arc::new(p));
                (key, None)
            }
            Fetched::Filter(key, Ok(body)) => {
                self.filters.insert(key.clone(), body.into());
                (key, None)
            }
            Fetched::Reveal(key, Ok(body)) => {
                self.reveals.insert(key.clone(), body.into());
                (key, None)
            }
            Fetched::Effect(key, Err(e))
            | Fetched::Filter(key, Err(e))
            | Fetched::Reveal(key, Err(e)) => (key, Some(e)),
        };
        self.pending.remove(&key);
        if let Some(e) = error {
            eprintln!("effect package {key}: {e:#}");
            self.failed.insert(key);
        }
    }

    /// Forget failures (a retry once back online).
    pub fn retry_failed(&mut self) {
        self.failed.clear();
    }
}

/// Fetches one package (blocking).
pub fn fetch(key: String, kind: PackageKind) -> Fetched {
    let text = fetch_text(&key);
    match kind {
        PackageKind::Filter => Fetched::Filter(key, text.and_then(|t| parse_filter(&t))),
        PackageKind::Reveal => Fetched::Reveal(key, text.and_then(|t| parse_reveal(&t))),
        PackageKind::Effect => Fetched::Effect(key, text.and_then(|t| parse_effect(&t))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_like_the_publisher() {
        // FNV-1a test vectors.
        assert_eq!(package_hash(""), "811c9dc5");
        assert_eq!(package_hash("a"), "e40c292c");
        assert_eq!(package_hash("foobar"), "bf9cf968");
        assert_eq!(
            hash_of_key("effects/edge-glow/v1-059cab27.json"),
            Some("059cab27")
        );
        assert_eq!(
            hash_of_key("filters/milk-tea-c3d94bac.json"),
            Some("c3d94bac")
        );
        assert_eq!(hash_of_key("filters/plain.json"), None);
    }

    #[test]
    fn parses_packages() {
        let d = parse_effect(r#"{"format":1,"kind":"distortion","body":"return S(uv);"}"#).unwrap();
        assert_eq!(d.kind, EffectKind::Distortion);
        let s =
            parse_effect(r#"{"format":1,"kind":"blend","blendMode":"screen","body":"x"}"#).unwrap();
        assert_eq!(s.kind, EffectKind::Screen);
        assert!(parse_effect(r#"{"format":2,"kind":"distortion","body":"x"}"#).is_err());
        assert!(
            parse_effect(r#"{"format":1,"kind":"blend","blendMode":"overlay","body":"x"}"#)
                .is_err()
        );
        assert_eq!(
            parse_filter(r#"{"format":1,"body":"return c;"}"#).unwrap(),
            "return c;"
        );
    }
}
