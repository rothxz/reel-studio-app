//! Motion-graphic scenes the agent makes from nothing: an animated
//! backdrop (drifting light, a tech grid, a dot field or sweeping beams),
//! and on it a chart whose bars grow and numbers count up, a counter, a
//! growth line drawing itself, a two-column comparison, a list of steps
//! appearing one by one, a flow diagram whose nodes light up in turn as a
//! signal runs along its connectors (`flow.rs`), or a code editor whose
//! steps highlight their lines. Or the scene is composed freely
//! (`canvas.rs`): words, numbers, icons, shapes, bars and cards, each
//! placed, timed and moved as the script says.
//!
//! Every scene has a seed: it picks the layout a diagram takes when none
//! was asked for, and where the light drifts. A video made twice from the
//! same script gets a new seed, so it doesn't look the same twice.
//!
//! A scene is drawn frame by frame (tiny-skia for shapes, `text_render` for
//! the words, so they're the editor's own fonts) and written as an H.264
//! video (`movie.rs`) the timeline plays like any clip. The words that
//! narrate a scene stay text clips on top (titles, captions): editable.
//! Only the labels that belong to a chart are drawn into it.
//!
//! The layout leaves the top of a portrait frame to the title and the
//! bottom third to captions; the graphic sits in between.

use std::collections::HashMap;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

use anyhow::{Result, bail};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tiny_skia::{
    Color, FillRule, FilterQuality, GradientStop, IntSize, LineCap, LinearGradient, Paint,
    PathBuilder, Pixmap, PixmapPaint, Point, RadialGradient, Rect, SpreadMode, Stroke, Transform,
};

mod backdrops;
pub mod canvas;
mod flow;
mod icons;

pub use canvas::{Element, Flow};
pub use flow::{CodeStep, Layout, Node};
pub use icons::{Icon, technical};

use super::movie::Writer;
use crate::model::{Clip, Micros, SECOND};
use crate::text_render::{self, TextStyle};

type Rgb = (u8, u8, u8);

/// A scene's colours.
#[derive(Debug, PartialEq)]
pub struct Palette {
    pub name: &'static str,
    pub about: &'static str,
    /// The backdrop's gradient, top to bottom.
    pub bg: [Rgb; 2],
    pub accent: Rgb,
    pub accent2: Rgb,
    /// A third colour: diagrams give each step its own.
    pub accent3: Rgb,
    pub text: Rgb,
    /// A light backdrop (dark words, softer light).
    pub light: bool,
}

impl Palette {
    pub fn text_hex(&self) -> String {
        hex(self.text)
    }
}

pub const PALETTES: &[Palette] = &[
    Palette {
        name: "neon",
        about: "near-black navy with glowing cyan, violet and teal (tech, AI, code)",
        bg: [(10, 16, 32), (3, 5, 12)],
        accent: (34, 211, 238),
        accent2: (192, 132, 252),
        accent3: (45, 212, 191),
        text: (236, 242, 255),
        light: false,
    },
    Palette {
        name: "blueprint",
        about: "deep blueprint blue with sky, white and amber (engineering, systems)",
        bg: [(18, 46, 102), (7, 18, 46)],
        accent: (125, 211, 252),
        accent2: (250, 204, 21),
        accent3: (255, 255, 255),
        text: (255, 255, 255),
        light: false,
    },
    Palette {
        name: "ember",
        about: "charcoal with glowing orange, gold and coral (bold, sport, energy)",
        bg: [(34, 26, 24), (10, 8, 8)],
        accent: (251, 146, 60),
        accent2: (250, 204, 21),
        accent3: (248, 113, 113),
        text: (255, 245, 235),
        light: false,
    },
    Palette {
        name: "fresh",
        about: "deep teal, money green and gold (finance, growth, nature)",
        bg: [(16, 66, 66), (8, 24, 30)],
        accent: (47, 209, 139),
        accent2: (255, 209, 102),
        accent3: (86, 204, 242),
        text: (255, 255, 255),
        light: false,
    },
    Palette {
        name: "calm",
        about: "navy and sky blue with peach (explainers, education, science, health)",
        bg: [(30, 46, 84), (11, 17, 33)],
        accent: (90, 169, 255),
        accent2: (255, 184, 107),
        accent3: (125, 226, 209),
        text: (255, 255, 255),
        light: false,
    },
    Palette {
        name: "night",
        about: "near-black violet with neon lilac and cyan (bold, modern, the mind, space)",
        bg: [(28, 20, 58), (7, 5, 18)],
        accent: (179, 136, 255),
        accent2: (62, 224, 207),
        accent3: (255, 121, 198),
        text: (255, 255, 255),
        light: false,
    },
    Palette {
        name: "warm",
        about: "coral to rose with cream (friendly, lifestyle, food, people)",
        bg: [(255, 138, 91), (196, 64, 104)],
        accent: (255, 241, 193),
        accent2: (43, 45, 66),
        accent3: (255, 255, 255),
        text: (255, 255, 255),
        light: false,
    },
    Palette {
        name: "sunset",
        about: "amber to red with white (energetic)",
        bg: [(255, 170, 70), (230, 80, 85)],
        accent: (255, 255, 255),
        accent2: (60, 30, 50),
        accent3: (255, 230, 150),
        text: (255, 255, 255),
        light: false,
    },
    Palette {
        name: "paper",
        about: "warm off-white with teal and terracotta (clean, light, learning, history)",
        bg: [(248, 244, 235), (234, 225, 206)],
        accent: (42, 157, 143),
        accent2: (231, 111, 81),
        accent3: (38, 70, 83),
        text: (31, 31, 35),
        light: true,
    },
    Palette {
        name: "mono",
        about: "black and white (minimal)",
        bg: [(34, 34, 36), (6, 6, 7)],
        accent: (255, 255, 255),
        accent2: (150, 150, 155),
        accent3: (200, 200, 205),
        text: (255, 255, 255),
        light: false,
    },
];

pub fn palette(name: &str) -> Option<&'static Palette> {
    let n = name.trim().to_lowercase();
    PALETTES.iter().find(|p| p.name == n)
}

fn hex((r, g, b): Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// A labelled number (a bar, a point on a line).
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub label: String,
    pub value: f64,
}

/// What a scene shows on its backdrop.
#[derive(Clone, Debug, PartialEq)]
pub enum Visual {
    Background,
    Bars {
        title: Option<String>,
        items: Vec<Item>,
    },
    Counter {
        from: f64,
        to: f64,
        label: Option<String>,
    },
    Line {
        title: Option<String>,
        items: Vec<Item>,
    },
    Split {
        left: (String, Vec<String>),
        right: (String, Vec<String>),
    },
    List {
        title: Option<String>,
        items: Vec<String>,
    },
    /// Parts joined by connectors; each lights up in turn.
    Flow {
        title: Option<String>,
        nodes: Vec<Node>,
        layout: Layout,
    },
    /// A code editor; each step highlights its lines.
    Code {
        title: Option<String>,
        file: Option<String>,
        lines: Vec<String>,
        steps: Vec<CodeStep>,
    },
    /// Composed freely: elements placed, timed and moved one by one.
    Canvas {
        elements: Vec<Element>,
        flow: Flow,
    },
}

impl Visual {
    pub fn kind(&self) -> &'static str {
        match self {
            Visual::Background => "background",
            Visual::Bars { .. } => "bar_chart",
            Visual::Counter { .. } => "counter",
            Visual::Line { .. } => "line_chart",
            Visual::Split { .. } => "comparison",
            Visual::List { .. } => "steps",
            Visual::Flow { .. } => "flow",
            Visual::Code { .. } => "code",
            Visual::Canvas { .. } => "canvas",
        }
    }

    /// Whether the scene is about a technical topic (software, AI,
    /// infrastructure): it gets the engineer's look (mono type, dashed
    /// signal lines, step counters, tech icons). Anything else (the body,
    /// cooking, history…) is drawn plainer: rounded type, round badges,
    /// everyday icons.
    pub fn technical(&self) -> bool {
        let mut words: Vec<&str> = vec![];
        match self {
            Visual::Code { .. } => return true,
            Visual::Flow { title, nodes, .. } => {
                words.extend(title.as_deref());
                for n in nodes {
                    words.push(&n.label);
                    words.extend(n.detail.as_deref());
                    words.extend(n.note.as_deref());
                }
            }
            Visual::Split { left, right } => {
                for (t, items) in [left, right] {
                    words.push(t);
                    words.extend(items.iter().map(String::as_str));
                }
            }
            Visual::List { title, items } => {
                words.extend(title.as_deref());
                words.extend(items.iter().map(String::as_str));
            }
            Visual::Canvas { elements, .. } => return technical(&canvas::words(elements)),
            _ => {}
        }
        technical(&words.join(" "))
    }
}

pub const KINDS: &[(&str, &str)] = &[
    (
        "background",
        "the animated backdrop alone (titles / stickers go on top)",
    ),
    (
        "bar_chart",
        "bars that grow, values counting up (items: label + value)",
    ),
    (
        "counter",
        "one big number counting up in a ring (from, to, label)",
    ),
    (
        "line_chart",
        "a growth line drawing itself (items: label + value, in order)",
    ),
    (
        "comparison",
        "two columns, e.g. Needs vs Wants (left / right: title + items)",
    ),
    (
        "steps",
        "a numbered list appearing one by one (items: short lines)",
    ),
    (
        "flow",
        "a diagram: parts or steps (nodes: label, detail, icon, note) joined by connectors a glowing signal runs along, each lighting up in turn; layout snake, row, column or cycle",
    ),
    (
        "code",
        "a code editor: the lines appear, then each step highlights its lines with a note (code, steps)",
    ),
    (
        "canvas",
        "compose it yourself from elements (text, number, icon, shape, bar, card, line), each placed, timed and animated: for anything the ready graphics don't show",
    ),
];

/// What a scene's backdrop is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Backdrop {
    /// A slow gradient with soft drifting light.
    Glow,
    /// A dark field with a faint grid and pools of light (tech).
    Grid,
    /// A field of dots a slow wave runs through.
    Dots,
    /// Soft beams of light sweeping across.
    Beams,
}

pub const BACKDROPS: &[(Backdrop, &str, &str)] = &[
    (Backdrop::Glow, "glow", "soft drifting light"),
    (
        Backdrop::Grid,
        "grid",
        "faint tech grid with pools of light",
    ),
    (Backdrop::Dots, "dots", "a dot field with a slow wave"),
    (
        Backdrop::Beams,
        "beams",
        "soft beams of light sweeping across",
    ),
];

impl Backdrop {
    pub fn name(self) -> &'static str {
        BACKDROPS
            .iter()
            .find(|(b, _, _)| *b == self)
            .map_or("glow", |(_, n, _)| n)
    }

    pub fn parse(s: &str) -> Option<Backdrop> {
        let s = s.trim().to_lowercase();
        // How people describe a backdrop rather than name one.
        if matches!(
            s.as_str(),
            "dark" | "light" | "plain" | "simple" | "soft" | "gradient" | "default" | "clean"
        ) {
            return Some(Backdrop::Glow);
        }
        BACKDROPS
            .iter()
            .find(|(_, n, _)| *n == s)
            .map(|(b, _, _)| *b)
    }
}

#[derive(Clone, Debug)]
pub struct Scene {
    pub visual: Visual,
    pub palette: &'static Palette,
    pub backdrop: Backdrop,
    /// Picks what wasn't asked for (a diagram's layout, where light drifts).
    pub seed: u64,
    /// Around every number ("$", " /mo").
    pub prefix: String,
    pub suffix: String,
    pub length: Micros,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Scene {
    /// Where the scene's file goes: the same scene is drawn once.
    pub fn path(&self) -> PathBuf {
        let mut h = Sha256::new();
        h.update(format!(
            "v3|{:?}|{}|{:?}|{}|{}|{}|{}|{}|{}|{}",
            self.visual,
            self.palette.name,
            self.backdrop,
            self.seed,
            self.prefix,
            self.suffix,
            self.length,
            self.width,
            self.height,
            self.fps
        ));
        let name: String = h
            .finalize()
            .iter()
            .take(10)
            .map(|b| format!("{b:02x}"))
            .collect();
        crate::prefs::data_dir()
            .join("agent")
            .join("scenes")
            .join(format!("{}-{name}.mp4", self.visual.kind()))
    }

    fn seed(&self) -> f32 {
        let p = self.path();
        let s = p.to_string_lossy();
        s.bytes()
            .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32)) as f32
            % 1000.
    }

    /// Where the scene's own animation lands, for sound: (seconds into the
    /// scene, what it is: "text" for a node lighting up, "typing" for code
    /// appearing, seconds it lasts).
    pub fn beats(&self) -> Vec<(f32, &'static str, f32)> {
        let len = self.length as f32 / SECOND as f32;
        match &self.visual {
            Visual::Flow { nodes, .. } => flow::flow_timing(nodes.len(), len)
                .active
                .into_iter()
                .map(|t| (t, "text", 0.))
                .collect(),
            Visual::Code { lines, steps, .. } => {
                let tm = flow::code_timing(lines.len(), steps.len(), len);
                let mut out = vec![(tm.type_from, "typing", tm.type_to - tm.type_from)];
                out.extend(tm.steps.into_iter().map(|t| (t, "text", 0.)));
                out
            }
            Visual::List { items, .. } => (0..items.len())
                .map(|k| (0.4 + k as f32 * 0.5, "text", 0.))
                .filter(|(t, _, _)| *t < len - 0.3)
                .collect(),
            Visual::Canvas { elements, .. } => canvas::beats(elements, len),
            _ => vec![],
        }
    }
}

/// The frame size a scene is drawn at for a canvas (even, at most 1080 on
/// the short side: the compositor scales it like any clip).
pub fn frame_size(canvas_w: u32, canvas_h: u32) -> (u32, u32) {
    let (w, h) = (canvas_w.max(2) as f64, canvas_h.max(2) as f64);
    let k = (1080. / w.min(h)).min(1.);
    let even = |v: f64| ((v * k / 2.).round() as u32 * 2).max(2);
    (even(w), even(h))
}

/// Draws the scene into its file (kept: a second ask is instant).
/// `progress` gets 0–1 and says whether to go on; `workers` threads draw.
pub fn render(
    scene: &Scene,
    workers: usize,
    progress: &(dyn Fn(f32) -> bool + Sync),
) -> Result<PathBuf> {
    let path = scene.path();
    if path.exists() {
        return Ok(path);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("part.mp4");
    let frames =
        (((scene.length as f64 / SECOND as f64) * scene.fps as f64).ceil() as usize).max(1);
    let mut writer = Writer::create(&tmp, scene.width, scene.height, scene.fps)?;
    // Frames are drawn on `workers` threads (worker k draws k, k+n, …) and
    // encoded here in order; each hands over at most two ahead.
    let n = workers.clamp(1, frames);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let result = std::thread::scope(|s| -> Result<()> {
        let mut inbox = vec![];
        for k in 0..n {
            let (tx, rx) = std::sync::mpsc::sync_channel::<Pixmap>(2);
            inbox.push(rx);
            let stop = &stop;
            s.spawn(move || {
                let mut painter = Painter::new(scene);
                for f in (k..frames).step_by(n) {
                    if stop.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    if tx.send(painter.frame(f as f32 / scene.fps as f32)).is_err() {
                        return;
                    }
                }
            });
        }
        let out = (|| {
            for f in 0..frames {
                let pix = inbox[f % n]
                    .recv()
                    .map_err(|_| anyhow::anyhow!("a drawing thread stopped"))?;
                writer.push(pix.data())?;
                if !progress((f + 1) as f32 / frames as f32) {
                    bail!("Stopped");
                }
            }
            Ok(())
        })();
        if out.is_err() {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            drop(inbox);
        }
        out
    });
    if let Err(e) = result {
        drop(writer);
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    writer.finish()?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

// ---- drawing ----------------------------------------------------------------------------

/// `src` stretched over all of `dst` (bilinear, 8-bit fixed point): the
/// backdrop is drawn small and blown up, and tiny-skia's general scaler
/// spent ~17 ms a 1080×1920 frame on it; this takes a few.
fn upscale(src: &Pixmap, dst: &mut Pixmap) {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let (dw, dh) = (dst.width() as usize, dst.height() as usize);
    // Each output column / row: the two source ones and the weight of the second (0–256).
    let taps = |d: usize, s: usize| -> Vec<(usize, usize, u32)> {
        (0..d)
            .map(|i| {
                let x = ((i as f32 + 0.5) * s as f32 / d as f32 - 0.5).max(0.);
                let x0 = (x as usize).min(s - 1);
                let x1 = (x0 + 1).min(s - 1);
                (x0, x1, ((x - x0 as f32) * 256.) as u32)
            })
            .collect()
    };
    let xs = taps(dw, sw);
    let ys = taps(dh, sh);
    let data = src.data();
    // Every source row stretched across first.
    let mut rows = vec![0u8; sh * dw * 4];
    for y in 0..sh {
        let row = &data[y * sw * 4..(y + 1) * sw * 4];
        let out = &mut rows[y * dw * 4..(y + 1) * dw * 4];
        for (x, &(x0, x1, f)) in xs.iter().enumerate() {
            for c in 0..4 {
                let a = row[x0 * 4 + c] as u32;
                let b = row[x1 * 4 + c] as u32;
                out[x * 4 + c] = ((a * (256 - f) + b * f) >> 8) as u8;
            }
        }
    }
    let out = dst.data_mut();
    for (y, &(y0, y1, f)) in ys.iter().enumerate() {
        let a = &rows[y0 * dw * 4..(y0 + 1) * dw * 4];
        let b = &rows[y1 * dw * 4..(y1 + 1) * dw * 4];
        let o = &mut out[y * dw * 4..(y + 1) * dw * 4];
        for i in 0..dw * 4 {
            o[i] = ((a[i] as u32 * (256 - f) + b[i] as u32 * f) >> 8) as u8;
        }
    }
}

fn clamp01(v: f32) -> f32 {
    v.clamp(0., 1.)
}

fn ease_out(v: f32) -> f32 {
    let v = clamp01(v);
    1. - (1. - v).powi(3)
}

fn ease_back(v: f32) -> f32 {
    let v = clamp01(v);
    let c = 1.70158;
    1. + (c + 1.) * (v - 1.).powi(3) + c * (v - 1.).powi(2)
}

fn rgba((r, g, b): Rgb, a: f32) -> Color {
    Color::from_rgba8(r, g, b, (clamp01(a) * 255.).round() as u8)
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

/// "$1,250", "4.50", "12%": `v` with the scene's decimals and marks.
pub fn format_value(v: f64, decimals: usize, prefix: &str, suffix: &str) -> String {
    let neg = v < 0.;
    let s = format!("{:.*}", decimals, v.abs());
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i.to_string(), Some(f.to_string())),
        None => (s.clone(), None),
    };
    let mut grouped = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let body = match frac {
        Some(f) => format!("{grouped}.{f}"),
        None => grouped,
    };
    format!("{}{prefix}{body}{suffix}", if neg { "-" } else { "" })
}

/// Decimals the numbers need: none for whole numbers, two otherwise.
fn decimals_for(values: &[f64]) -> usize {
    if values.iter().all(|v| (v - v.round()).abs() < 1e-9) {
        0
    } else if values
        .iter()
        .all(|v| ((v * 10.) - (v * 10.).round()).abs() < 1e-9)
    {
        1
    } else {
        2
    }
}

enum Anchor {
    Center,
    Left,
}

struct Painter<'a> {
    scene: &'a Scene,
    w: f32,
    h: f32,
    /// Px per text unit (as text clips: the short side / 390).
    u: f32,
    seed: f32,
    words: HashMap<String, Pixmap>,
    decimals: usize,
    /// `Visual::technical`: the engineer's look, else the plain one.
    tech: bool,
    /// A canvas scene's elements placed (worked out on the first frame).
    canvas: Option<canvas::Laid>,
}

impl<'a> Painter<'a> {
    fn new(scene: &'a Scene) -> Self {
        let values: Vec<f64> = match &scene.visual {
            Visual::Bars { items, .. } | Visual::Line { items, .. } => {
                items.iter().map(|i| i.value).collect()
            }
            Visual::Counter { from, to, .. } => vec![*from, *to],
            _ => vec![],
        };
        Painter {
            scene,
            w: scene.width as f32,
            h: scene.height as f32,
            u: text_render::unit_px(scene.width, scene.height),
            seed: scene.seed(),
            words: HashMap::new(),
            tech: scene.visual.technical(),
            canvas: None,
            decimals: match decimals_for(&values) {
                // Money with cents has two places ($4.50, not $4.5).
                1 if ["$", "€", "£"].iter().any(|c| scene.prefix.contains(c)) => 2,
                d => d,
            },
        }
    }

    fn fmt(&self, v: f64) -> String {
        format_value(v, self.decimals, &self.scene.prefix, &self.scene.suffix)
    }

    /// The graphic's box: under the title, over the captions.
    fn area(&self) -> Rect {
        let (w, h) = (self.w, self.h);
        let r = if h >= w {
            Rect::from_ltrb(w * 0.07, h * 0.28, w * 0.93, h * 0.69)
        } else {
            Rect::from_ltrb(w * 0.14, h * 0.24, w * 0.86, h * 0.76)
        };
        r.unwrap_or_else(|| Rect::from_xywh(0., 0., w, h).unwrap())
    }

    fn frame(&mut self, t: f32) -> Pixmap {
        let mut pix = Pixmap::new(self.scene.width, self.scene.height).expect("frame");
        match self.scene.backdrop {
            Backdrop::Glow => self.backdrop(&mut pix, t),
            Backdrop::Grid => self.grid_backdrop(&mut pix, t),
            Backdrop::Dots => self.dots_backdrop(&mut pix, t),
            Backdrop::Beams => self.beams_backdrop(&mut pix, t),
        }
        match self.scene.visual.clone() {
            Visual::Background => {}
            Visual::Bars { title, items } => self.bars(&mut pix, t, title.as_deref(), &items),
            Visual::Counter { from, to, label } => {
                self.counter(&mut pix, t, from, to, label.as_deref())
            }
            Visual::Line { title, items } => self.line(&mut pix, t, title.as_deref(), &items),
            Visual::Split { left, right } => self.split(&mut pix, t, &left, &right),
            Visual::List { title, items } => self.list(&mut pix, t, title.as_deref(), &items),
            Visual::Flow {
                title,
                nodes,
                layout,
            } => self.flow(&mut pix, t, title.as_deref(), &nodes, layout),
            Visual::Code {
                title,
                file,
                lines,
                steps,
            } => self.code(
                &mut pix,
                t,
                title.as_deref(),
                file.as_deref(),
                &lines,
                &steps,
            ),
            Visual::Canvas { elements, flow } => self.canvas(&mut pix, t, &elements, flow),
        }
        pix
    }

    /// A slow gradient with soft drifting light, drawn small and scaled up
    /// (it's all soft, so nothing shows the difference).
    fn backdrop(&self, pix: &mut Pixmap, t: f32) {
        let p = self.scene.palette;
        let (bw, bh) = (
            (self.scene.width / 4).max(2),
            (self.scene.height / 4).max(2),
        );
        let Some(mut small) = Pixmap::new(bw, bh) else {
            return;
        };
        let (fw, fh) = (bw as f32, bh as f32);
        let sway = (t * 0.35 + self.seed).sin() * 0.2;
        if let Some(shader) = LinearGradient::new(
            Point::from_xy(fw * (0.3 + sway), 0.),
            Point::from_xy(fw * (0.7 - sway), fh),
            vec![
                GradientStop::new(0., rgba(p.bg[0], 1.)),
                GradientStop::new(1., rgba(p.bg[1], 1.)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            let paint = Paint {
                shader,
                ..Paint::default()
            };
            small.fill_rect(
                Rect::from_xywh(0., 0., fw, fh).unwrap(),
                &paint,
                Transform::identity(),
                None,
            );
        }
        let glow = if p.light { 0.16 } else { 0.22 };
        for i in 0..3 {
            let k = i as f32;
            let phase = self.seed * 0.37 + k * 2.1;
            let cx = fw * (0.5 + 0.38 * (t * 0.21 * (k + 1.) * 0.6 + phase).sin());
            let cy = fh * (0.5 + 0.4 * (t * 0.15 * (k + 1.3) * 0.6 + phase * 1.3).cos());
            let r = fw.max(fh) * (0.38 + 0.06 * k);
            let color = if i == 1 { p.accent2 } else { p.accent };
            if let Some(shader) = RadialGradient::new(
                Point::from_xy(cx, cy),
                Point::from_xy(cx, cy),
                r,
                vec![
                    GradientStop::new(0., rgba(color, glow)),
                    GradientStop::new(1., rgba(color, 0.)),
                ],
                SpreadMode::Pad,
                Transform::identity(),
            ) {
                let paint = Paint {
                    shader,
                    ..Paint::default()
                };
                small.fill_rect(
                    Rect::from_xywh(0., 0., fw, fh).unwrap(),
                    &paint,
                    Transform::identity(),
                    None,
                );
            }
        }
        if !p.light
            && let Some(shader) = RadialGradient::new(
                Point::from_xy(fw / 2., fh / 2.),
                Point::from_xy(fw / 2., fh / 2.),
                fw.max(fh) * 0.75,
                vec![
                    GradientStop::new(0.55, Color::from_rgba8(0, 0, 0, 0)),
                    GradientStop::new(1., Color::from_rgba8(0, 0, 0, 90)),
                ],
                SpreadMode::Pad,
                Transform::identity(),
            )
        {
            let paint = Paint {
                shader,
                ..Paint::default()
            };
            small.fill_rect(
                Rect::from_xywh(0., 0., fw, fh).unwrap(),
                &paint,
                Transform::identity(),
                None,
            );
        }
        upscale(&small, pix);
    }

    fn card(&self, pix: &mut Pixmap, r: Rect, alpha: f32, color: Option<Rgb>) {
        let p = self.scene.palette;
        let (fill, a) = match color {
            Some(c) => (c, 0.16),
            None if p.light => ((255, 255, 255), 0.62),
            None => ((255, 255, 255), 0.075),
        };
        let roundness = (56. * self.u / r.height().min(r.width())).min(1.);
        if let Some(path) = text_render::rounded(r, roundness) {
            pix.fill_path(
                &path,
                &paint(rgba(fill, a * alpha)),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
            let edge = if p.light { (0, 0, 0) } else { (255, 255, 255) };
            let stroke = Stroke {
                width: self.u * 0.8,
                ..Stroke::default()
            };
            pix.stroke_path(
                &path,
                &paint(rgba(edge, 0.08 * alpha)),
                &stroke,
                Transform::identity(),
                None,
            );
        }
    }

    /// The words as a picture (the editor's text look), kept for reuse.
    fn words(&mut self, text: &str, size: f32, color: Rgb, bold: bool) -> Option<&Pixmap> {
        let family = if bold {
            "Nunito_700Bold"
        } else {
            "Nunito_400Regular"
        };
        self.raster(text, size, color, family, false)
    }

    /// `words` in a font family (as text clips name them); never wrapped.
    fn words_in(&mut self, text: &str, size: f32, color: Rgb, family: &str) -> Option<&Pixmap> {
        self.raster(text, size, color, family, true)
    }

    fn raster(
        &mut self,
        text: &str,
        size: f32,
        color: Rgb,
        family: &str,
        one_line: bool,
    ) -> Option<&Pixmap> {
        let key = format!("{text}|{size}|{color:?}|{family}|{one_line}");
        if !self.words.contains_key(&key) {
            if self.words.len() > 400 {
                self.words.clear();
            }
            let mut extra = Map::new();
            extra.insert("fontFamily".into(), family.into());
            extra.insert("fontSize".into(), json!(size));
            extra.insert("color".into(), Value::String(hex(color)));
            let clip = Clip {
                id: String::new(),
                kind: "text".into(),
                timeline_start: 0,
                timeline_duration: SECOND,
                speed: 1.,
                source: None,
                source_start: None,
                source_duration: None,
                text: Some(text.into()),
                hidden: false,
                extra,
            };
            let style = TextStyle::from_clip(&clip);
            // One line: a wrap box far wider than the frame.
            let wrap_w = if one_line {
                self.scene.width * 4
            } else {
                self.scene.width
            };
            let raster = text_render::rasterize(&style, (wrap_w, self.scene.height), self.u)?;
            let mut data = raster.bgra;
            for px in data.as_chunks_mut::<4>().0 {
                px.swap(0, 2);
            }
            let pm = Pixmap::from_vec(data, IntSize::from_wh(raster.width, raster.height)?)?;
            self.words.insert(key.clone(), pm);
        }
        self.words.get(&key)
    }

    /// Draws words at (x, y): centred there, or starting there; scaled down
    /// to fit `max_w`.
    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        pix: &mut Pixmap,
        text: &str,
        size: f32,
        color: Rgb,
        bold: bool,
        (x, y): (f32, f32),
        anchor: Anchor,
        alpha: f32,
        max_w: f32,
    ) {
        if alpha <= 0.01 || text.trim().is_empty() {
            return;
        }
        let Some(w) = self.words(text, size, color, bold) else {
            return;
        };
        let (ww, wh) = (w.width() as f32, w.height() as f32);
        let s = (max_w / ww).min(1.);
        let left = match anchor {
            Anchor::Center => x - ww * s / 2.,
            Anchor::Left => x,
        };
        let pp = PixmapPaint {
            opacity: clamp01(alpha),
            quality: FilterQuality::Bilinear,
            ..PixmapPaint::default()
        };
        pix.draw_pixmap(
            0,
            0,
            w.as_ref(),
            &pp,
            Transform::from_row(s, 0., 0., s, left, y - wh * s / 2.),
            None,
        );
    }

    fn muted(&self) -> Rgb {
        self.scene.palette.text
    }

    fn appear(&self, t: f32) -> f32 {
        ease_out((t - 0.05) / 0.35)
    }

    // ---- the visuals ----------------------------------------------------------------

    fn bars(&mut self, pix: &mut Pixmap, t: f32, title: Option<&str>, items: &[Item]) {
        let p = self.scene.palette;
        let a = self.area();
        let show = self.appear(t);
        self.card(pix, a, show, None);
        let pad = a.width() * 0.07;
        let mut top = a.top() + pad;
        if let Some(title) = title {
            let c = self.muted();
            self.text(
                pix,
                title,
                17.,
                c,
                true,
                (a.left() + a.width() / 2., top + 12. * self.u),
                Anchor::Center,
                show,
                a.width() - pad * 2.,
            );
            top += 34. * self.u;
        }
        let values_room = 24. * self.u;
        let labels_room = 30. * self.u;
        let base = a.bottom() - pad - labels_room;
        let chart_top = top + values_room;
        let chart_h = (base - chart_top).max(1.);
        let n = items.len().max(1);
        let slot = (a.width() - pad * 2.) / n as f32;
        let bar_w = slot * 0.56;
        let max = items.iter().map(|i| i.value).fold(0f64, f64::max).max(1e-9);
        // The baseline.
        let mut pb = PathBuilder::new();
        pb.move_to(a.left() + pad, base);
        pb.line_to(a.right() - pad, base);
        if let Some(path) = pb.finish() {
            let stroke = Stroke {
                width: self.u * 1.2,
                line_cap: LineCap::Round,
                ..Stroke::default()
            };
            pix.stroke_path(
                &path,
                &paint(rgba(p.text, 0.25 * show)),
                &stroke,
                Transform::identity(),
                None,
            );
        }
        for (i, item) in items.iter().enumerate() {
            let grow = ease_out((t - 0.35 - i as f32 * 0.14) / 0.85);
            let x = a.left() + pad + slot * i as f32 + (slot - bar_w) / 2.;
            let full = (item.value.max(0.) / max) as f32 * chart_h;
            let h = (full * grow).max(if grow > 0. { self.u * 2. } else { 0. });
            let color = if (item.value - max).abs() < 1e-9 && n > 1 {
                p.accent2
            } else {
                p.accent
            };
            if h > 0.5
                && let Some(r) = Rect::from_xywh(x, base - h, bar_w, h)
                && let Some(path) = text_render::rounded(r, (bar_w * 0.22 / h.min(bar_w)).min(1.))
            {
                pix.fill_path(
                    &path,
                    &paint(rgba(color, 1.)),
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
            let cx = x + bar_w / 2.;
            let value = self.fmt(item.value * grow as f64);
            self.text(
                pix,
                &value,
                15.,
                p.text,
                true,
                (cx, base - h - 13. * self.u),
                Anchor::Center,
                (grow * 3.).min(1.),
                slot * 0.98,
            );
            let c = self.muted();
            self.text(
                pix,
                &item.label,
                12.,
                c,
                false,
                (cx, base + labels_room / 2.),
                Anchor::Center,
                show * 0.85,
                slot * 0.96,
            );
        }
    }

    fn counter(&mut self, pix: &mut Pixmap, t: f32, from: f64, to: f64, label: Option<&str>) {
        let p = self.scene.palette;
        let a = self.area();
        let (cx, cy) = (a.left() + a.width() / 2., a.top() + a.height() * 0.44);
        let r = a.width().min(a.height()) * 0.36;
        let show = self.appear(t);
        let run = ease_out((t - 0.3) / 1.9);
        let stroke = Stroke {
            width: r * 0.09,
            line_cap: LineCap::Round,
            ..Stroke::default()
        };
        if let Some(circle) = PathBuilder::from_circle(cx, cy, r) {
            pix.stroke_path(
                &circle,
                &paint(rgba(p.text, 0.12 * show)),
                &stroke,
                Transform::identity(),
                None,
            );
        }
        if run > 0.001 {
            let steps = (96. * run).ceil().max(2.) as usize;
            let mut pb = PathBuilder::new();
            for s in 0..=steps {
                let ang = -std::f32::consts::FRAC_PI_2
                    + std::f32::consts::TAU * run * s as f32 / steps as f32;
                let (x, y) = (cx + r * ang.cos(), cy + r * ang.sin());
                if s == 0 {
                    pb.move_to(x, y);
                } else {
                    pb.line_to(x, y);
                }
            }
            if let Some(arc) = pb.finish() {
                pix.stroke_path(
                    &arc,
                    &paint(rgba(p.accent, 1.)),
                    &stroke,
                    Transform::identity(),
                    None,
                );
            }
        }
        let pop = 0.85 + 0.15 * ease_back((t - 0.1) / 0.5);
        let value = self.fmt(from + (to - from) * run as f64);
        let size = 46. * pop;
        self.text(
            pix,
            &value,
            size,
            p.text,
            true,
            (cx, cy),
            Anchor::Center,
            show,
            r * 1.7,
        );
        if let Some(label) = label {
            let c = self.muted();
            self.text(
                pix,
                label,
                16.,
                c,
                false,
                (cx, cy + r + 30. * self.u),
                Anchor::Center,
                ease_out((t - 0.6) / 0.4) * 0.9,
                a.width(),
            );
        }
    }

    fn line(&mut self, pix: &mut Pixmap, t: f32, title: Option<&str>, items: &[Item]) {
        let p = self.scene.palette;
        let a = self.area();
        let show = self.appear(t);
        self.card(pix, a, show, None);
        let pad = a.width() * 0.08;
        let mut top = a.top() + pad;
        if let Some(title) = title {
            let c = self.muted();
            self.text(
                pix,
                title,
                17.,
                c,
                true,
                (a.left() + a.width() / 2., top + 12. * self.u),
                Anchor::Center,
                show,
                a.width() - pad * 2.,
            );
            top += 34. * self.u;
        }
        let base = a.bottom() - pad - 28. * self.u;
        let chart_top = top + 30. * self.u;
        let (left, right) = (a.left() + pad, a.right() - pad);
        let n = items.len();
        if n == 0 {
            return;
        }
        let max = items.iter().map(|i| i.value).fold(0f64, f64::max).max(1e-9);
        let pts: Vec<(f32, f32)> = items
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let x = if n == 1 {
                    (left + right) / 2.
                } else {
                    left + (right - left) * i as f32 / (n - 1) as f32
                };
                let y = base - (it.value.max(0.) / max) as f32 * (base - chart_top);
                (x, y)
            })
            .collect();
        let run = ease_out((t - 0.35) / 2.0);
        let cut = left + (right - left) * run;
        let mut drawn: Vec<(f32, f32)> = vec![pts[0]];
        for w in pts.windows(2) {
            let ((x0, y0), (x1, y1)) = (w[0], w[1]);
            if x1 <= cut {
                drawn.push((x1, y1));
            } else if x0 < cut {
                let k = (cut - x0) / (x1 - x0).max(1e-3);
                drawn.push((cut, y0 + (y1 - y0) * k));
                break;
            } else {
                break;
            }
        }
        if run > 0.001 && drawn.len() >= 2 {
            let mut fill = PathBuilder::new();
            fill.move_to(drawn[0].0, base);
            for &(x, y) in &drawn {
                fill.line_to(x, y);
            }
            fill.line_to(drawn[drawn.len() - 1].0, base);
            fill.close();
            if let Some(path) = fill.finish()
                && let Some(shader) = LinearGradient::new(
                    Point::from_xy(0., chart_top),
                    Point::from_xy(0., base),
                    vec![
                        GradientStop::new(0., rgba(p.accent, 0.35)),
                        GradientStop::new(1., rgba(p.accent, 0.02)),
                    ],
                    SpreadMode::Pad,
                    Transform::identity(),
                )
            {
                let pa = Paint {
                    shader,
                    anti_alias: true,
                    ..Paint::default()
                };
                pix.fill_path(&path, &pa, FillRule::Winding, Transform::identity(), None);
            }
            let mut pb = PathBuilder::new();
            pb.move_to(drawn[0].0, drawn[0].1);
            for &(x, y) in &drawn[1..] {
                pb.line_to(x, y);
            }
            if let Some(path) = pb.finish() {
                let stroke = Stroke {
                    width: self.u * 3.4,
                    line_cap: LineCap::Round,
                    line_join: tiny_skia::LineJoin::Round,
                    ..Stroke::default()
                };
                pix.stroke_path(
                    &path,
                    &paint(rgba(p.accent, 1.)),
                    &stroke,
                    Transform::identity(),
                    None,
                );
            }
        }
        for (i, (&(x, y), it)) in pts.iter().zip(items).enumerate() {
            let reached = x <= cut + 0.5;
            if reached && let Some(dot) = PathBuilder::from_circle(x, y, self.u * 4.6) {
                pix.fill_path(
                    &dot,
                    &paint(rgba(p.accent2, 1.)),
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
            let c = self.muted();
            self.text(
                pix,
                &it.label,
                11.5,
                c,
                false,
                (x, base + 16. * self.u),
                Anchor::Center,
                show * 0.85,
                (right - left) / n.max(1) as f32 * 1.1,
            );
            let last = i + 1 == n;
            if reached && (last || i == 0) {
                let value = self.fmt(it.value);
                self.text(
                    pix,
                    &value,
                    if last { 17. } else { 13. },
                    p.text,
                    true,
                    (
                        x.clamp(left + 30. * self.u, right - 30. * self.u),
                        y - 17. * self.u,
                    ),
                    Anchor::Center,
                    1.,
                    (right - left) * 0.5,
                );
            }
        }
    }

    fn split(
        &mut self,
        pix: &mut Pixmap,
        t: f32,
        left: &(String, Vec<String>),
        right: &(String, Vec<String>),
    ) {
        let p = self.scene.palette;
        let a = self.area();
        let u = self.u;
        let gap = a.width() * 0.04;
        let col_w = (a.width() - gap) / 2.;
        let show = self.appear(t);
        // A picture over each side when both sides' words suggest one
        // (Learning → book, Memory → brain).
        let words = [left.1.join(" "), right.1.join(" ")];
        let icons = (Icon::suggest(&left.0, &words[0], self.tech).is_some()
            && Icon::suggest(&right.0, &words[1], self.tech).is_some())
        .then(|| {
            Icon::guess_all(
                &[
                    (left.0.as_str(), words[0].as_str(), None),
                    (right.0.as_str(), words[1].as_str(), None),
                ],
                self.tech,
            )
        });
        // The cards as tall as what's in them, centred in the area.
        let badge = 56. * u;
        let pad = 22. * u;
        let head = pad + if icons.is_some() { badge + 14. * u } else { 0. } + 26. * u;
        let n = left.1.len().max(right.1.len()).max(1);
        let col_h = (head + 10. * u + n as f32 * 46. * u + pad).min(a.height());
        let top = a.top() + (a.height() - col_h) / 2.;
        let row_h = ((col_h - head - 10. * u - pad) / n as f32).min(52. * u);
        for (side, (title, items)) in [left, right].into_iter().enumerate() {
            let x = a.left() + side as f32 * (col_w + gap);
            let Some(col) = Rect::from_xywh(x, top, col_w, col_h) else {
                continue;
            };
            let color = if side == 0 { p.accent } else { p.accent2 };
            self.card(pix, col, show, Some(color));
            let cx = x + col_w / 2.;
            let mut y = top + pad;
            if let Some(icons) = &icons {
                let e = ease_out((t - 0.2 - side as f32 * 0.12) / 0.4);
                let s = badge * (0.75 + 0.25 * e);
                if let Some(b) = Rect::from_xywh(cx - s / 2., y + (badge - s) / 2., s, s) {
                    self.glass(pix, b, s / 2., color, 1., 0.4, e);
                    let inset = s * 0.25;
                    if let Some(ir) = Rect::from_ltrb(
                        b.left() + inset,
                        b.top() + inset,
                        b.right() - inset,
                        b.bottom() - inset,
                    ) {
                        icons[side].draw(pix, ir, color, e, (s * 0.055).max(1.5 * u));
                    }
                }
                y += badge + 14. * u;
            }
            let head_y = y + 13. * u;
            self.text(
                pix,
                title,
                19.,
                color,
                true,
                (cx, head_y),
                Anchor::Center,
                show,
                col_w * 0.9,
            );
            let row_top = head_y + 13. * u + 10. * u;
            for (k, item) in items.iter().enumerate() {
                // Left and right take turns.
                let at = 0.55 + (k * 2 + side) as f32 * 0.32;
                let e = ease_out((t - at) / 0.35);
                let y = row_top + row_h * (k as f32 + 0.5) + (1. - e) * 14. * u;
                let c = self.muted();
                self.text(
                    pix,
                    item,
                    16.,
                    c,
                    false,
                    (cx, y),
                    Anchor::Center,
                    e,
                    col_w * 0.9,
                );
            }
        }
    }

    fn list(&mut self, pix: &mut Pixmap, t: f32, title: Option<&str>, items: &[String]) {
        let p = self.scene.palette;
        let a = self.area();
        let show = self.appear(t);
        let mut top = a.top();
        if let Some(title) = title {
            let c = self.muted();
            self.text(
                pix,
                title,
                19.,
                c,
                true,
                (a.left() + a.width() / 2., top + 14. * self.u),
                Anchor::Center,
                show,
                a.width(),
            );
            top += 38. * self.u;
        }
        let n = items.len().max(1);
        let gap = 10. * self.u;
        let row_h = ((a.bottom() - top - gap * (n as f32 - 1.)) / n as f32).min(58. * self.u);
        for (k, item) in items.iter().enumerate() {
            let at = 0.4 + k as f32 * 0.5;
            let e = ease_out((t - at) / 0.4);
            if e <= 0. {
                continue;
            }
            let dx = (1. - e) * -40. * self.u;
            let y = top + k as f32 * (row_h + gap);
            let Some(row) = Rect::from_xywh(a.left() + dx, y, a.width(), row_h) else {
                continue;
            };
            self.card(pix, row, e, None);
            let r = row_h * 0.3;
            let (dot_x, dot_y) = (row.left() + row_h * 0.5, y + row_h / 2.);
            let pop = ease_back((t - at - 0.1) / 0.4);
            if let Some(dot) = PathBuilder::from_circle(dot_x, dot_y, (r * pop).max(0.1)) {
                pix.fill_path(
                    &dot,
                    &paint(rgba(p.accent, e)),
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
            let num_color = if p.light { (255, 255, 255) } else { p.bg[1] };
            self.text(
                pix,
                &(k + 1).to_string(),
                14.,
                num_color,
                true,
                (dot_x, dot_y),
                Anchor::Center,
                e * clamp01(pop),
                r * 1.6,
            );
            let c = self.muted();
            self.text(
                pix,
                item,
                16.,
                c,
                false,
                (row.left() + row_h * 0.95, dot_y),
                Anchor::Left,
                e,
                row.width() - row_h * 1.2,
            );
        }
    }
}

/// Renders one frame (tests and previews).
#[cfg(test)]
pub fn still(scene: &Scene, t: f32) -> Pixmap {
    Painter::new(scene).frame(t)
}

/// Writes a frame as PNG (for checking a look by eye).
#[cfg(test)]
pub fn save_still(scene: &Scene, t: f32, path: &Path) -> Result<()> {
    still(scene, t).save_png(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(visual: Visual) -> Scene {
        Scene {
            visual,
            palette: &PALETTES[0],
            backdrop: Backdrop::Glow,
            seed: 1,
            prefix: "$".into(),
            suffix: String::new(),
            length: SECOND,
            width: 540,
            height: 960,
            fps: 30,
        }
    }

    #[test]
    fn formats_values() {
        assert_eq!(format_value(1825., 0, "$", ""), "$1,825");
        assert_eq!(format_value(4.5, 2, "$", ""), "$4.50");
        assert_eq!(format_value(12., 0, "", "%"), "12%");
        assert_eq!(format_value(-1500.25, 2, "$", ""), "-$1,500.25");
        assert_eq!(decimals_for(&[4.5, 3.]), 1);
        assert_eq!(decimals_for(&[4.25, 3.]), 2);
        assert_eq!(decimals_for(&[4., 3.]), 0);
    }

    #[test]
    fn frame_sizes_are_even_and_capped() {
        assert_eq!(frame_size(1080, 1920), (1080, 1920));
        assert_eq!(frame_size(2160, 3840), (1080, 1920));
        assert_eq!(frame_size(1921, 1081), (1920, 1080));
    }

    #[test]
    fn every_visual_draws() {
        let items = vec![
            Item {
                label: "Coffee".into(),
                value: 4.5,
            },
            Item {
                label: "Snacks".into(),
                value: 3.,
            },
        ];
        for v in [
            Visual::Background,
            Visual::Bars {
                title: Some("A day".into()),
                items: items.clone(),
            },
            Visual::Counter {
                from: 0.,
                to: 1825.,
                label: Some("a year".into()),
            },
            Visual::Line {
                title: None,
                items: items.clone(),
            },
            Visual::Split {
                left: ("Needs".into(), vec!["Rent".into()]),
                right: ("Wants".into(), vec!["Latte".into()]),
            },
            Visual::List {
                title: None,
                items: vec!["Track every spend".into()],
            },
            Visual::Flow {
                title: Some("How it works".into()),
                nodes: rag_nodes(),
                layout: Layout::Auto,
            },
            Visual::Code {
                title: None,
                file: Some("app.py".into()),
                lines: vec!["# fetch".into(), "docs = search(q)".into()],
                steps: vec![CodeStep {
                    label: "Fetch".into(),
                    from: 1,
                    to: 2,
                    note: None,
                }],
            },
        ] {
            let mut s = scene(v);
            s.length = 3 * SECOND;
            let a = still(&s, 0.2);
            let b = still(&s, 2.5);
            assert_eq!((a.width(), a.height()), (540, 960));
            // The picture moves.
            assert_ne!(a.data(), b.data(), "{}", s.visual.kind());
            // Opaque all over (it's a video frame).
            assert!(b.pixels().iter().all(|p| p.alpha() == 255));
        }
    }

    fn rag_nodes() -> Vec<Node> {
        [
            (
                "Query",
                "\"What's our API rate limit?\"",
                "user asks a question",
            ),
            (
                "Retriever",
                "semantic vector search",
                "the query is embedded and searched",
            ),
            (
                "Knowledge base",
                "vector store · your docs",
                "top-k chunks come back from your docs",
            ),
            (
                "Augment",
                "query + retrieved context",
                "query and context merged into one prompt",
            ),
            (
                "LLM",
                "generate a grounded answer",
                "the LLM answers from the retrieved facts",
            ),
            (
                "Answer",
                "500 req/min (from docs)",
                "a grounded answer, with its source",
            ),
        ]
        .iter()
        .map(|(l, d, n)| Node {
            label: (*l).into(),
            detail: Some((*d).into()),
            icon: Icon::guess(l, d, true),
            note: Some((*n).into()),
        })
        .collect()
    }

    #[test]
    fn backdrops_draw_and_move() {
        for (b, _, _) in BACKDROPS {
            let mut s = scene(Visual::Background);
            s.backdrop = *b;
            let (x, y) = (still(&s, 0.), still(&s, 3.));
            assert!(y.pixels().iter().all(|p| p.alpha() == 255), "{b:?}");
            assert_ne!(x.data(), y.data(), "{b:?}");
        }
    }

    #[test]
    fn explainers_have_beats() {
        let mut s = scene(Visual::Flow {
            title: None,
            nodes: rag_nodes(),
            layout: Layout::Snake,
        });
        s.length = 10 * SECOND;
        let beats = s.beats();
        assert_eq!(beats.len(), 6);
        assert!(
            beats
                .iter()
                .all(|(t, k, _)| *t > 0. && *t < 10. && *k == "text")
        );
    }

    #[test]
    fn scenes_know_their_topic() {
        let node = |l: &str| Node {
            label: l.into(),
            detail: None,
            icon: Icon::Sparkle,
            note: None,
        };
        let flow = |labels: &[&str]| Visual::Flow {
            title: None,
            nodes: labels.iter().map(|l| node(l)).collect(),
            layout: Layout::Row,
        };
        assert!(!flow(&["Brain", "Memory", "Thinking", "Emotions"]).technical());
        assert!(flow(&["Query", "Retriever", "Vector store", "LLM"]).technical());
        assert!(
            !Visual::Split {
                left: ("Learning".into(), vec!["New experiences".into()]),
                right: ("Memory".into(), vec!["Recall knowledge".into()]),
            }
            .technical()
        );
    }

    #[test]
    fn same_scene_same_file() {
        let a = scene(Visual::Background);
        let mut b = scene(Visual::Background);
        assert_eq!(a.path(), b.path());
        b.palette = &PALETTES[1];
        assert_ne!(a.path(), b.path());
    }

    /// Writes one frame of each visual to the scratch folder for a look.
    #[test]
    #[ignore]
    fn save_looks() {
        let dir = std::env::var("REEL_LOOKS_DIR").unwrap_or_else(|_| "/tmp".into());
        let items: Vec<Item> = [("Coffee", 4.5), ("Snacks", 3.), ("Rides", 6.), ("Apps", 2.)]
            .iter()
            .map(|(l, v)| Item {
                label: (*l).into(),
                value: *v,
            })
            .collect();
        let mut s = scene(Visual::Bars {
            title: Some("Every day".into()),
            items: items.clone(),
        });
        s.width = 1080;
        s.height = 1920;
        let looks = [
            s.visual.clone(),
            Visual::Counter {
                from: 0.,
                to: 5475.,
                label: Some("in one year".into()),
            },
            Visual::Line {
                title: Some("Saving $5 a day".into()),
                items: [
                    ("Month 1", 150.),
                    ("Month 3", 450.),
                    ("Month 6", 900.),
                    ("Year 1", 1825.),
                ]
                .iter()
                .map(|(l, v)| Item {
                    label: (*l).into(),
                    value: *v,
                })
                .collect(),
            },
            Visual::Split {
                left: (
                    "Needs".into(),
                    vec!["Rent".into(), "Groceries".into(), "Transport".into()],
                ),
                right: (
                    "Wants".into(),
                    vec![
                        "Daily latte".into(),
                        "New sneakers".into(),
                        "Streaming".into(),
                    ],
                ),
            },
            Visual::List {
                title: Some("Budget in 3 steps".into()),
                items: vec![
                    "Write down every spend".into(),
                    "Set a daily limit".into(),
                    "Save first, spend after".into(),
                ],
            },
        ];
        for (i, v) in looks.into_iter().enumerate() {
            s.visual = v;
            s.palette = &PALETTES[i % PALETTES.len()];
            save_still(&s, 3., &Path::new(&dir).join(format!("look-{i}.png"))).unwrap();
        }
        // The explainers, in each layout and backdrop, mid-way and lit.
        s.length = 12 * SECOND;
        s.palette = palette("neon").unwrap();
        let layouts = [Layout::Snake, Layout::Row, Layout::Column, Layout::Cycle];
        for (i, layout) in layouts.into_iter().enumerate() {
            let n = if layout == Layout::Snake { 6 } else { 4 };
            s.visual = Visual::Flow {
                title: Some("now the LLM answers from the real source".into()),
                nodes: rag_nodes().into_iter().take(n).collect(),
                layout,
            };
            s.backdrop = BACKDROPS[i % BACKDROPS.len()].0;
            s.seed = i as u64;
            save_still(&s, 7.3, &Path::new(&dir).join(format!("flow-{i}.png"))).unwrap();
        }
        s.visual = Visual::Code {
            title: Some("the same pipeline, written as code".into()),
            file: Some("rag_pipeline.py  —  editor".into()),
            lines: [
                "# 1. RETRIEVE: search your vector database",
                "docs = vector_store.similarity_search(query, k=3)",
                "",
                "# 2. AUGMENT: combine the query + retrieved context",
                "prompt = f\"Use this context: {docs}\\nQuestion: {query}\"",
                "",
                "# 3. GENERATE: the LLM answers from your data",
                "answer = llm.invoke(prompt)",
            ]
            .iter()
            .map(|l| l.to_string())
            .collect(),
            steps: vec![
                CodeStep {
                    label: "Retrieve".into(),
                    from: 1,
                    to: 2,
                    note: Some("the retriever pulls the top 3 chunks".into()),
                },
                CodeStep {
                    label: "Augment".into(),
                    from: 4,
                    to: 5,
                    note: Some("query + context become one grounded prompt".into()),
                },
                CodeStep {
                    label: "Generate".into(),
                    from: 7,
                    to: 8,
                    note: None,
                },
            ],
        };
        s.backdrop = Backdrop::Grid;
        save_still(&s, 7., &Path::new(&dir).join("code.png")).unwrap();
        for (i, (b, name, _)) in BACKDROPS.iter().enumerate() {
            s.visual = Visual::Background;
            s.backdrop = *b;
            s.palette = &PALETTES[(i * 3) % PALETTES.len()];
            save_still(
                &s,
                2.,
                &Path::new(&dir).join(format!("backdrop-{name}.png")),
            )
            .unwrap();
        }
    }

    /// Speed of one 5 s 1080×1920 scene: drawing vs encoding (run with
    /// --release).
    #[test]
    #[ignore]
    fn bench_scene() {
        crate::decoder::init();
        let mut s = scene(Visual::Bars {
            title: Some("Every day".into()),
            items: vec![
                Item {
                    label: "Coffee".into(),
                    value: 4.5,
                },
                Item {
                    label: "Snacks".into(),
                    value: 3.,
                },
            ],
        });
        (s.width, s.height, s.length) = (1080, 1920, 5 * SECOND);
        let t9 = std::time::Instant::now();
        let _ = std::fs::remove_file(s.path());
        render(&s, 4, &|_| true).unwrap();
        println!("render with 4 threads {:?}/frame", t9.elapsed() / 150);
        let _ = std::fs::remove_file(s.path());
        let mut painter = Painter::new(&s);
        let t0 = std::time::Instant::now();
        let frames: Vec<Pixmap> = (0..150).map(|f| painter.frame(f as f32 / 30.)).collect();
        let draw = t0.elapsed();
        let dir = std::env::temp_dir().join("reel-bench.mp4");
        let t1 = std::time::Instant::now();
        let mut w = Writer::create(&dir, 1080, 1920, 30).unwrap();
        for f in &frames {
            w.push(f.data()).unwrap();
        }
        w.finish().unwrap();
        println!(
            "draw {:?}/frame, encode {:?}/frame",
            draw / 150,
            t1.elapsed() / 150
        );
        let t2 = std::time::Instant::now();
        let mut pix = Pixmap::new(1080, 1920).unwrap();
        for f in 0..150 {
            painter.backdrop(&mut pix, f as f32 / 30.);
        }
        println!("backdrop {:?}/frame", t2.elapsed() / 150);
    }
}
