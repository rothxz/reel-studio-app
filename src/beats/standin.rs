//! A small beat tracker of our own, for builds without the C++ engine
//! (whose analyzer is the real one): the same streaming surface and JSON
//! result, so the rest of the app can't tell them apart.
//!
//! The method is the textbook one. Loudness per ~11.6 ms frame (whole band
//! and a low band for the kick) → onset strength as the rise in log
//! loudness, minus its local mean → tempo from the onset curve's
//! autocorrelation (leaning toward 120 BPM to settle half / double
//! tempo) → beats by dynamic programming (each beat is an onset, about a
//! period after the last) → bars where the low band hits hardest every
//! fourth beat, offbeats where a real hit falls halfway.

#![cfg_attr(reel_cpp_engine, allow(dead_code))]

use super::{Analysis, WireBeat};
use crate::model::{Micros, SECOND};

/// Results from this analyzer carry this version, which no engine build
/// uses: an engine build re-analyzes them.
const VERSION: f64 = 0.;
/// Onset frames per second (a 512-sample hop at 44.1 kHz).
const FRAME_RATE: f64 = 86.;
const MIN_BPM: f64 = 60.;
const MAX_BPM: f64 = 200.;
const PREFERRED_BPM: f64 = 120.;
const LOW_BAND_HZ: f64 = 150.;
const BEATS_PER_BAR: usize = 4;
/// Below this the song has no steady beat: no markers rather than a guess.
const MIN_CONFIDENCE: f64 = 0.05;

pub struct Analyzer {
    rate: f64,
    hop: usize,
    /// One-pole low-pass state and coefficient (the kick band).
    low: f64,
    low_k: f64,
    /// The current frame's sums of squares and its sample count.
    sum: f64,
    low_sum: f64,
    count: usize,
    loud: Vec<f32>,
    low_loud: Vec<f32>,
}

impl Analyzer {
    /// The `analysisVersion` its results carry.
    pub fn version() -> String {
        format!("{}", VERSION as i64)
    }

    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f64;
        Analyzer {
            rate,
            hop: (rate / FRAME_RATE).round().max(1.) as usize,
            low: 0.,
            low_k: 1. - (-2. * std::f64::consts::PI * LOW_BAND_HZ / rate).exp(),
            sum: 0.,
            low_sum: 0.,
            count: 0,
            loud: vec![],
            low_loud: vec![],
        }
    }

    /// Interleaved samples, `channels` per frame.
    pub fn process(&mut self, interleaved: &[f32], channels: usize) {
        let channels = channels.max(1);
        for frame in interleaved.chunks_exact(channels) {
            let m = frame.iter().map(|&s| s as f64).sum::<f64>() / channels as f64;
            self.low += self.low_k * (m - self.low);
            self.sum += m * m;
            self.low_sum += self.low * self.low;
            self.count += 1;
            if self.count == self.hop {
                let n = self.count as f64;
                self.loud.push((self.sum / n).sqrt() as f32);
                self.low_loud.push((self.low_sum / n).sqrt() as f32);
                (self.sum, self.low_sum, self.count) = (0., 0., 0);
            }
        }
    }

    pub fn finish(self, duration: Micros) -> String {
        let analysis = self.analyze(duration);
        serde_json::to_string(&analysis).unwrap_or_default()
    }

    fn analyze(&self, duration: Micros) -> Analysis {
        let mut out = Analysis {
            version: Some(VERSION),
            duration_micros: duration as f64,
            ..Default::default()
        };
        let fr = self.rate / self.hop as f64;
        let n = self.loud.len();
        if n < (fr * 3.) as usize {
            return out; // too short to have a tempo
        }
        let full = rise(&self.loud);
        let low = rise(&self.low_loud);
        let onset = normalize(
            &full
                .iter()
                .zip(&low)
                .map(|(a, b)| a + b)
                .collect::<Vec<_>>(),
            fr,
        );
        let Some((period, confidence)) = tempo(&onset, fr) else {
            return out;
        };
        out.confidence = confidence;
        if confidence < MIN_CONFIDENCE {
            return out;
        }
        let mut beats = track(&onset, period);
        // Nothing in the quiet: no beats over silence (intros, endings).
        let mut sorted = self.loud.clone();
        sorted.sort_by(f32::total_cmp);
        let floor = sorted[sorted.len() / 2] * 0.05;
        beats.retain(|&k| self.loud[k.min(n - 1)] > floor);
        if beats.len() < 4 {
            return out;
        }
        let mut gaps: Vec<usize> = beats.windows(2).map(|w| w[1] - w[0]).collect();
        gaps.sort_unstable();
        out.bpm = 60. * fr / gaps[gaps.len() / 2] as f64;

        let mut hits: Vec<f32> = beats.iter().map(|&k| peak(&onset, k, 1)).collect();
        hits.sort_by(f32::total_cmp);
        let typical = hits[hits.len() / 2].max(1e-6) as f64;
        let strength = |v: f32| (v as f64 / (typical * 1.5)).clamp(0., 1.);
        // Bars: the phase (of four) where the low band lands hardest.
        let low_norm = normalize(&low, fr);
        let bar_phase = (0..BEATS_PER_BAR)
            .max_by(|&a, &b| {
                let sum = |p: usize| -> f32 {
                    beats
                        .iter()
                        .skip(p)
                        .step_by(BEATS_PER_BAR)
                        .map(|&k| peak(&low_norm, k, 1))
                        .sum()
                };
                sum(a).total_cmp(&sum(b))
            })
            .unwrap_or(0);
        let time = |k: usize| (k as f64 / fr * SECOND as f64).round();
        for (i, &k) in beats.iter().enumerate() {
            let in_bar = (i + BEATS_PER_BAR - bar_phase % BEATS_PER_BAR) % BEATS_PER_BAR;
            out.beats.push(WireBeat {
                time_micros: time(k),
                strength: strength(peak(&onset, k, 1)),
                confidence,
                level: if in_bar == 0 { 0. } else { 1. },
                beat_in_bar: in_bar as f64 + 1.,
            });
            // An offbeat where a real hit falls between this beat and the next.
            if let Some(&next) = beats.get(i + 1) {
                let mid = (k + next) / 2;
                let v = peak(&onset, mid, 1);
                if v as f64 >= typical * 0.5 {
                    out.beats.push(WireBeat {
                        time_micros: time(mid),
                        strength: strength(v),
                        confidence,
                        level: 2.,
                        beat_in_bar: 0.,
                    });
                }
            }
        }
        out
    }
}

/// How much louder each frame is than the last (log scale, rises only).
fn rise(loud: &[f32]) -> Vec<f32> {
    let log = |v: f32| (1. + 1000. * v).ln();
    let mut out = vec![0.; loud.len()];
    for k in 1..loud.len() {
        out[k] = (log(loud[k]) - log(loud[k - 1])).max(0.);
    }
    out
}

/// The onset curve minus its local mean (±¼ s), rises only, scaled to unit
/// standard deviation.
fn normalize(onset: &[f32], fr: f64) -> Vec<f32> {
    let w = (fr / 4.).round() as usize;
    let mut prefix = vec![0f64; onset.len() + 1];
    for (k, &v) in onset.iter().enumerate() {
        prefix[k + 1] = prefix[k] + v as f64;
    }
    let out: Vec<f32> = (0..onset.len())
        .map(|k| {
            let (a, b) = (k.saturating_sub(w), (k + w + 1).min(onset.len()));
            let mean = (prefix[b] - prefix[a]) / (b - a) as f64;
            (onset[k] as f64 - mean).max(0.) as f32
        })
        .collect();
    let n = out.len().max(1) as f64;
    let mean = out.iter().map(|&v| v as f64).sum::<f64>() / n;
    let var = out.iter().map(|&v| (v as f64 - mean).powi(2)).sum::<f64>() / n;
    let sd = var.sqrt().max(1e-9) as f32;
    out.into_iter().map(|v| v / sd).collect()
}

/// The beat period in frames (fractional) and how periodic the curve is
/// (0–1), from its autocorrelation over the tempo range.
fn tempo(onset: &[f32], fr: f64) -> Option<(f64, f64)> {
    let n = onset.len();
    let ac = |lag: usize| -> f64 {
        if lag >= n {
            return 0.;
        }
        let s: f64 = onset[..n - lag]
            .iter()
            .zip(&onset[lag..])
            .map(|(a, b)| (a * b) as f64)
            .sum();
        s / (n - lag) as f64
    };
    let zero = ac(0);
    if zero <= 0. {
        return None;
    }
    let lo = (fr * 60. / MAX_BPM).floor().max(1.) as usize;
    let hi = (fr * 60. / MIN_BPM).ceil() as usize;
    let score = |lag: usize| {
        let bpm = 60. * fr / lag as f64;
        let prior = (-0.5 * (bpm / PREFERRED_BPM).log2().powi(2)).exp();
        // A real beat's double lag lines up too.
        prior * (ac(lag) + 0.5 * ac(lag * 2))
    };
    let best = (lo..=hi).max_by(|&a, &b| score(a).total_cmp(&score(b)))?;
    // A parabola through the neighbours: the period between frames.
    let (a, b, c) = (ac(best - 1), ac(best), ac(best + 1));
    let bend = a - 2. * b + c;
    let shift = if bend < 0. {
        (0.5 * (a - c) / bend).clamp(-0.5, 0.5)
    } else {
        0.
    };
    Some((best as f64 + shift, (b / zero).clamp(0., 1.)))
}

/// The beats (frame indices): the sequence that lands on the strongest
/// onsets while keeping about one period between beats.
fn track(onset: &[f32], period: f64) -> Vec<usize> {
    const TIGHTNESS: f64 = 100.;
    let n = onset.len();
    let mut score = vec![0f64; n];
    let mut back = vec![usize::MAX; n];
    let (near, far) = (
        (period / 2.).round().max(1.) as usize,
        (period * 2.).round() as usize,
    );
    for k in 0..n {
        let mut best = 0.;
        for gap in near..=far.min(k) {
            let j = k - gap;
            let miss = (gap as f64 / period).ln();
            let s = score[j] - TIGHTNESS * miss * miss;
            if back[k] == usize::MAX || s > best {
                best = s;
                back[k] = j;
            }
        }
        score[k] = onset[k] as f64 + best.max(0.);
        if best <= 0. {
            back[k] = usize::MAX;
        }
    }
    // From the best-scoring frame in the last period, walk back.
    let tail = n.saturating_sub(period.ceil() as usize);
    let Some(mut k) = (tail..n).max_by(|&a, &b| score[a].total_cmp(&score[b])) else {
        return vec![];
    };
    let mut beats = vec![k];
    while back[k] != usize::MAX {
        k = back[k];
        beats.push(k);
    }
    beats.reverse();
    beats
}

/// The highest value within `r` frames of `k`.
fn peak(v: &[f32], k: usize, r: usize) -> f32 {
    let (a, b) = (k.saturating_sub(r), (k + r + 1).min(v.len()));
    v[a..b].iter().copied().fold(0., f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beats::tests::clicks;

    #[test]
    fn silence_has_no_beats() {
        let mut a = Analyzer::new(44_100);
        a.process(&vec![0.; 44_100 * 10], 1);
        let r = a.analyze(10 * SECOND);
        assert!(r.beats.is_empty());
        assert_eq!(r.bpm, 0.);
    }

    #[test]
    fn finds_a_click_tracks_tempo_and_bars() {
        let mut a = Analyzer::new(44_100);
        for block in clicks(120., 20., 44_100).chunks(1000) {
            a.process(block, 1);
        }
        let r = a.analyze(20 * SECOND);
        assert!((r.bpm - 120.).abs() < 2., "bpm {}", r.bpm);
        let beats: Vec<&WireBeat> = r.beats.iter().filter(|b| b.level < 2.).collect();
        assert!(beats.len() >= 36, "{} beats", beats.len());
        let on_grid = beats
            .iter()
            .filter(|b| {
                let off = (b.time_micros / 500_000.).fract();
                off.min(1. - off) * 500. < 30. // ms
            })
            .count();
        assert!(
            on_grid * 10 >= beats.len() * 9,
            "{on_grid} of {}",
            beats.len()
        );
        // Bars land on the thumps (every 2 s).
        let bars: Vec<&&WireBeat> = beats.iter().filter(|b| b.level == 0.).collect();
        assert!(!bars.is_empty());
        let on_bar = bars
            .iter()
            .filter(|b| {
                let off = (b.time_micros / 2_000_000.).fract();
                off.min(1. - off) * 2000. < 30.
            })
            .count();
        assert!(on_bar * 4 >= bars.len() * 3, "{on_bar} of {}", bars.len());
    }
}
