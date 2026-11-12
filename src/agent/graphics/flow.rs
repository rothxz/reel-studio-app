//! Explainer scenes: a flow diagram and a code editor.
//!
//! **Flow**: parts or steps as glowing cards or tiles with an icon, joined
//! by dashed connectors. The nodes arrive one after another; then each
//! lights up in turn as a bright signal runs along the connector into it,
//! with the step's note under the diagram and a row of progress dots. Four
//! layouts: a snake of two columns, a row, a column, a cycle; when none is
//! asked for, the scene's seed picks one that suits the count and frame.
//!
//! **Code**: an editor window whose lines appear one by one (coloured:
//! keywords, strings, comments), then each step highlights its lines with
//! a bar, its pill lit below and its note under that.
//!
//! A diagram about anything but tech (`Visual::technical`) is drawn plainer:
//! rounded type, round icon badges, solid connectors, labels that wrap
//! instead of shrinking, and no step counter unless the nodes carry notes.
//!
//! The timings (`flow_timing`, `code_timing`) are shared with
//! `Scene::beats`, so sound effects land on the frames that change.

use tiny_skia::{
    FillRule, GradientStop, Paint, Path, PathBuilder, Pixmap, Point, RadialGradient, Rect,
    SpreadMode, Stroke, StrokeDash, Transform,
};

use super::{Anchor, Icon, LineCap, Painter, Rgb, SECOND, clamp01, ease_out, paint, rgba};
use crate::agent::dice::Dice;
use crate::text_render;

const MONO: &str = "IBM Plex Mono";
const MONO_BOLD: &str = "IBMPlexMono_600SemiBold";
/// The plain look's type (a scene that isn't about tech).
const SANS: &str = "Nunito_400Regular";
const SANS_BOLD: &str = "Nunito_700Bold";

/// One part of a diagram.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub label: String,
    pub detail: Option<String>,
    pub icon: Icon,
    /// Shown under the diagram while this node is lit.
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Layout {
    Auto,
    /// Two columns, the path snaking down (left→right, then right→left).
    Snake,
    Row,
    Column,
    /// Round in a ring, the last joined back to the first.
    Cycle,
}

impl Layout {
    pub const NAMES: &[&str] = &["auto", "snake", "row", "column", "cycle"];

    pub fn parse(s: &str) -> Option<Layout> {
        Some(match s.trim().to_lowercase().as_str() {
            "" | "auto" => Layout::Auto,
            "snake" | "zigzag" | "grid" | "two_columns" => Layout::Snake,
            "row" | "horizontal" | "chain" | "pipeline" | "line" => Layout::Row,
            "column" | "vertical" | "stack" | "list" => Layout::Column,
            "cycle" | "loop" | "circle" | "ring" => Layout::Cycle,
            _ => return None,
        })
    }

    /// The layout to draw: the one asked for, else one that suits `n`
    /// nodes in this frame, picked by `seed`.
    pub fn resolve(self, n: usize, portrait: bool, seed: u64) -> Layout {
        let fits = |l: Layout| match l {
            Layout::Row => n <= if portrait { 4 } else { 6 },
            Layout::Column => n <= if portrait { 6 } else { 4 },
            Layout::Snake => n >= 3 && n <= 8,
            Layout::Cycle => (3..=6).contains(&n),
            Layout::Auto => false,
        };
        if self != Layout::Auto && fits(self) {
            return self;
        }
        if self != Layout::Auto {
            // Asked for one that can't hold them: the snake holds most.
            return if n <= 8 {
                Layout::Snake
            } else {
                Layout::Column
            };
        }
        let options: Vec<Layout> = [Layout::Snake, Layout::Row, Layout::Column, Layout::Cycle]
            .into_iter()
            .filter(|l| fits(*l) && *l != Layout::Cycle)
            .collect();
        if options.is_empty() {
            Layout::Snake
        } else {
            *Dice::new(seed).pick(&options)
        }
    }
}

/// A code step: its lines (1-based, inclusive) and a note.
#[derive(Clone, Debug, PartialEq)]
pub struct CodeStep {
    pub label: String,
    pub from: usize,
    pub to: usize,
    pub note: Option<String>,
}

pub(super) struct FlowTiming {
    /// When each node arrives.
    pub appear: Vec<f32>,
    /// When each node lights up.
    pub active: Vec<f32>,
    /// How long the signal takes along a connector (ending at `active`).
    pub travel: f32,
}

pub(super) fn flow_timing(n: usize, len: f32) -> FlowTiming {
    let n = n.max(1);
    let stagger = (0.9 / n as f32).min(0.14);
    let appear: Vec<f32> = (0..n).map(|i| 0.12 + i as f32 * stagger).collect();
    let start = appear[n - 1] + 0.5;
    let slot = ((len - start - 0.5) / n as f32).max(0.35);
    let active = (0..n).map(|k| start + k as f32 * slot).collect();
    FlowTiming {
        appear,
        active,
        travel: (slot * 0.45).clamp(0.2, 0.6),
    }
}

pub(super) struct CodeTiming {
    pub type_from: f32,
    pub type_to: f32,
    pub line_at: Vec<f32>,
    pub steps: Vec<f32>,
}

pub(super) fn code_timing(lines: usize, steps: usize, len: f32) -> CodeTiming {
    let n = lines.max(1);
    let type_from = 0.35;
    let per = (1.4 / n as f32).clamp(0.08, 0.22);
    let line_at = (0..n).map(|i| type_from + i as f32 * per).collect();
    let type_to = type_from + n as f32 * per;
    let start = type_to + 0.35;
    let slot = ((len - start - 0.4) / steps.max(1) as f32).max(0.5);
    CodeTiming {
        type_from,
        type_to,
        line_at,
        steps: (0..steps).map(|k| start + k as f32 * slot).collect(),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    /// A square with the icon, the words under it.
    Tile,
    /// A wide card: the icon on the left, the words beside it.
    Card,
}

struct Placed {
    r: Rect,
    shape: Shape,
    /// Room for the words under a tile.
    label_w: f32,
}

/// A connector: a quadratic curve (straight when `c` is the middle).
#[derive(Clone, Copy)]
struct Link {
    p0: Point,
    c: Point,
    p1: Point,
}

impl Link {
    fn straight(p0: Point, p1: Point) -> Link {
        Link {
            p0,
            c: Point::from_xy((p0.x + p1.x) / 2., (p0.y + p1.y) / 2.),
            p1,
        }
    }

    fn at(&self, s: f32) -> Point {
        let m = 1. - s;
        Point::from_xy(
            m * m * self.p0.x + 2. * m * s * self.c.x + s * s * self.p1.x,
            m * m * self.p0.y + 2. * m * s * self.c.y + s * s * self.p1.y,
        )
    }

    /// The curve from its start to `upto` (0–1).
    fn path(&self, upto: f32) -> Option<Path> {
        let upto = clamp01(upto);
        if upto <= 0.001 {
            return None;
        }
        let mut pb = PathBuilder::new();
        pb.move_to(self.p0.x, self.p0.y);
        let steps = 32;
        for i in 1..=steps {
            let p = self.at(upto * i as f32 / steps as f32);
            pb.line_to(p.x, p.y);
        }
        pb.finish()
    }
}

fn mix(a: Rgb, b: Rgb, k: f32) -> Rgb {
    let k = clamp01(k);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

/// A line of code in coloured runs: (column, text, kind).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Tok {
    Plain,
    Keyword,
    Func,
    Str,
    Num,
    Comment,
    Punct,
}

const KEYWORDS: &[&str] = &[
    "def",
    "return",
    "import",
    "from",
    "for",
    "in",
    "if",
    "elif",
    "else",
    "while",
    "class",
    "const",
    "let",
    "var",
    "fn",
    "function",
    "await",
    "async",
    "pub",
    "use",
    "match",
    "with",
    "as",
    "try",
    "except",
    "catch",
    "new",
    "and",
    "or",
    "not",
    "None",
    "True",
    "False",
    "null",
    "true",
    "false",
    "self",
    "this",
    "export",
    "struct",
    "impl",
    "type",
    "interface",
    "yield",
    "lambda",
    "SELECT",
    "FROM",
    "WHERE",
    "JOIN",
    "ORDER",
    "BY",
    "GROUP",
    "INSERT",
    "UPDATE",
];

fn tokenize(line: &str) -> Vec<(usize, String, Tok)> {
    let chars: Vec<char> = line.chars().collect();
    let mut toks: Vec<(usize, String, Tok)> = vec![];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let start = i;
        let rest: String = chars[i..].iter().collect();
        let kind = if c.is_whitespace() {
            i += 1;
            continue;
        } else if c == '#' || rest.starts_with("//") || rest.starts_with("--") {
            i = chars.len();
            Tok::Comment
        } else if c == '"' || c == '\'' || c == '`' {
            i += 1;
            while i < chars.len() && chars[i] != c {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(chars.len());
            Tok::Str
        } else if c.is_ascii_digit() {
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_')
            {
                i += 1;
            }
            Tok::Num
        } else if c.is_alphabetic() || c == '_' {
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            // f"…" / r"…": the prefix goes with its string.
            if i < chars.len() && (chars[i] == '"' || chars[i] == '\'') && word.len() <= 2 {
                let q = chars[i];
                i += 1;
                while i < chars.len() && chars[i] != q {
                    i += 1;
                }
                i = (i + 1).min(chars.len());
                Tok::Str
            } else if KEYWORDS.contains(&word.as_str()) {
                Tok::Keyword
            } else if i < chars.len() && chars[i] == '(' {
                Tok::Func
            } else {
                Tok::Plain
            }
        } else {
            i += 1;
            Tok::Punct
        };
        let text: String = chars[start..i].iter().collect();
        // Same-kind tokens apart only by spaces become one run (fewer
        // pictures); plain words and punctuation draw alike.
        let same = |a: Tok, b: Tok| {
            a == b || matches!((a, b), (Tok::Plain | Tok::Punct, Tok::Plain | Tok::Punct))
        };
        if let Some(last) = toks.last_mut()
            && same(last.2, kind)
        {
            let gap = start - (last.0 + last.1.chars().count());
            last.1.push_str(&" ".repeat(gap));
            last.1.push_str(&text);
            if last.2 == Tok::Punct {
                last.2 = kind;
            }
        } else {
            toks.push((start, text, kind));
        }
    }
    toks
}

impl Painter<'_> {
    /// The tall box explainers use: under the title, over the captions.
    fn tall_area(&self) -> Rect {
        let (w, h) = (self.w, self.h);
        let r = if h >= w {
            Rect::from_ltrb(w * 0.05, h * 0.235, w * 0.95, h * 0.715)
        } else {
            Rect::from_ltrb(w * 0.08, h * 0.2, w * 0.92, h * 0.8)
        };
        r.unwrap_or_else(|| Rect::from_xywh(0., 0., w, h).unwrap())
    }

    /// The step colours, starting where the seed says.
    fn step_colors(&self) -> [Rgb; 3] {
        let p = self.scene.palette;
        let all = [p.accent, p.accent3, p.accent2];
        let k = (self.scene.seed % 3) as usize;
        [all[k], all[(k + 1) % 3], all[(k + 2) % 3]]
    }

    #[allow(clippy::too_many_arguments)]
    fn text_in(
        &mut self,
        pix: &mut Pixmap,
        text: &str,
        size: f32,
        color: Rgb,
        family: &str,
        (x, y): (f32, f32),
        anchor: Anchor,
        alpha: f32,
        max_w: f32,
    ) {
        if alpha <= 0.01 || text.trim().is_empty() {
            return;
        }
        let Some(w) = self.words_in(text, size, color, family) else {
            return;
        };
        let (ww, wh) = (w.width() as f32, w.height() as f32);
        let s = (max_w / ww).min(1.);
        let left = match anchor {
            Anchor::Center => x - ww * s / 2.,
            Anchor::Left => x,
        };
        let pp = tiny_skia::PixmapPaint {
            opacity: clamp01(alpha),
            quality: tiny_skia::FilterQuality::Bilinear,
            ..tiny_skia::PixmapPaint::default()
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

    /// `text` on one line, or on two split at the space nearest its middle
    /// when one is wider than `room`.
    fn wrap2(&mut self, text: &str, size: f32, family: &str, room: f32) -> Vec<String> {
        if self.width_of(text, size, family) <= room || !text.contains(' ') {
            return vec![text.to_string()];
        }
        let mid = text.len() / 2;
        let cut = text
            .match_indices(' ')
            .map(|(i, _)| i)
            .min_by_key(|i| i.abs_diff(mid))
            .unwrap_or(mid);
        vec![
            text[..cut].trim().to_string(),
            text[cut..].trim().to_string(),
        ]
    }

    fn width_of(&mut self, text: &str, size: f32, family: &str) -> f32 {
        self.words_in(text, size, (255, 255, 255), family)
            .map_or(0., |p| p.width() as f32)
    }

    /// Words in a rounded outline (a heading, a step pill).
    #[allow(clippy::too_many_arguments)]
    fn pill(
        &mut self,
        pix: &mut Pixmap,
        text: &str,
        size: f32,
        (cx, cy): (f32, f32),
        color: Rgb,
        lit: f32,
        alpha: f32,
        max_w: f32,
    ) {
        if alpha <= 0.01 {
            return;
        }
        let u = self.u;
        let font = if self.tech { MONO } else { SANS_BOLD };
        let tw = self.width_of(text, size, font).min(max_w - 24. * u);
        let (w, h) = (tw + 26. * u, size * u * 2.2);
        let Some(r) = Rect::from_xywh(cx - w / 2., cy - h / 2., w, h) else {
            return;
        };
        if let Some(path) = text_render::rounded(r, 1.) {
            if lit > 0. {
                self.glow_path(pix, &path, color, lit * 0.7 * alpha);
            }
            pix.fill_path(
                &path,
                &paint(rgba(color, (0.08 + 0.12 * lit) * alpha)),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
            let stroke = Stroke {
                width: u * 0.9,
                ..Stroke::default()
            };
            pix.stroke_path(
                &path,
                &paint(rgba(color, (0.45 + 0.5 * lit) * alpha)),
                &stroke,
                Transform::identity(),
                None,
            );
        }
        let tc = mix(color, self.scene.palette.text, 0.25);
        self.text_in(
            pix,
            text,
            size,
            tc,
            font,
            (cx, cy),
            Anchor::Center,
            alpha,
            max_w - 24. * u,
        );
    }

    /// A soft glow round `path`: wide faint strokes under it.
    pub(super) fn glow_path(&self, pix: &mut Pixmap, path: &Path, color: Rgb, strength: f32) {
        if strength <= 0.01 {
            return;
        }
        for (w, a) in [(16., 0.035), (10., 0.06), (5., 0.11)] {
            let stroke = Stroke {
                width: w * self.u,
                line_cap: LineCap::Round,
                line_join: tiny_skia::LineJoin::Round,
                ..Stroke::default()
            };
            pix.stroke_path(
                path,
                &paint(rgba(color, a * strength)),
                &stroke,
                Transform::identity(),
                None,
            );
        }
    }

    /// A dark glass panel tinted and edged with `color`, lit 0–1.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn glass(
        &self,
        pix: &mut Pixmap,
        r: Rect,
        radius: f32,
        color: Rgb,
        lit: f32,
        glow: f32,
        alpha: f32,
    ) {
        let p = self.scene.palette;
        let roundness = (radius / (r.width().min(r.height()) / 2.)).min(1.);
        let Some(path) = text_render::rounded(r, roundness) else {
            return;
        };
        self.glow_path(pix, &path, color, glow * alpha);
        let base = if p.light {
            (255, 255, 255)
        } else {
            mix(p.bg[1], (0, 0, 0), 0.3)
        };
        pix.fill_path(
            &path,
            &paint(rgba(base, (if p.light { 0.88 } else { 0.84 }) * alpha)),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        pix.fill_path(
            &path,
            &paint(rgba(color, (0.035 + 0.075 * lit) * alpha)),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        let stroke = Stroke {
            width: self.u * (1.1 + 0.5 * lit),
            ..Stroke::default()
        };
        pix.stroke_path(
            &path,
            &paint(rgba(color, (0.28 + 0.72 * lit) * alpha)),
            &stroke,
            Transform::identity(),
            None,
        );
    }

    fn dot_glow(&self, pix: &mut Pixmap, at: Point, color: Rgb, radius: f32, alpha: f32) {
        if let Some(sh) = RadialGradient::new(
            at,
            at,
            radius,
            vec![
                GradientStop::new(0., rgba(color, 0.55 * alpha)),
                GradientStop::new(1., rgba(color, 0.)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            let paint = Paint {
                shader: sh,
                anti_alias: true,
                ..Paint::default()
            };
            if let Some(r) = Rect::from_xywh(at.x - radius, at.y - radius, radius * 2., radius * 2.)
            {
                pix.fill_rect(r, &paint, Transform::identity(), None);
            }
        }
        if let Some(core) = PathBuilder::from_circle(at.x, at.y, radius * 0.24) {
            pix.fill_path(
                &core,
                &paint(rgba(mix(color, (255, 255, 255), 0.6), alpha)),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    fn flow_geometry(&self, a: Rect, n: usize, layout: Layout) -> (Vec<Placed>, Vec<Link>) {
        let u = self.u;
        let n = n.max(1);
        let mut boxes = vec![];
        let mut links = vec![];
        let pt = Point::from_xy;
        match layout {
            Layout::Row | Layout::Auto => {
                let slot = a.width() / n as f32;
                let s = if self.tech {
                    (slot * 0.58).min(a.height() * 0.4).min(76. * u)
                } else {
                    (slot * 0.64).min(a.height() * 0.4).min(84. * u)
                };
                let cy = a.top() + a.height() * 0.42;
                for i in 0..n {
                    let cx = a.left() + slot * (i as f32 + 0.5);
                    boxes.push(Placed {
                        r: Rect::from_xywh(cx - s / 2., cy - s / 2., s, s).unwrap(),
                        shape: Shape::Tile,
                        label_w: slot * 0.94,
                    });
                }
                for i in 0..n - 1 {
                    links.push(Link::straight(
                        pt(boxes[i].r.right() + 5. * u, cy),
                        pt(boxes[i + 1].r.left() - 5. * u, cy),
                    ));
                }
            }
            Layout::Column => {
                let ch = ((a.height() - 24. * u * (n as f32 - 1.)) / n as f32).min(60. * u);
                let gap = if n > 1 {
                    ((a.height() - ch * n as f32) / (n as f32 - 1.)).clamp(16. * u, 40. * u)
                } else {
                    0.
                };
                let total = ch * n as f32 + gap * (n as f32 - 1.);
                let top = a.top() + (a.height() - total) / 2.;
                let cw = a.width() * 0.84;
                let left = a.left() + (a.width() - cw) / 2.;
                for i in 0..n {
                    boxes.push(Placed {
                        r: Rect::from_xywh(left, top + i as f32 * (ch + gap), cw, ch).unwrap(),
                        shape: Shape::Card,
                        label_w: 0.,
                    });
                }
                let x = left + ch * 0.5;
                for i in 0..n - 1 {
                    links.push(Link::straight(
                        pt(x, boxes[i].r.bottom() + 3. * u),
                        pt(x, boxes[i + 1].r.top() - 3. * u),
                    ));
                }
            }
            Layout::Snake => {
                let rows = n.div_ceil(2);
                let hgap = a.width() * 0.13;
                let cw = (a.width() - hgap) / 2.;
                let vgap = 30. * u;
                let ch = ((a.height() - vgap * (rows as f32 - 1.)) / rows as f32).min(74. * u);
                let total = ch * rows as f32 + vgap * (rows as f32 - 1.);
                let top = a.top() + (a.height() - total) / 2.;
                for i in 0..n {
                    let (row, c) = (i / 2, i % 2);
                    let col = if row % 2 == 0 { c } else { 1 - c };
                    let x = a.left() + col as f32 * (cw + hgap);
                    let y = top + row as f32 * (ch + vgap);
                    boxes.push(Placed {
                        r: Rect::from_xywh(x, y, cw, ch).unwrap(),
                        shape: Shape::Card,
                        label_w: 0.,
                    });
                }
                for i in 0..n - 1 {
                    let (b0, b1) = (boxes[i].r, boxes[i + 1].r);
                    if i / 2 == (i + 1) / 2 {
                        // Across a row: an arc over the gap.
                        let y = b0.top() + b0.height() * 0.4;
                        let right = b1.left() > b0.left();
                        let (x0, x1) = if right {
                            (b0.right() + 5. * u, b1.left() - 5. * u)
                        } else {
                            (b0.left() - 5. * u, b1.right() + 5. * u)
                        };
                        links.push(Link {
                            p0: pt(x0, y),
                            c: pt((x0 + x1) / 2., y - ch * 0.6),
                            p1: pt(x1, y),
                        });
                    } else {
                        let x = b0.left() + b0.width() / 2.;
                        links.push(Link::straight(
                            pt(x, b0.bottom() + 3. * u),
                            pt(x, b1.top() - 3. * u),
                        ));
                    }
                }
            }
            Layout::Cycle => {
                let s = (a.width().min(a.height()) * 0.2).min(64. * u);
                let (cx, cy) = (a.left() + a.width() / 2., a.top() + a.height() * 0.46);
                let rx = (a.width() / 2. - s * 0.5 - 46. * u).max(s);
                let ry = (a.height() / 2. - s * 0.5 - 30. * u).max(s);
                let centres: Vec<Point> = (0..n)
                    .map(|i| {
                        let th = -std::f32::consts::FRAC_PI_2
                            + i as f32 * std::f32::consts::TAU / n as f32;
                        pt(cx + rx * th.cos(), cy + ry * th.sin())
                    })
                    .collect();
                for c in &centres {
                    boxes.push(Placed {
                        r: Rect::from_xywh(c.x - s / 2., c.y - s / 2., s, s).unwrap(),
                        shape: Shape::Tile,
                        label_w: s * 2.4,
                    });
                }
                for i in 0..n {
                    let (a0, a1) = (centres[i], centres[(i + 1) % n]);
                    let (dx, dy) = (a1.x - a0.x, a1.y - a0.y);
                    let d = (dx * dx + dy * dy).sqrt().max(1.);
                    let (ux, uy) = (dx / d, dy / d);
                    let p0 = pt(a0.x + ux * s * 0.72, a0.y + uy * s * 0.72);
                    let p1 = pt(a1.x - ux * s * 0.72, a1.y - uy * s * 0.72);
                    let mid = pt((p0.x + p1.x) / 2., (p0.y + p1.y) / 2.);
                    let (ox, oy) = (mid.x - cx, mid.y - cy);
                    let o = (ox * ox + oy * oy).sqrt().max(1.);
                    links.push(Link {
                        p0,
                        c: pt(mid.x + ox / o * d * 0.16, mid.y + oy / o * d * 0.16),
                        p1,
                    });
                }
            }
        }
        (boxes, links)
    }

    fn arrowhead(&self, pix: &mut Pixmap, link: &Link, color: Rgb, alpha: f32) {
        let tip = link.p1;
        let back = link.at(0.9);
        let (dx, dy) = (tip.x - back.x, tip.y - back.y);
        let d = (dx * dx + dy * dy).sqrt().max(0.001);
        let (ux, uy) = (dx / d, dy / d);
        let s = 5.5 * self.u;
        let mut pb = PathBuilder::new();
        pb.move_to(tip.x, tip.y);
        pb.line_to(tip.x - ux * s - uy * s * 0.6, tip.y - uy * s + ux * s * 0.6);
        pb.line_to(tip.x - ux * s + uy * s * 0.6, tip.y - uy * s - ux * s * 0.6);
        pb.close();
        if let Some(path) = pb.finish() {
            pix.fill_path(
                &path,
                &paint(rgba(color, alpha)),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    pub(super) fn flow(
        &mut self,
        pix: &mut Pixmap,
        t: f32,
        title: Option<&str>,
        nodes: &[Node],
        layout: Layout,
    ) {
        if nodes.is_empty() {
            return;
        }
        let u = self.u;
        let n = nodes.len();
        let len = self.scene.length as f32 / SECOND as f32;
        let tm = flow_timing(n, len);
        let mut a = self.tall_area();
        let p = self.scene.palette;
        let colors = self.step_colors();
        let color = |k: usize| colors[k % 3];
        if let Some(title) = title {
            let e = ease_out((t - 0.05) / 0.4);
            let (cx, cy) = (a.left() + a.width() / 2., a.top() + 12. * u);
            self.pill(pix, title, 10.5, (cx, cy), color(0), 0., e, a.width());
            a = Rect::from_ltrb(a.left(), a.top() + 40. * u, a.right(), a.bottom()).unwrap_or(a);
        }
        let tech = self.tech;
        let (bold, regular) = if tech {
            (MONO_BOLD, MONO)
        } else {
            (SANS_BOLD, SANS)
        };
        // The counter and note line under the diagram: the engineer's look,
        // or nodes that have something to say.
        let notes = tech || nodes.iter().any(|n| n.note.is_some());
        let room = if notes { 54. * u } else { 0. };
        let body = Rect::from_ltrb(a.left(), a.top(), a.right(), a.bottom() - room).unwrap_or(a);
        let portrait = self.h >= self.w;
        let layout = layout.resolve(n, portrait, self.scene.seed);
        let (boxes, links) = self.flow_geometry(body, n, layout);
        let current = tm.active.iter().rposition(|&x| t >= x);
        let ink = self.ink();
        // The cycle's closing link runs once the last node is lit.
        let wrap_at = tm.active[n - 1] + tm.travel + 0.5;

        for (k, link) in links.iter().enumerate() {
            let to = (k + 1) % n;
            let shown = ease_out((t - tm.appear[to].max(tm.appear[k]) - 0.1) / 0.4);
            if shown <= 0. {
                continue;
            }
            let dim = Stroke {
                width: u * if tech { 1.4 } else { 1.8 },
                line_cap: LineCap::Round,
                dash: if tech {
                    StrokeDash::new(vec![4. * u, 5. * u], 0.)
                } else {
                    None
                },
                ..Stroke::default()
            };
            if let Some(path) = link.path(1.) {
                pix.stroke_path(
                    &path,
                    &paint(rgba(ink, 0.2 * shown)),
                    &dim,
                    Transform::identity(),
                    None,
                );
            }
            let arrive = if to == 0 { wrap_at } else { tm.active[to] };
            let prog = clamp01((t - (arrive - tm.travel)) / tm.travel);
            let c = color(to);
            if prog > 0.
                && let Some(path) = link.path(prog)
            {
                self.glow_path(pix, &path, c, 0.45);
                let lit = Stroke {
                    width: u * if tech { 2. } else { 2.6 },
                    line_cap: LineCap::Round,
                    dash: if tech {
                        StrokeDash::new(vec![5. * u, 4. * u], -t * 26. * u)
                    } else {
                        None
                    },
                    ..Stroke::default()
                };
                pix.stroke_path(
                    &path,
                    &paint(rgba(c, 0.95 * shown)),
                    &lit,
                    Transform::identity(),
                    None,
                );
            }
            if prog > 0. && prog < 1. {
                self.dot_glow(pix, link.at(prog), c, 14. * u, shown);
            }
            if prog >= 1. {
                self.arrowhead(pix, link, c, 0.95 * shown);
            } else {
                self.arrowhead(pix, link, ink, 0.22 * shown);
            }
        }

        for (k, node) in nodes.iter().enumerate() {
            let e = ease_out((t - tm.appear[k]) / 0.35);
            if e <= 0. {
                continue;
            }
            let lift = (1. - e) * 14. * u;
            let lit = ease_out((t - tm.active[k]) / 0.25);
            let glow = if t < tm.active[k] {
                0.
            } else if current == Some(k) {
                0.8 + 0.7 * (-(t - tm.active[k]) * 3.).exp() + 0.12 * (t * 4.).sin()
            } else {
                0.3
            };
            let b = &boxes[k];
            let r = Rect::from_xywh(b.r.left(), b.r.top() + lift, b.r.width(), b.r.height())
                .unwrap_or(b.r);
            let c = color(k);
            let word_color = if lit > 0.5 { c } else { p.text };
            let word_alpha = e * (0.6 + 0.4 * lit);
            match b.shape {
                Shape::Tile => {
                    // Rounded squares for tech, round badges otherwise.
                    let radius = if tech {
                        r.width() * 0.24
                    } else {
                        r.width() / 2.
                    };
                    self.glass(pix, r, radius, c, lit, glow, e);
                    let inset = r.width() * if tech { 0.24 } else { 0.25 };
                    if let Some(ir) = Rect::from_ltrb(
                        r.left() + inset,
                        r.top() + inset,
                        r.right() - inset,
                        r.bottom() - inset,
                    ) {
                        node.icon.draw(
                            pix,
                            ir,
                            c,
                            e * (0.55 + 0.45 * lit),
                            (r.width() * 0.05).max(1.5 * u),
                        );
                    }
                    let cx = r.left() + r.width() / 2.;
                    // The plain look wraps a long label on two lines
                    // rather than shrinking it.
                    let (size, line_h) = if tech { (12., 0.) } else { (13.5, 16.) };
                    let label_rows = if tech {
                        vec![node.label.clone()]
                    } else {
                        self.wrap2(&node.label, size, bold, b.label_w)
                    };
                    for (j, row) in label_rows.iter().enumerate() {
                        self.text_in(
                            pix,
                            row,
                            size,
                            word_color,
                            bold,
                            (cx, r.bottom() + (14. + line_h * j as f32) * u),
                            Anchor::Center,
                            word_alpha,
                            b.label_w,
                        );
                    }
                    if let Some(d) = &node.detail {
                        let below = line_h * (label_rows.len() as f32 - 1.);
                        let (ds, dh) = if tech { (9., 12.) } else { (10.5, 14.) };
                        let rows = self.wrap2(d, ds, regular, b.label_w);
                        for (j, row) in rows.iter().enumerate() {
                            self.text_in(
                                pix,
                                row,
                                ds,
                                p.text,
                                regular,
                                (cx, r.bottom() + (30. + below + dh * j as f32) * u),
                                Anchor::Center,
                                e * 0.62,
                                b.label_w,
                            );
                        }
                    }
                }
                Shape::Card => {
                    self.glass(pix, r, 12. * u, c, lit, glow, e);
                    let side = r.height() * 0.62;
                    let tile = Rect::from_xywh(
                        r.left() + r.height() * 0.19,
                        r.top() + (r.height() - side) / 2.,
                        side,
                        side,
                    )
                    .unwrap_or(r);
                    let radius = if tech { side * 0.24 } else { side / 2. };
                    self.glass(pix, tile, radius, c, lit, 0., e);
                    let inset = side * 0.2;
                    if let Some(ir) = Rect::from_ltrb(
                        tile.left() + inset,
                        tile.top() + inset,
                        tile.right() - inset,
                        tile.bottom() - inset,
                    ) {
                        node.icon.draw(
                            pix,
                            ir,
                            c,
                            e * (0.55 + 0.45 * lit),
                            (side * 0.06).max(1.4 * u),
                        );
                    }
                    let x0 = tile.right() + r.height() * 0.2;
                    let room = r.right() - x0 - 12. * u;
                    let cy = r.top() + r.height() / 2.;
                    let (ls, ds, dh) = if tech {
                        (11.5, 9., 12.)
                    } else {
                        (14., 10.5, 14.)
                    };
                    let rows = match &node.detail {
                        Some(d) => self.wrap2(d, ds, regular, room),
                        None => vec![],
                    };
                    // The label over its detail, the block centred.
                    let block = (ls + 2.5) * u + rows.len() as f32 * dh * u;
                    let label_y = cy - block / 2. + (ls / 2. + 1.) * u;
                    let label = if tech {
                        node.label.to_uppercase()
                    } else {
                        node.label.clone()
                    };
                    self.text_in(
                        pix,
                        &label,
                        ls,
                        word_color,
                        bold,
                        (x0, label_y),
                        Anchor::Left,
                        word_alpha,
                        room,
                    );
                    for (j, row) in rows.iter().enumerate() {
                        self.text_in(
                            pix,
                            row,
                            ds,
                            p.text,
                            regular,
                            (x0, label_y + (ls + 4.5 + dh * j as f32) * u),
                            Anchor::Left,
                            e * 0.66,
                            room,
                        );
                    }
                    if !tech {
                        continue;
                    }
                    self.text_in(
                        pix,
                        &format!("{:02}", k + 1),
                        7.5,
                        p.text,
                        MONO,
                        (r.right() - 12. * u, r.top() + 9. * u),
                        Anchor::Center,
                        e * 0.35,
                        30. * u,
                    );
                }
            }
        }

        if !notes {
            return;
        }
        if !tech {
            // The plain look: the lit node's note alone, in its own type.
            let cx = a.left() + a.width() / 2.;
            if let Some(k) = current
                && let Some(note) = &nodes[k].note
            {
                let e = ease_out((t - tm.active[k]) / 0.3);
                let y = a.bottom() - room + 26. * u + (1. - e) * 6. * u;
                let rows = self.wrap2(note, 13., SANS, a.width() * 0.92);
                for (j, row) in rows.iter().enumerate() {
                    self.text_in(
                        pix,
                        row,
                        13.,
                        p.text,
                        SANS,
                        (cx, y + 17. * u * j as f32),
                        Anchor::Center,
                        e,
                        a.width() * 0.96,
                    );
                }
            }
            return;
        }
        // The lit step's note, and the progress dots.
        let cx = a.left() + a.width() / 2.;
        let note_y = a.bottom() - room + 22. * u;
        if let Some(k) = current {
            let e = ease_out((t - tm.active[k]) / 0.3);
            let note = node_note(&nodes[k]);
            let num = format!("{} · ", k + 1);
            let (nw, tw) = (
                self.width_of(&num, 11., MONO_BOLD),
                self.width_of(&note, 11., MONO),
            );
            let s = ((a.width() * 0.96) / (nw + tw).max(1.)).min(1.);
            let left = cx - (nw + tw) * s / 2.;
            let y = note_y + (1. - e) * 6. * u;
            self.text_in(
                pix,
                &num,
                11.,
                color(k),
                MONO_BOLD,
                (left, y),
                Anchor::Left,
                e,
                nw * s + 1.,
            );
            self.text_in(
                pix,
                &note,
                11.,
                p.text,
                MONO,
                (left + nw * s, y),
                Anchor::Left,
                e,
                tw * s + 1.,
            );
        }
        let dots_y = note_y + 22. * u;
        let gap = 13. * u;
        let show = ease_out((t - tm.appear[n - 1]) / 0.4);
        for k in 0..n {
            let x = cx + (k as f32 - (n as f32 - 1.) / 2.) * gap;
            let c = color(k);
            let path = |r: f32| PathBuilder::from_circle(x, dots_y, r);
            match current {
                Some(cur) if k == cur => {
                    self.dot_glow(pix, Point::from_xy(x, dots_y), c, 10. * u, show * 0.6);
                    if let Some(d) = path(4.2 * u) {
                        pix.fill_path(
                            &d,
                            &paint(rgba(c, show)),
                            FillRule::Winding,
                            Transform::identity(),
                            None,
                        );
                    }
                }
                Some(cur) if k < cur => {
                    if let Some(d) = path(3. * u) {
                        pix.fill_path(
                            &d,
                            &paint(rgba(c, 0.55 * show)),
                            FillRule::Winding,
                            Transform::identity(),
                            None,
                        );
                    }
                }
                _ => {
                    if let Some(d) = path(3. * u) {
                        let stroke = Stroke {
                            width: u,
                            ..Stroke::default()
                        };
                        pix.stroke_path(
                            &d,
                            &paint(rgba(ink, 0.35 * show)),
                            &stroke,
                            Transform::identity(),
                            None,
                        );
                    }
                }
            }
        }
    }

    pub(super) fn code(
        &mut self,
        pix: &mut Pixmap,
        t: f32,
        title: Option<&str>,
        file: Option<&str>,
        lines: &[String],
        steps: &[CodeStep],
    ) {
        let u = self.u;
        let p = self.scene.palette;
        let ink = self.ink();
        let len = self.scene.length as f32 / SECOND as f32;
        let tm = code_timing(lines.len(), steps.len(), len);
        let colors = self.step_colors();
        let color = |k: usize| colors[k % 3];
        let mut a = self.tall_area();
        if let Some(title) = title {
            let e = ease_out((t - 0.05) / 0.4);
            let (cx, cy) = (a.left() + a.width() / 2., a.top() + 12. * u);
            self.pill(pix, title, 10.5, (cx, cy), color(0), 0., e, a.width());
            a = Rect::from_ltrb(a.left(), a.top() + 40. * u, a.right(), a.bottom()).unwrap_or(a);
        }
        let ns = steps.len();
        let below = if ns > 0 { 70. * u } else { 0. };
        let show = ease_out((t - 0.05) / 0.35);
        let Some(panel) = Rect::from_ltrb(
            a.left(),
            a.top() + (1. - show) * 16. * u,
            a.right(),
            a.bottom() - below,
        ) else {
            return;
        };
        // The window.
        let roundness = (12. * u / (panel.height().min(panel.width()) / 2.)).min(1.);
        if let Some(path) = text_render::rounded(panel, roundness) {
            let base = if p.light {
                (255, 255, 255)
            } else {
                (9, 12, 20)
            };
            pix.fill_path(
                &path,
                &paint(rgba(base, 0.93 * show)),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
            let stroke = Stroke {
                width: u,
                ..Stroke::default()
            };
            pix.stroke_path(
                &path,
                &paint(rgba(ink, 0.12 * show)),
                &stroke,
                Transform::identity(),
                None,
            );
        }
        let bar = 24. * u;
        for (i, c) in [(255, 95, 86), (255, 189, 46), (39, 201, 63)]
            .into_iter()
            .enumerate()
        {
            if let Some(d) = PathBuilder::from_circle(
                panel.left() + 15. * u + i as f32 * 11. * u,
                panel.top() + bar / 2.,
                3.4 * u,
            ) {
                pix.fill_path(
                    &d,
                    &paint(rgba(c, show)),
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
        if let Some(f) = file {
            self.text_in(
                pix,
                f,
                8.5,
                p.text,
                MONO,
                (panel.left() + panel.width() / 2., panel.top() + bar / 2.),
                Anchor::Center,
                0.55 * show,
                panel.width() * 0.6,
            );
        }
        let mut pb = PathBuilder::new();
        pb.move_to(panel.left(), panel.top() + bar);
        pb.line_to(panel.right(), panel.top() + bar);
        if let Some(line) = pb.finish() {
            pix.stroke_path(
                &line,
                &paint(rgba(ink, 0.07 * show)),
                &Stroke {
                    width: u,
                    ..Stroke::default()
                },
                Transform::identity(),
                None,
            );
        }
        // The lines, all at one size: the longest fits.
        let n = lines.len().max(1);
        let top = panel.top() + bar + 10. * u;
        let bottom = panel.bottom() - 10. * u;
        let lh = ((bottom - top) / n as f32).min(22. * u);
        let gutter = 28. * u;
        let x0 = panel.left() + gutter + 8. * u;
        let code_w = panel.right() - x0 - 12. * u;
        let w10 = self.width_of("MMMMMMMMMM", 10., MONO);
        let w20 = self.width_of("MMMMMMMMMMMMMMMMMMMM", 10., MONO);
        let adv10 = ((w20 - w10) / 10.).max(1.);
        let pad10 = (w10 - adv10 * 10.).max(0.);
        let longest = lines
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(1)
            .max(1);
        let size = (10. * code_w / (longest as f32 * adv10))
            .min(lh / (1.35 * u))
            .clamp(5.5, 11.);
        let adv = adv10 * size / 10.;
        let pad = pad10 * size / 10. / 2.;
        let fits = (code_w / adv).floor() as usize;
        let current = tm.steps.iter().rposition(|&x| t >= x);
        let comment = mix(p.text, p.bg[1], 0.5);
        for (i, line) in lines.iter().enumerate() {
            let e = ease_out((t - tm.line_at[i]) / 0.25);
            if e <= 0. {
                continue;
            }
            let y = top + lh * (i as f32 + 0.5);
            let step_here = current.filter(|&k| steps[k].from <= i + 1 && i + 1 <= steps[k].to);
            let dim = if current.is_some() && step_here.is_none() {
                0.38
            } else {
                1.
            };
            if let Some(k) = step_here {
                let he = ease_out((t - tm.steps[k]) / 0.3);
                let c = color(k);
                if let Some(band) =
                    Rect::from_xywh(panel.left() + 1., y - lh / 2., panel.width() - 2., lh)
                {
                    pix.fill_rect(
                        band,
                        &paint(rgba(c, 0.13 * he)),
                        Transform::identity(),
                        None,
                    );
                }
                if let Some(edge) = Rect::from_xywh(panel.left() + 1., y - lh / 2., 3. * u, lh) {
                    pix.fill_rect(edge, &paint(rgba(c, he)), Transform::identity(), None);
                }
            }
            self.text_in(
                pix,
                &(i + 1).to_string(),
                size * 0.85,
                p.text,
                MONO,
                (panel.left() + gutter / 2. + 4. * u, y),
                Anchor::Center,
                0.3 * e * dim,
                gutter,
            );
            let slide = (1. - e) * 10. * u;
            for (col, text, kind) in tokenize(line) {
                if col >= fits {
                    break;
                }
                let text: String = text.chars().take(fits - col).collect();
                let (c, al) = match kind {
                    Tok::Keyword => (color(2), 1.),
                    Tok::Func => (color(0), 1.),
                    Tok::Str => (color(1), 1.),
                    Tok::Num => (color(2), 1.),
                    Tok::Comment => (comment, 0.9),
                    Tok::Plain | Tok::Punct => (p.text, 0.92),
                };
                self.text_in(
                    pix,
                    &text,
                    size,
                    c,
                    MONO,
                    (x0 + col as f32 * adv - pad + slide, y),
                    Anchor::Left,
                    al * e * dim,
                    f32::MAX,
                );
            }
        }
        // A cursor blinking where the typing is.
        if t < tm.type_to + 0.8 {
            let shown = tm.line_at.iter().rposition(|&x| t >= x);
            if let Some(i) = shown {
                let chars = lines[i].chars().count().min(fits) as f32;
                let y = top + lh * (i as f32 + 0.5);
                let on = (t * 4.).sin() > -0.2;
                if on
                    && let Some(r) = Rect::from_xywh(
                        x0 + chars * adv + 2. * u,
                        y - lh * 0.32,
                        adv * 0.55,
                        lh * 0.64,
                    )
                {
                    pix.fill_rect(r, &paint(rgba(color(0), 0.85)), Transform::identity(), None);
                }
            }
        }
        if ns == 0 {
            return;
        }
        // The step pills, and the lit step's note.
        let gap = 8. * u;
        let pw = (a.width() - gap * (ns as f32 - 1.)) / ns as f32;
        let py = panel.bottom() + 24. * u;
        let pills_in = ease_out((t - tm.type_to) / 0.4);
        for (k, step) in steps.iter().enumerate() {
            let cx = a.left() + k as f32 * (pw + gap) + pw / 2.;
            let c = color(k);
            let (lit, al) = match current {
                Some(cur) if cur == k => (ease_out((t - tm.steps[k]) / 0.3), 1.),
                Some(cur) if k < cur => (0., 0.75),
                _ => (0., 0.45),
            };
            let label = format!("{}  {}", k + 1, step.label.to_uppercase());
            self.pill(pix, &label, 9., (cx, py), c, lit, pills_in * al, pw);
        }
        if let Some(k) = current {
            let e = ease_out((t - tm.steps[k]) / 0.3);
            let note = steps[k]
                .note
                .clone()
                .unwrap_or_else(|| steps[k].label.clone());
            self.text_in(
                pix,
                &note,
                11.,
                p.text,
                MONO,
                (a.left() + a.width() / 2., py + 30. * u + (1. - e) * 5. * u),
                Anchor::Center,
                e,
                a.width(),
            );
        }
    }
}

fn node_note(node: &Node) -> String {
    node.note.clone().unwrap_or_else(|| match &node.detail {
        Some(d) => format!("{} · {}", node.label, d),
        None => node.label.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_suit_the_count() {
        for seed in 0..20 {
            let l = Layout::Auto.resolve(6, true, seed);
            assert!(matches!(l, Layout::Snake | Layout::Column), "{l:?}");
            assert_ne!(Layout::Auto.resolve(3, true, seed), Layout::Cycle);
        }
        // Different seeds pick different layouts.
        let picks: std::collections::HashSet<_> =
            (0..30).map(|s| Layout::Auto.resolve(4, true, s)).collect();
        assert!(picks.len() >= 2, "{picks:?}");
        assert_eq!(Layout::Cycle.resolve(4, true, 0), Layout::Cycle);
        assert_eq!(Layout::Row.resolve(7, true, 0), Layout::Snake);
        assert_eq!(Layout::parse("zigzag"), Some(Layout::Snake));
    }

    #[test]
    fn timings_fit_the_scene() {
        let tm = flow_timing(6, 10.);
        assert_eq!(tm.active.len(), 6);
        assert!(tm.active.windows(2).all(|w| w[1] > w[0]));
        assert!(*tm.active.last().unwrap() < 10.);
        let ct = code_timing(8, 3, 9.);
        assert!(ct.steps[0] > ct.type_to);
        assert!(*ct.steps.last().unwrap() < 9.);
    }

    #[test]
    fn colours_code() {
        let toks = tokenize("docs = store.search(query, k=3)  # top 3");
        let kinds: Vec<Tok> = toks.iter().map(|t| t.2).collect();
        assert!(kinds.contains(&Tok::Func));
        assert!(kinds.contains(&Tok::Num));
        assert_eq!(toks.last().unwrap().2, Tok::Comment);
        let s = tokenize("prompt = f\"Use {docs}\"");
        assert_eq!(
            s.last().unwrap(),
            &(9, "f\"Use {docs}\"".to_string(), Tok::Str)
        );
        let k = tokenize("def run(): return 1");
        assert_eq!(k[0], (0, "def".to_string(), Tok::Keyword));
    }
}
