//! An audio clip's beats (`clip.beats` in the document, the same JSON the
//! app writes): what analysis found once (`detected`, every grid level),
//! and the markers drawn on the clip and snapped to: the automatic ones
//! for the chosen detection mode plus the user's own (added, or moved
//! automatic ones, which remember where they were).
//!
//! Every time in a beat map is SOURCE-media time, so moving, trimming,
//! splitting or re-speeding the clip never rewrites it; `to_timeline` /
//! `to_source` are the only conversions. Everything here is pure, cheap
//! and undoable as one document edit. The DSP is `analyze` (the engine's
//! analyzer through the shim, or `standin` without it).

pub mod standin;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::model::{Clip, Micros, SECOND};

/// A new marker this close to another is the same beat.
pub const SAME_BEAT: Micros = 50_000;

/// Which grid levels the automatic markers show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Bars (each bar's first beat).
    Strong,
    /// Every beat.
    #[default]
    Normal,
    /// Beats and the offbeats between them that carry a hit.
    Dense,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Strong, Mode::Normal, Mode::Dense];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Strong => "Strong",
            Mode::Normal => "Normal",
            Mode::Dense => "Dense",
        }
    }

    fn shows(self, level: Level) -> bool {
        match self {
            Mode::Strong => level == Level::Bar,
            Mode::Normal => level != Level::Offbeat,
            Mode::Dense => true,
        }
    }
}

/// Where analysis placed a beat in the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Bar,
    Beat,
    Offbeat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strength {
    Weak,
    Medium,
    Strong,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Automatic,
    Manual,
    /// An automatic beat the user moved (`original_time` keeps where).
    Override,
}

/// One beat analysis found (source time).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Detected {
    #[serde(deserialize_with = "micros")]
    pub time: Micros,
    /// 0–1 against the song's typical beat.
    #[serde(default)]
    pub strength: f64,
    pub level: Level,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Marker {
    pub id: String,
    #[serde(deserialize_with = "micros")]
    pub time: Micros,
    pub source: Source,
    pub strength: Strength,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "opt_micros"
    )]
    pub original_time: Option<Micros>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeatMap {
    /// "Auto generate" on: the markers include the analysis at `mode`.
    pub auto_generate: bool,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// The analyzed song's length.
    #[serde(default, deserialize_with = "micros")]
    pub duration: Micros,
    #[serde(default)]
    pub analysis_version: String,
    /// The cached analysis: switching mode, auto generate back on and
    /// Reset re-derive the markers from it without decoding again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected: Option<Vec<Detected>>,
    #[serde(default)]
    pub markers: Vec<Marker>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn micros<'de, D: Deserializer<'de>>(d: D) -> Result<Micros, D::Error> {
    Ok(f64::deserialize(d)?.round() as Micros)
}

fn opt_micros<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Micros>, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.map(|v| v.round() as Micros))
}

// ---- analysis (the engine's wire format) ------------------------------------------------

/// What the analyzer returns: `{ version, durationMicros, bpm, confidence,
/// error?, beats: [{ timeMicros, strength, confidence, level: 0 bar | 1
/// beat | 2 offbeat, beatInBar }] }`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<f64>,
    #[serde(default)]
    pub duration_micros: f64,
    #[serde(default)]
    pub bpm: f64,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub beats: Vec<WireBeat>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireBeat {
    pub time_micros: f64,
    #[serde(default)]
    pub strength: f64,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub level: f64,
    #[serde(default)]
    pub beat_in_bar: f64,
}

impl Analysis {
    /// The `analysisVersion` a map built from this carries.
    pub fn version_string(&self) -> String {
        self.version
            .map_or_else(|| "legacy".into(), |v| format!("{}", v as i64))
    }

    fn detected(&self) -> Vec<Detected> {
        let mut out: Vec<Detected> = self
            .beats
            .iter()
            .filter(|b| b.time_micros.is_finite() && b.time_micros >= 0.)
            .map(|b| Detected {
                time: b.time_micros.round() as Micros,
                strength: (b.strength * 100.).round() / 100.,
                level: match b.level as i64 {
                    0 => Level::Bar,
                    2 => Level::Offbeat,
                    _ => Level::Beat,
                },
            })
            .collect();
        out.sort_by_key(|d| d.time);
        out
    }
}

/// The sample rate the song is decoded at for analysis.
const ANALYSIS_RATE: u32 = 44_100;
/// Songs longer than this are analyzed up to here.
const MAX_ANALYZED: Micros = 30 * 60 * SECOND;

/// Decodes `path`'s sound and finds its beats. Blocking: run it off the
/// UI thread. Err: the sound couldn't be read at all (no beats is Ok,
/// with none in it).
pub fn analyze(path: &str) -> Result<Analysis, String> {
    let mut analyzer = crate::engine::BeatAnalyzer::new(ANALYSIS_RATE);
    let mut block: Vec<f32> = Vec::with_capacity(4096);
    let read = crate::audio::stream_stereo(path, ANALYSIS_RATE, MAX_ANALYZED, |frames| {
        block.clear();
        block.extend(frames.iter().flat_map(|f| [f[0], f[1]]));
        analyzer.process(&block, 2);
    })
    .map_err(|e| format!("{e}"))?;
    if read <= 0 {
        return Err("The audio track is empty.".into());
    }
    let json = analyzer.finish(read);
    let a: Analysis = serde_json::from_str(&json).map_err(|e| format!("{e}"))?;
    match a.error.clone() {
        Some(e) => Err(e),
        None => Ok(a),
    }
}

// ---- the map's edits -----------------------------------------------------------------------

fn strength_of(d: &Detected) -> Strength {
    match d.level {
        Level::Bar => Strength::Strong,
        Level::Offbeat => Strength::Weak,
        Level::Beat if d.strength >= 0.85 => Strength::Strong,
        Level::Beat => Strength::Medium,
    }
}

/// The automatic markers for a mode: which grid levels show, not a
/// loudness threshold.
fn automatic(detected: &[Detected], mode: Mode) -> Vec<Marker> {
    detected
        .iter()
        .filter(|d| mode.shows(d.level))
        .map(|d| Marker {
            id: format!("auto-{}", d.time),
            time: d.time,
            source: Source::Automatic,
            strength: strength_of(d),
            confidence: Some(d.strength),
            original_time: None,
        })
        .collect()
}

impl BeatMap {
    /// A map to hold hand-placed beats on a clip never analyzed.
    pub fn empty(duration: Micros, mode: Mode, version: String) -> BeatMap {
        BeatMap {
            auto_generate: false,
            mode,
            bpm: None,
            confidence: None,
            duration,
            analysis_version: version,
            detected: None,
            markers: vec![],
            extra: Map::new(),
        }
    }

    /// A fresh (or re-analyzed) map that keeps `previous`'s own markers.
    pub fn from_analysis(a: &Analysis, mode: Mode, previous: Option<&BeatMap>) -> BeatMap {
        let mut map = BeatMap {
            auto_generate: true,
            mode,
            bpm: (a.bpm > 0.).then(|| (a.bpm * 10.).round() / 10.),
            confidence: Some((a.confidence * 100.).round() / 100.),
            duration: a.duration_micros.round() as Micros,
            analysis_version: a.version_string(),
            detected: Some(a.detected()),
            markers: previous.map(|p| p.markers.clone()).unwrap_or_default(),
            extra: previous.map(|p| p.extra.clone()).unwrap_or_default(),
        };
        map.rebuild();
        map
    }

    /// The user's markers kept; the automatic ones made again from
    /// `detected`, minus those a user marker already stands for.
    fn rebuild(&mut self) {
        let kept: Vec<Marker> = self
            .markers
            .drain(..)
            .filter(|m| m.source != Source::Automatic)
            .collect();
        let mut out = kept.clone();
        if let (true, Some(detected)) = (self.auto_generate, &self.detected) {
            let covered = |t: Micros| {
                kept.iter().any(|m| {
                    (m.time - t).abs() < SAME_BEAT
                        || m.original_time.is_some_and(|o| (o - t).abs() < SAME_BEAT)
                })
            };
            out.extend(
                automatic(detected, self.mode)
                    .into_iter()
                    .filter(|m| !covered(m.time)),
            );
        }
        out.sort_by_key(|m| m.time);
        self.markers = out;
    }

    fn rebuilt(mut self) -> BeatMap {
        self.rebuild();
        self
    }

    pub fn with_mode(&self, mode: Mode) -> BeatMap {
        BeatMap {
            mode,
            ..self.clone()
        }
        .rebuilt()
    }

    pub fn with_auto(&self, on: bool) -> BeatMap {
        BeatMap {
            auto_generate: on,
            ..self.clone()
        }
        .rebuilt()
    }

    /// Every edit dropped: exactly what analysis found (when auto is on).
    pub fn reset(&self) -> BeatMap {
        BeatMap {
            markers: vec![],
            ..self.clone()
        }
        .rebuilt()
    }

    /// Whether Reset would change anything.
    pub fn has_user_edits(&self) -> bool {
        self.markers.iter().any(|m| m.source != Source::Automatic) || self.reset() != *self
    }

    /// A beat at `time` (unchanged when one is already that close).
    pub fn add(&self, time: Micros, id: String) -> BeatMap {
        if self
            .markers
            .iter()
            .any(|m| (m.time - time).abs() < SAME_BEAT)
        {
            return self.clone();
        }
        let mut map = self.clone();
        let at = map.markers.partition_point(|m| m.time < time);
        map.markers.insert(
            at,
            Marker {
                id,
                time,
                source: Source::Manual,
                strength: Strength::Strong,
                confidence: Some(1.),
                original_time: None,
            },
        );
        map
    }

    pub fn remove(&self, id: &str) -> BeatMap {
        let mut map = self.clone();
        map.markers.retain(|m| m.id != id);
        map
    }

    /// Moves one beat; an automatic one becomes the user's (it never comes
    /// back from analysis at its old place).
    pub fn move_to(&self, id: &str, time: Micros) -> BeatMap {
        self.shift_where(|m| m.id == id, |_| time)
    }

    /// Moves every beat by `delta` (lining the grid up with the song by
    /// ear); beats pushed before 0 are dropped.
    pub fn shift(&self, delta: Micros) -> BeatMap {
        let mut map = self.shift_where(|_| true, |t| t + delta);
        map.markers.retain(|m| m.time >= 0);
        map
    }

    fn shift_where(
        &self,
        pick: impl Fn(&Marker) -> bool,
        to: impl Fn(Micros) -> Micros,
    ) -> BeatMap {
        let mut map = self.clone();
        for m in map.markers.iter_mut().filter(|m| pick(m)) {
            if m.source == Source::Automatic {
                m.source = Source::Override;
                m.original_time = Some(m.time);
            }
            m.time = to(m.time);
        }
        map.markers.sort_by_key(|m| m.time);
        map
    }

    /// Nothing worth keeping on the clip.
    pub fn is_empty(&self) -> bool {
        self.detected.is_none() && self.markers.is_empty()
    }
}

// ---- on the clip -----------------------------------------------------------------------------

/// The clip's beat map, if it has one.
pub fn of(clip: &Clip) -> Option<BeatMap> {
    if clip.kind != "audio" {
        return None;
    }
    serde_json::from_value(clip.extra.get("beats")?.clone()).ok()
}

/// Sets (or with None, or an empty map with auto off, removes) the
/// clip's beat map; whether anything changed.
pub fn store(clip: &mut Clip, map: Option<BeatMap>) -> bool {
    let value = map
        .filter(|m| !(m.is_empty() && !m.auto_generate))
        .and_then(|m| serde_json::to_value(m).ok());
    let before = clip.extra.get("beats").cloned();
    match value {
        Some(v) => {
            clip.extra.insert("beats".into(), v);
        }
        None => {
            clip.extra.remove("beats");
        }
    }
    clip.extra.get("beats") != before.as_ref()
}

fn speed(clip: &Clip) -> f64 {
    if clip.speed > 0. { clip.speed } else { 1. }
}

/// Timeline time of a source-time beat; None when it's trimmed out.
pub fn to_timeline(clip: &Clip, source_time: Micros) -> Option<Micros> {
    let offset = source_time - clip.source_start.unwrap_or(0);
    if offset < 0 || clip.source_duration.is_some_and(|d| offset > d) {
        return None;
    }
    let local = offset as f64 / speed(clip);
    (local <= clip.timeline_duration as f64).then(|| clip.timeline_start + local.round() as Micros)
}

/// Source time under timeline time `t`.
pub fn to_source(clip: &Clip, t: Micros) -> Micros {
    clip.source_start.unwrap_or(0)
        + ((t - clip.timeline_start) as f64 * speed(clip)).round() as Micros
}

/// The markers inside the clip's window, with their timeline times.
pub fn on_timeline<'a>(clip: &Clip, map: &'a BeatMap) -> Vec<(Micros, &'a Marker)> {
    map.markers
        .iter()
        .filter_map(|m| Some((to_timeline(clip, m.time)?, m)))
        .collect()
}

/// The marker nearest timeline time `t`, within `reach`.
pub fn nearest<'a>(
    clip: &Clip,
    map: &'a BeatMap,
    t: Micros,
    reach: Micros,
) -> Option<(Micros, &'a Marker)> {
    on_timeline(clip, map)
        .into_iter()
        .filter(|(bt, _)| (bt - t).abs() <= reach)
        .min_by_key(|(bt, _)| (bt - t).abs())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// A click track: a decaying 1 kHz blip (with a low thump on every
    /// fourth) every 60/bpm seconds, quiet hiss between.
    pub(crate) fn clicks(bpm: f64, seconds: f64, rate: u32) -> Vec<f32> {
        let n = (seconds * rate as f64) as usize;
        let period = 60. / bpm * rate as f64;
        let mut seed = 1u32;
        (0..n)
            .map(|i| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let hiss = (seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5;
                let beat = (i as f64 / period).floor();
                let since = (i as f64 - beat * period) / rate as f64;
                let env = (-since * 60.).exp() as f32;
                let t = since as f32;
                let mut s = env * (t * 1000. * std::f32::consts::TAU).sin() * 0.6;
                if beat as i64 % 4 == 0 {
                    s += env * (t * 60. * std::f32::consts::TAU).sin() * 0.6;
                }
                s + hiss * 0.01
            })
            .collect()
    }

    /// The analyzer this build uses (the engine's, or the stand-in) on a
    /// click track, through the wire format.
    #[test]
    fn the_builds_analyzer_finds_the_beat() {
        let mut a = crate::engine::BeatAnalyzer::new(44_100);
        let mono = clicks(120., 20., 44_100);
        let stereo: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();
        for block in stereo.chunks(4096) {
            a.process(block, 2);
        }
        let r: Analysis = serde_json::from_str(&a.finish(20 * SECOND)).unwrap();
        assert!(r.error.is_none());
        assert!((r.bpm - 120.).abs() < 2., "bpm {}", r.bpm);
        let map = BeatMap::from_analysis(&r, Mode::Normal, None);
        assert!(map.markers.len() >= 30, "{} beats", map.markers.len());
        assert!(map.with_mode(Mode::Strong).markers.len() < map.markers.len());
    }

    fn analysis() -> Analysis {
        let beats = (0..8)
            .flat_map(|i| {
                let t = i as f64 * 500_000.;
                let level = if i % 4 == 0 { 0. } else { 1. };
                [
                    WireBeat {
                        time_micros: t,
                        strength: if i == 1 { 0.9 } else { 0.6 },
                        level,
                        ..Default::default()
                    },
                    WireBeat {
                        time_micros: t + 250_000.,
                        strength: 0.3,
                        level: 2.,
                        ..Default::default()
                    },
                ]
            })
            .collect();
        Analysis {
            version: Some(3.),
            duration_micros: 4_000_000.,
            bpm: 120.04,
            confidence: 0.812,
            error: None,
            beats,
        }
    }

    #[test]
    fn modes_pick_grid_levels() {
        let a = analysis();
        let strong = BeatMap::from_analysis(&a, Mode::Strong, None);
        assert_eq!(strong.markers.len(), 2);
        let normal = strong.with_mode(Mode::Normal);
        assert_eq!(normal.markers.len(), 8);
        assert_eq!(normal.markers[1].strength, Strength::Strong);
        assert_eq!(normal.markers[2].strength, Strength::Medium);
        assert_eq!(normal.with_mode(Mode::Dense).markers.len(), 16);
        assert_eq!(normal.bpm, Some(120.));
        assert_eq!(normal.analysis_version, "3");
        assert!(!normal.has_user_edits());
        assert!(normal.with_auto(false).markers.is_empty());
    }

    #[test]
    fn user_beats_survive_mode_changes_and_reset_drops_them() {
        let map = BeatMap::from_analysis(&analysis(), Mode::Normal, None);
        // Moving an automatic beat makes it the user's; analysis doesn't
        // put it back at its old place.
        let moved = map.move_to("auto-500000", 600_000);
        let m = moved.markers.iter().find(|m| m.time == 600_000).unwrap();
        assert_eq!(m.source, Source::Override);
        assert_eq!(m.original_time, Some(500_000));
        let dense = moved.with_mode(Mode::Dense);
        assert!(!dense.markers.iter().any(|m| m.time == 500_000));
        assert!(dense.markers.iter().any(|m| m.time == 600_000));
        assert!(dense.has_user_edits());
        // Adding next to an existing beat is a no-op.
        assert_eq!(map.add(1_020_000, "b".into()), map);
        let added = map.add(1_300_000, "b".into());
        assert_eq!(added.markers.len(), 9);
        assert_eq!(added.remove("b"), map);
        assert_eq!(added.reset(), map);
        // A re-analysis keeps the user's markers.
        let again = BeatMap::from_analysis(&analysis(), Mode::Normal, Some(&added));
        assert!(again.markers.iter().any(|m| m.id == "b"));
    }

    #[test]
    fn shift_moves_every_beat_and_drops_those_before_zero() {
        let map = BeatMap::from_analysis(&analysis(), Mode::Normal, None);
        let later = map.shift(40_000);
        assert_eq!(later.markers.len(), 8);
        assert_eq!(later.markers[0].time, 40_000);
        // The shifted grid stands in for the old one in every mode.
        assert_eq!(later.with_mode(Mode::Normal).markers.len(), 8);
        let earlier = map.shift(-40_000);
        assert_eq!(earlier.markers.len(), 7);
        assert_eq!(earlier.reset(), map);
    }

    #[test]
    fn reads_and_writes_the_documents_json() {
        let v = json!({
            "autoGenerate": true, "mode": "dense", "bpm": 98.5, "duration": 4.0e6,
            "analysisVersion": "3", "future": 1,
            "detected": [{ "time": 1000.4, "strength": 0.5, "level": "bar" }],
            "markers": [{ "id": "a", "time": 1000, "source": "override",
                          "strength": "weak", "originalTime": 900 }]
        });
        let map: BeatMap = serde_json::from_value(v).unwrap();
        assert_eq!(map.mode, Mode::Dense);
        assert_eq!(map.detected.as_ref().unwrap()[0].time, 1000);
        assert_eq!(map.markers[0].original_time, Some(900));
        let back = serde_json::to_value(&map).unwrap();
        assert_eq!(back["future"], 1);
        assert_eq!(back["markers"][0]["originalTime"], 900);
        assert!(back["markers"][0].get("confidence").is_none());
    }

    #[test]
    fn timeline_follows_the_clip_window_and_speed() {
        let mut clip: Clip = serde_json::from_value(json!({
            "id": "a", "kind": "audio", "timelineStart": 10_000_000,
            "timelineDuration": 2_000_000, "speed": 2.0,
            "sourceStart": 1_000_000, "sourceDuration": 4_000_000
        }))
        .unwrap();
        assert_eq!(to_timeline(&clip, 3_000_000), Some(11_000_000));
        assert_eq!(to_timeline(&clip, 500_000), None);
        assert_eq!(to_timeline(&clip, 6_000_000), None);
        assert_eq!(to_source(&clip, 11_000_000), 3_000_000);

        let map = BeatMap::empty(8_000_000, Mode::Normal, "3".into()).add(3_000_000, "x".into());
        assert!(store(&mut clip, Some(map.clone())));
        assert_eq!(of(&clip), Some(map.clone()));
        assert_eq!(clip.beats().len(), 1);
        assert_eq!(nearest(&clip, &map, 11_040_000, 50_000).unwrap().1.id, "x");
        assert!(nearest(&clip, &map, 11_400_000, 50_000).is_none());
        // An emptied map with auto off leaves the clip.
        assert!(store(&mut clip, Some(map.remove("x"))));
        assert!(!clip.extra.contains_key("beats"));
    }
}
