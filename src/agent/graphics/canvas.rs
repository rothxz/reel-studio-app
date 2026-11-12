//! A free scene: instead of picking one of the ready graphics, the model
//! composes the picture from elements (words, a number counting up, an
//! icon, a shape, a progress bar, a card, a line joining two of them) and
//! says where each sits, when it comes in and how, where it moves and when
//! it leaves.
//!
//! What isn't said is decided here, so a short list still makes a finished
//! scene: elements with no place are laid out (a stack, a row or a grid),
//! ones with no time come in one after another across the scene, and each
//! kind has its own way in (words rise, icons pop, lines draw themselves,
//! numbers count).
//!
//! An element's state at `t` is worked out from scratch (its entrance, its
//! moves in order, its exit), never carried from the frame before: frames
//! draw on any thread in any order.

use serde_json::{Map, Value};
use tiny_skia::{
    FillRule, FilterQuality, LineCap, PathBuilder, Pixmap, PixmapPaint, Point, Rect, Stroke,
    Transform,
};

use super::{Icon, Painter, Rgb, clamp01, decimals_for, ease_back, ease_out, format_value, paint};
use super::{rgba, text_render};

const SANS: &str = "Nunito_400Regular";
const SANS_BOLD: &str = "Nunito_700Bold";
const MONO: &str = "IBM Plex Mono";

/// Elements a scene takes at most (more is a wall, and slow to draw).
pub const MAX_ELEMENTS: usize = 24;

pub const ENTRANCES: &[&str] = &[
    "fade",
    "rise",
    "drop",
    "slide_left",
    "slide_right",
    "pop",
    "grow",
    "spin",
    "draw",
    "type",
    "words",
    "none",
];
pub const EXITS: &[&str] = &["fade", "fall", "shrink", "slide_left", "slide_right", "cut"];
pub const LOOPS: &[&str] = &["float", "pulse", "spin"];
pub const SHAPES: &[&str] = &["circle", "ring", "rect", "pill", "triangle", "star", "line"];
pub const FLOWS: &[&str] = &["stack", "row", "grid"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tint {
    Accent,
    Accent2,
    Accent3,
    Text,
    Muted,
    Custom(Rgb),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Circle,
    Ring,
    Rect,
    Pill,
    Triangle,
    Star,
    Line,
}

/// One end of a joining line: another element, or a spot on the stage.
#[derive(Clone, Debug, PartialEq)]
pub enum End {
    Id(String),
    At(f32, f32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum What {
    Text {
        text: String,
        bold: bool,
        mono: bool,
    },
    Number {
        from: f64,
        to: f64,
        prefix: String,
        suffix: String,
    },
    Icon(Icon),
    Shape {
        shape: Shape,
        /// Width over height.
        ratio: f32,
        filled: bool,
    },
    /// A progress bar filled to `value` (0–1).
    Bar {
        value: f32,
        shown: String,
    },
    Card {
        text: String,
        detail: Option<String>,
        icon: Option<Icon>,
    },
    Link {
        from: End,
        to: End,
        arrow: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enter {
    /// The kind's own way in.
    Auto,
    None,
    Fade,
    Rise,
    Drop,
    SlideLeft,
    SlideRight,
    Pop,
    Grow,
    Spin,
    Draw,
    Type,
    Words,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Fade,
    Fall,
    Shrink,
    SlideLeft,
    SlideRight,
    Cut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Idle {
    Still,
    Float,
    Pulse,
    Spin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ease {
    Out,
    In,
    InOut,
    Back,
    Expo,
    Linear,
}

/// From `at`, over `len` seconds, the element goes to the values given
/// (x / y on the stage 0–1, scale, degrees, opacity) and stays there.
#[derive(Clone, Debug, PartialEq)]
pub struct Move {
    pub at: f32,
    pub len: f32,
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub scale: Option<f32>,
    pub rotate: Option<f32>,
    pub opacity: Option<f32>,
    pub ease: Ease,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub id: Option<String>,
    pub what: What,
    /// Its centre on the stage, 0–1; None: the scene lays it out.
    pub at: Option<(f32, f32)>,
    /// Words and numbers: text units. Everything else: a share of the
    /// stage's short side.
    pub size: f32,
    /// The widest it may be, a share of the stage's width.
    pub width: Option<f32>,
    pub tint: Tint,
    /// A small line under a number, an icon, a shape; over a bar.
    pub label: Option<String>,
    /// Seconds into the scene; None: in turn.
    pub start: Option<f32>,
    pub end: Option<f32>,
    pub enter: Enter,
    pub exit: Exit,
    pub moves: Vec<Move>,
    pub idle: Idle,
}

/// How the elements with no place of their own are arranged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Stack,
    Row,
    Grid,
}

impl Flow {
    pub fn parse(s: &str) -> Option<Flow> {
        match norm(s).as_str() {
            "stack" | "column" | "vertical" | "list" | "auto" | "center" => Some(Flow::Stack),
            "row" | "horizontal" | "line" => Some(Flow::Row),
            "grid" | "tiles" | "cards" => Some(Flow::Grid),
            _ => None,
        }
    }
}

// ---- reading what the model wrote ---------------------------------------------------------

fn norm(s: &str) -> String {
    s.trim().to_lowercase().replace([' ', '-'], "_")
}

fn text_of(o: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| match o.get(*k)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => crate::agent::tools::parse_number(s),
        _ => None,
    }
}

fn num_of(o: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| number(o.get(*k)?))
}

fn flag(o: &Map<String, Value>, key: &str) -> Option<bool> {
    match o.get(key)? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match norm(s).as_str() {
            "true" | "yes" | "on" => Some(true),
            "false" | "no" | "off" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// A share written 0–1, or as a percent (35 → 0.35).
fn share(v: f64) -> f32 {
    (if v.abs() > 2. { v / 100. } else { v }) as f32
}

/// A spot by name, "x,y", [x, y] or {x, y}.
fn point(v: &Value) -> Option<(f32, f32)> {
    match v {
        Value::Array(a) if a.len() >= 2 => Some((share(number(&a[0])?), share(number(&a[1])?))),
        Value::Object(o) => Some((share(num_of(o, &["x"])?), share(num_of(o, &["y"])?))),
        Value::String(s) => {
            if let Some((x, y)) = s.split_once(',') {
                return Some((
                    share(crate::agent::tools::parse_number(x)?),
                    share(crate::agent::tools::parse_number(y)?),
                ));
            }
            Some(match norm(s).as_str() {
                "center" | "centre" | "middle" => (0.5, 0.5),
                "top" => (0.5, 0.12),
                "bottom" => (0.5, 0.88),
                "left" => (0.18, 0.5),
                "right" => (0.82, 0.5),
                "top_left" => (0.2, 0.14),
                "top_right" => (0.8, 0.14),
                "bottom_left" => (0.2, 0.86),
                "bottom_right" => (0.8, 0.86),
                _ => return None,
            })
        }
        _ => None,
    }
}

fn tint(s: &str) -> Option<Tint> {
    let n = norm(s);
    Some(match n.as_str() {
        "accent" | "primary" | "accent1" | "main" => Tint::Accent,
        "accent2" | "secondary" => Tint::Accent2,
        "accent3" | "tertiary" => Tint::Accent3,
        "text" | "foreground" | "ink" => Tint::Text,
        "muted" | "dim" | "gray" | "grey" | "soft" => Tint::Muted,
        "white" => Tint::Custom((255, 255, 255)),
        "black" => Tint::Custom((16, 16, 20)),
        "red" => Tint::Custom((248, 113, 113)),
        "orange" => Tint::Custom((251, 146, 60)),
        "yellow" | "gold" => Tint::Custom((250, 204, 21)),
        "green" => Tint::Custom((74, 222, 128)),
        "teal" | "cyan" => Tint::Custom((45, 212, 191)),
        "blue" => Tint::Custom((96, 165, 250)),
        "purple" | "violet" => Tint::Custom((192, 132, 252)),
        "pink" => Tint::Custom((244, 114, 182)),
        _ => {
            let h = n.strip_prefix('#')?;
            let full = match h.len() {
                3 => h.chars().flat_map(|c| [c, c]).collect::<String>(),
                6 => h.to_string(),
                _ => return None,
            };
            let v = u32::from_str_radix(&full, 16).ok()?;
            Tint::Custom(((v >> 16) as u8, (v >> 8) as u8, v as u8))
        }
    })
}

fn enter(s: &str) -> Option<Enter> {
    Some(match norm(s).as_str() {
        "auto" | "default" => Enter::Auto,
        "none" | "cut" | "instant" => Enter::None,
        "fade" | "fade_in" | "appear" => Enter::Fade,
        "rise" | "slide_up" | "slide_in_up" | "up" | "fade_up" => Enter::Rise,
        "drop" | "slide_down" | "slide_in_down" | "down" | "fall" => Enter::Drop,
        "slide_left" | "from_left" | "slide_in_left" => Enter::SlideLeft,
        "slide_right" | "from_right" | "slide_in_right" => Enter::SlideRight,
        "pop" | "pop_in" | "bounce" | "scale_in" => Enter::Pop,
        "grow" | "zoom" | "zoom_in" | "scale" => Enter::Grow,
        "spin" | "spin_in" | "rotate" | "rotate_in" => Enter::Spin,
        "draw" | "draw_in" | "wipe" | "reveal" => Enter::Draw,
        "type" | "typewriter" | "typing" => Enter::Type,
        "words" | "word_by_word" | "stagger" => Enter::Words,
        _ => return None,
    })
}

fn exit(s: &str) -> Option<Exit> {
    Some(match norm(s).as_str() {
        "fade" | "fade_out" => Exit::Fade,
        "fall" | "drop" | "slide_down" => Exit::Fall,
        "shrink" | "pop_out" | "scale_out" | "zoom_out" => Exit::Shrink,
        "slide_left" | "to_left" => Exit::SlideLeft,
        "slide_right" | "to_right" => Exit::SlideRight,
        "cut" | "none" | "instant" => Exit::Cut,
        _ => return None,
    })
}

fn ease(s: &str) -> Ease {
    match norm(s).replace('_', "").as_str() {
        "in" | "easein" | "accelerate" => Ease::In,
        "inout" | "easeinout" | "smooth" => Ease::InOut,
        "back" | "bounce" | "spring" | "overshoot" | "elastic" => Ease::Back,
        "expo" | "expoout" | "snap" => Ease::Expo,
        "linear" | "none" => Ease::Linear,
        _ => Ease::Out,
    }
}

/// Words and numbers: a size by name or in text units.
fn text_size(v: Option<&Value>, default: f32) -> f32 {
    match v {
        Some(Value::String(s)) => match norm(s).as_str() {
            "hero" | "huge" | "xl" | "giant" => 60.,
            "large" | "big" | "l" => 40.,
            "medium" | "m" | "normal" => 27.,
            "small" | "s" => 18.,
            "tiny" | "xs" => 13.5,
            other => crate::agent::tools::parse_number(other)
                .map_or(default, |n| (n as f32).clamp(9., 110.)),
        },
        Some(v) => number(v).map_or(default, |n| {
            // A share of the stage rather than a type size.
            let n = n as f32;
            (if n <= 1.5 { 20. + n * 50. } else { n }).clamp(9., 110.)
        }),
        None => default,
    }
}

/// Icons, shapes, cards: a size by name or a share of the stage.
fn box_size(v: Option<&Value>, default: f32) -> f32 {
    match v {
        Some(Value::String(s)) => match norm(s).as_str() {
            "hero" | "huge" | "xl" | "giant" => 0.82,
            "large" | "big" | "l" => 0.5,
            "medium" | "m" | "normal" => 0.26,
            "small" | "s" => 0.15,
            "tiny" | "xs" => 0.08,
            other => crate::agent::tools::parse_number(other)
                .map_or(default, |n| share(n).clamp(0.02, 1.2)),
        },
        Some(v) => number(v).map_or(default, |n| share(n).clamp(0.02, 1.2)),
        None => default,
    }
}

/// The icon a name means: the icon of that name, else the one its words
/// suggest ("bee" → leaf), else the one the element's own words do. A miss
/// is said, but isn't worth a second try: the scene is drawn all the same.
fn icon_named(name: &str, about: &str, tech: bool, notes: &mut Vec<String>) -> Icon {
    if let Some(icon) = Icon::parse(name) {
        return icon;
    }
    let guess = Icon::suggest(name, "", tech)
        .or_else(|| Icon::suggest(name, "", !tech))
        .or_else(|| Icon::suggest(about, "", tech))
        .unwrap_or(Icon::Sparkle);
    let drawn = Icon::names()
        .into_iter()
        .find(|n| Icon::parse(n) == Some(guess))
        .unwrap_or("sparkle");
    notes.push(format!(
        "There is no icon \"{name}\": the {drawn} icon is drawn instead."
    ));
    guess
}

fn read_end(v: &Value) -> Option<End> {
    match v {
        Value::String(s) if s.contains(',') => point(v).map(|(x, y)| End::At(x, y)),
        Value::String(s) if !s.trim().is_empty() => Some(End::Id(norm(s))),
        v => point(v).map(|(x, y)| End::At(x, y)),
    }
}

fn read_moves(o: &Map<String, Value>) -> Vec<Move> {
    let raw: Vec<&Value> = ["moves", "then", "keyframes", "move"]
        .iter()
        .find_map(|k| match o.get(*k)? {
            Value::Array(a) => Some(a.iter().collect()),
            v @ Value::Object(_) => Some(vec![v]),
            _ => None,
        })
        .unwrap_or_default();
    let mut moves: Vec<Move> = raw
        .into_iter()
        .filter_map(|v| {
            let m = v.as_object()?;
            let to = m.get("to").or_else(|| m.get("position")).and_then(point);
            let mv = Move {
                at: num_of(m, &["at", "time", "start", "t"])
                    .unwrap_or(0.)
                    .max(0.) as f32,
                len: num_of(m, &["duration", "over", "len"])
                    .unwrap_or(0.6)
                    .clamp(0.05, 30.) as f32,
                x: num_of(m, &["x"]).map(share).or(to.map(|p| p.0)),
                y: num_of(m, &["y"]).map(share).or(to.map(|p| p.1)),
                scale: num_of(m, &["scale", "size"]).map(|s| (s as f32).clamp(0., 8.)),
                rotate: num_of(m, &["rotate", "rotation", "angle"]).map(|r| r as f32),
                opacity: num_of(m, &["opacity", "alpha"]).map(|a| clamp01(share(a))),
                ease: text_of(m, &["ease", "easing"]).map_or(Ease::InOut, |e| ease(&e)),
            };
            (mv.x.is_some()
                || mv.y.is_some()
                || mv.scale.is_some()
                || mv.rotate.is_some()
                || mv.opacity.is_some())
            .then_some(mv)
        })
        .collect();
    moves.sort_by(|a, b| a.at.total_cmp(&b.at));
    moves.truncate(8);
    moves
}

/// What kind of element an object is: its `type`, else the key it carries.
fn read_what(
    o: &Map<String, Value>,
    tech: bool,
    notes: &mut Vec<String>,
) -> Result<(What, &'static str), String> {
    let kind = text_of(o, &["type", "kind"]).map(|k| norm(&k));
    let is = |names: &[&str]| kind.as_deref().is_some_and(|k| names.contains(&k));
    let has = |k: &str| o.get(k).is_some_and(|v| !v.is_null());
    let label_text = || text_of(o, &["text", "title", "label", "content"]);

    if is(&["line", "arrow", "link", "connector"]) || has("line") || has("arrow") || has("link") {
        let ends = ["line", "arrow", "link", "connect"]
            .iter()
            .find_map(|k| o.get(*k).and_then(Value::as_array))
            .filter(|a| a.len() >= 2)
            .map(|a| (read_end(&a[0]), read_end(&a[1])))
            .or_else(|| {
                Some((
                    o.get("from").and_then(read_end),
                    o.get("to").and_then(read_end),
                ))
            });
        return match ends {
            Some((Some(from), Some(to))) => Ok((
                What::Link {
                    from,
                    to,
                    arrow: flag(o, "arrow").unwrap_or(!is(&["line"]) || has("arrow")),
                },
                "line",
            )),
            _ => Err("a line joins two ends: {\"line\": [\"a\", \"b\"]} (ids of other elements) or [[0.2, 0.5], [0.8, 0.5]]".into()),
        };
    }
    if is(&["icon"]) || has("icon") && !is(&["card", "box"]) && !has("card") {
        let name = text_of(o, &["icon", "name"]).ok_or("an icon needs its name")?;
        let about = text_of(o, &["label", "caption"]).unwrap_or_default();
        return Ok((What::Icon(icon_named(&name, &about, tech, notes)), "icon"));
    }
    if is(&["bar", "progress", "meter"]) || has("bar") || has("progress") {
        let raw = num_of(o, &["bar", "progress", "value", "percent"])
            .ok_or("a bar needs its value: {\"bar\": 0.72} or {\"bar\": 72}")?;
        let max = num_of(o, &["max", "of"]).filter(|m| *m > 0.);
        let value = match max {
            Some(m) => (raw / m) as f32,
            None => share(raw),
        };
        let shown = text_of(o, &["display", "shown"]).unwrap_or_else(|| match max {
            Some(_) => format_value(raw, decimals_for(&[raw]), "", ""),
            None => format!("{}%", (clamp01(value) * 100.).round()),
        });
        return Ok((
            What::Bar {
                value: clamp01(value),
                shown,
            },
            "bar",
        ));
    }
    if is(&["card", "box", "panel", "node"]) || has("card") || has("box") {
        let text = text_of(o, &["card", "box", "text", "title", "label"])
            .ok_or("a card needs its words: {\"card\": \"Step 1\"}")?;
        return Ok((
            What::Card {
                text: text.clone(),
                detail: text_of(o, &["detail", "subtitle", "note", "body"]),
                icon: text_of(o, &["icon"]).map(|n| icon_named(&n, &text, tech, notes)),
            },
            "card",
        ));
    }
    if is(&["shape"]) || has("shape") || kind.as_deref().is_some_and(|k| SHAPES.contains(&k)) {
        let name = text_of(o, &["shape"])
            .map(|s| norm(&s))
            .or(kind.clone())
            .unwrap_or_default();
        let shape = match name.as_str() {
            "circle" | "dot" | "disc" | "ball" => Shape::Circle,
            "ring" | "donut" | "outline" => Shape::Ring,
            "rect" | "rectangle" | "square" | "box" | "block" => Shape::Rect,
            "pill" | "capsule" | "button" => Shape::Pill,
            "triangle" => Shape::Triangle,
            "star" => Shape::Star,
            "line" | "rule" | "divider" | "underline" => Shape::Line,
            other => {
                return Err(format!(
                    "no shape \"{other}\": one of {}",
                    SHAPES.join(", ")
                ));
            }
        };
        let ratio = num_of(o, &["ratio", "aspect"]).map_or(
            match (shape, name.as_str()) {
                (Shape::Rect, "square") => 1.,
                (Shape::Rect, _) => 1.6,
                (Shape::Pill, _) => 3.,
                _ => 1.,
            },
            |r| (r as f32).clamp(0.2, 8.),
        );
        return Ok((
            What::Shape {
                shape,
                ratio,
                filled: flag(o, "fill")
                    .or(flag(o, "filled"))
                    .unwrap_or(shape != Shape::Ring),
            },
            "shape",
        ));
    }
    let wants_number = is(&["number", "counter", "count", "stat"]) || has("number");
    if wants_number || (has("to") && !has("text")) {
        let to = num_of(o, &["number", "to", "value"])
            .ok_or("a number needs its value: {\"number\": 87, \"suffix\": \"%\"}")?;
        return Ok((
            What::Number {
                from: num_of(o, &["from"]).unwrap_or(0.),
                to,
                prefix: text_of(o, &["prefix"]).unwrap_or_default(),
                suffix: text_of(o, &["suffix", "unit"]).unwrap_or_default(),
            },
            "number",
        ));
    }
    match label_text() {
        Some(text) => Ok((
            What::Text {
                text,
                bold: true,
                mono: flag(o, "mono").unwrap_or(false),
            },
            "text",
        )),
        None => Err(
            "each element is one of {\"text\"}, {\"number\"}, {\"icon\"}, {\"shape\"}, {\"bar\"}, {\"card\"}, {\"line\"}"
                .into(),
        ),
    }
}

/// The scene's elements from a call's `elements` list, and what was read
/// forgivingly or left out (each with its fix). `tech`: a technical topic
/// (it picks the icons a vague name means).
pub fn read(raw: &[Value], tech: bool) -> Result<(Vec<Element>, Vec<String>), String> {
    const EXAMPLE: &str = "[{\"number\": 87, \"suffix\": \"%\", \"label\": \"finish the course\"}, {\"icon\": \"trophy\"}, {\"text\": \"Small steps win\", \"in\": \"words\"}]";
    if raw.is_empty() {
        return Err(format!("canvas needs elements, like {EXAMPLE}"));
    }
    let mut notes = vec![];
    if raw.len() > MAX_ELEMENTS {
        notes.push(format!(
            "Only the first {MAX_ELEMENTS} of {} elements are drawn. Fix: split the rest into another scene.",
            raw.len()
        ));
    }
    let mut out: Vec<Element> = vec![];
    let mut coloured = 0;
    for (i, v) in raw.iter().take(MAX_ELEMENTS).enumerate() {
        let o = match v {
            Value::Object(o) => o.clone(),
            Value::String(s) if !s.trim().is_empty() => {
                Map::from_iter([("text".to_string(), Value::from(s.trim()))])
            }
            _ => {
                notes.push(format!("Element {} isn't an object: left out.", i + 1));
                continue;
            }
        };
        let (what, kind) = match read_what(&o, tech, &mut notes) {
            Ok(w) => w,
            Err(e) => {
                notes.push(format!("Element {} left out: {e}", i + 1));
                continue;
            }
        };
        let size_v = o.get("size");
        let size = match &what {
            What::Text { .. } => text_size(size_v, 27.),
            What::Number { .. } => text_size(size_v, 60.),
            What::Icon(_) => box_size(size_v, 0.26),
            What::Shape { .. } => box_size(size_v, 0.26),
            What::Card { .. } | What::Bar { .. } | What::Link { .. } => box_size(size_v, 1.),
        };
        let what = match what {
            // Small words read better light; a headline is bold.
            What::Text { text, mono, .. } => What::Text {
                bold: flag(&o, "bold").unwrap_or(size >= 24.),
                text,
                mono,
            },
            w => w,
        };
        let cycle = [Tint::Accent, Tint::Accent2, Tint::Accent3];
        let default_tint = match &what {
            What::Text { .. } => Tint::Text,
            What::Number { .. } => Tint::Accent,
            What::Link { .. } => Tint::Muted,
            _ => {
                coloured += 1;
                cycle[(coloured - 1) % 3]
            }
        };
        let tint_ = match text_of(&o, &["color", "colour", "tint"]) {
            None => default_tint,
            Some(c) => tint(&c).unwrap_or_else(|| {
                notes.push(format!(
                    "Element {}: no colour \"{c}\". Fix: accent, accent2, accent3, text, muted or a hex like #ff8800.",
                    i + 1
                ));
                default_tint
            }),
        };
        let at = ["at", "position", "pos"]
            .iter()
            .find_map(|k| o.get(*k).and_then(point))
            .or_else(|| Some((share(num_of(&o, &["x"])?), share(num_of(&o, &["y"])?))));
        let start = num_of(&o, &["start", "delay", "time"]).map(|s| s.max(0.) as f32);
        let end = num_of(&o, &["end", "until"])
            .map(|e| e as f32)
            .or_else(|| {
                let d = num_of(&o, &["duration", "for"])? as f32;
                Some(start.unwrap_or(0.) + d)
            })
            .filter(|e| *e > 0.);
        let enter_ = match text_of(&o, &["in", "enter", "entrance", "animation", "animate"]) {
            None => Enter::Auto,
            Some(s) => enter(&s).unwrap_or_else(|| {
                notes.push(format!(
                    "Element {}: no entrance \"{s}\". Fix: one of {}.",
                    i + 1,
                    ENTRANCES.join(", ")
                ));
                Enter::Auto
            }),
        };
        let exit_ = text_of(&o, &["out", "exit"])
            .and_then(|s| exit(&s))
            .unwrap_or(Exit::Fade);
        let idle = match text_of(&o, &["loop", "idle"]).map(|s| norm(&s)).as_deref() {
            Some("float" | "bob" | "hover") => Idle::Float,
            Some("pulse" | "breathe" | "beat") => Idle::Pulse,
            Some("spin" | "rotate") => Idle::Spin,
            _ => Idle::Still,
        };
        let el = Element {
            id: text_of(&o, &["id"]).map(|s| norm(&s)),
            what,
            at,
            size,
            width: num_of(&o, &["width", "w"]).map(|w| share(w).clamp(0.1, 1.2)),
            tint: tint_,
            label: match kind {
                "text" | "card" | "line" => None,
                _ => text_of(&o, &["label", "caption"]),
            },
            start,
            end,
            enter: enter_,
            exit: exit_,
            moves: read_moves(&o),
            idle,
        };
        if let Some((x, y)) = el.at
            && (!(-0.05..=1.05).contains(&x) || !(-0.05..=1.05).contains(&y))
            && !el.moves.iter().any(|m| m.x.is_some() || m.y.is_some())
        {
            notes.push(format!(
                "Element {} sits off the stage (at {x:.2}, {y:.2}) and never moves in. Fix: an at inside 0–1, or a move that brings it in.",
                i + 1
            ));
        }
        out.push(el);
    }
    // Words given the same spot at the same time would sit on top of each
    // other (a small model writes "at": "center" on every line): they are
    // laid out instead. A word on a shape is meant, and left alone.
    let wordy = |e: &Element| {
        matches!(
            e.what,
            What::Text { .. } | What::Number { .. } | What::Card { .. } | What::Bar { .. }
        )
    };
    let together = |a: &Element, b: &Element| {
        let span = |e: &Element| (e.start.unwrap_or(0.), e.end.unwrap_or(f32::MAX));
        let ((s0, e0), (s1, e1)) = (span(a), span(b));
        s0 < e1 && s1 < e0
    };
    let stacked: Vec<usize> = (0..out.len())
        .filter(|&i| {
            let Some((x, y)) = out[i].at else {
                return false;
            };
            wordy(&out[i])
                && out[i].moves.is_empty()
                && out.iter().enumerate().any(|(k, o)| {
                    k != i
                        && wordy(o)
                        && together(&out[i], o)
                        && o.at
                            .is_some_and(|(ox, oy)| (ox - x).abs() < 0.04 && (oy - y).abs() < 0.04)
                })
        })
        .collect();
    if !stacked.is_empty() {
        notes.push(format!(
            "Elements {} were all given the same spot, so they were laid out one under another instead.",
            stacked
                .iter()
                .map(|i| (i + 1).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        for i in stacked {
            out[i].at = None;
        }
    }
    // A line to an element that isn't there can't be drawn.
    let ids: Vec<String> = out.iter().filter_map(|e| e.id.clone()).collect();
    out.retain(|e| match &e.what {
        What::Link { from, to, .. } => [from, to].iter().all(|end| match end {
            End::Id(id) if !ids.contains(id) => {
                notes.push(format!(
                    "A line to \"{id}\" was left out: no element has that id. Fix: give the element \"id\": \"{id}\"."
                ));
                false
            }
            _ => true,
        }),
        _ => true,
    });
    if out.iter().all(|e| matches!(e.what, What::Link { .. })) {
        return Err(format!(
            "No element could be read. {} Elements look like {EXAMPLE}",
            notes.join(" ")
        ));
    }
    Ok((out, notes))
}

/// The words a scene shows (its topic, for the look and the icons).
pub fn words(elements: &[Element]) -> String {
    elements
        .iter()
        .flat_map(|e| {
            let own = match &e.what {
                What::Text { text, .. } => Some(text.as_str()),
                What::Card { text, .. } => Some(text.as_str()),
                _ => None,
            };
            own.into_iter().chain(e.label.as_deref())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---- when ---------------------------------------------------------------------------------

/// Each element's (start, end) in a scene `len` seconds long: the times
/// given, else one after another across the scene's first two thirds (the
/// narration walks through them).
pub fn timing(elements: &[Element], len: f32) -> Vec<(f32, Option<f32>)> {
    let waiting = elements.iter().filter(|e| e.start.is_none()).count();
    let gap = ((len * 0.62 - 0.25) / waiting.max(1) as f32).clamp(0.16, 1.5);
    let mut k = 0;
    elements
        .iter()
        .map(|e| {
            let start = match e.start {
                Some(s) => s,
                None => {
                    k += 1;
                    0.25 + (k - 1) as f32 * gap
                }
            }
            .min((len - 0.4).max(0.));
            let end = e.end.map(|x| x.min(len)).filter(|x| *x > start + 0.2);
            (start, end)
        })
        .collect()
}

/// Where the scene's sound effects go: an element arriving.
pub fn beats(elements: &[Element], len: f32) -> Vec<(f32, &'static str, f32)> {
    let mut starts: Vec<f32> = timing(elements, len)
        .into_iter()
        .zip(elements)
        .filter(|(_, e)| !matches!(e.what, What::Link { .. }))
        .map(|((s, _), _)| s)
        .filter(|s| *s < len - 0.3)
        .collect();
    starts.sort_by(f32::total_cmp);
    let mut out: Vec<(f32, &'static str, f32)> = vec![];
    for s in starts {
        if out.last().is_none_or(|(t, _, _)| s - t >= 0.35) {
            out.push((s, "text", 0.));
        }
    }
    out.truncate(10);
    out
}

// ---- where --------------------------------------------------------------------------------

/// An element placed: its centre, its box at full size, the scale that
/// makes it fit, and its words broken into lines.
#[derive(Clone, Debug)]
struct Spot {
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    fit: f32,
    /// The label's room under the picture (px at full size).
    label_h: f32,
    lines: Vec<String>,
    start: f32,
    end: Option<f32>,
}

#[derive(Clone, Debug)]
pub(super) struct Laid {
    spots: Vec<Spot>,
}

/// An element's state at one time.
struct Pose {
    x: f32,
    y: f32,
    scale: f32,
    rot: f32,
    alpha: f32,
    /// How far a drawing-in (a line, typing, a bar filling) has got.
    progress: f32,
    /// Seconds since it started.
    lt: f32,
}

fn blend(a: Rgb, b: Rgb, k: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

fn ease_expo(v: f32) -> f32 {
    let v = clamp01(v);
    if v >= 1. {
        1.
    } else {
        1. - 2f32.powf(-10. * v)
    }
}

fn ease_in(v: f32) -> f32 {
    clamp01(v).powi(3)
}

fn apply(e: Ease, v: f32) -> f32 {
    let v = clamp01(v);
    match e {
        Ease::Out => ease_out(v),
        Ease::In => ease_in(v),
        Ease::InOut => {
            if v < 0.5 {
                4. * v * v * v
            } else {
                1. - (-2. * v + 2.).powi(3) / 2.
            }
        }
        Ease::Back => ease_back(v),
        Ease::Expo => ease_expo(v),
        Ease::Linear => v,
    }
}

fn lerp(a: f32, b: f32, k: f32) -> f32 {
    a + (b - a) * k
}

fn family(bold: bool, mono: bool) -> &'static str {
    match (mono, bold) {
        (true, _) => MONO,
        (false, true) => SANS_BOLD,
        (false, false) => SANS,
    }
}

impl Painter<'_> {
    fn tint(&self, t: Tint) -> Rgb {
        let p = self.scene.palette;
        match t {
            Tint::Accent => p.accent,
            Tint::Accent2 => p.accent2,
            Tint::Accent3 => p.accent3,
            Tint::Text => p.text,
            Tint::Muted => blend(p.text, p.bg[1], 0.38),
            Tint::Custom(c) => c,
        }
    }

    /// The size of words on one line (px).
    fn dims(&mut self, text: &str, size: f32, color: Rgb, family: &str) -> (f32, f32) {
        self.words_in(text, size, color, family)
            .map_or((0., 0.), |p| (p.width() as f32, p.height() as f32))
    }

    /// `text` broken into lines no wider than `room`.
    fn wrap(&mut self, text: &str, size: f32, family: &str, room: f32) -> Vec<String> {
        let white = (255, 255, 255);
        let mut lines = vec![];
        for para in text.split('\n') {
            let mut cur = String::new();
            for word in para.split_whitespace() {
                let cand = if cur.is_empty() {
                    word.to_string()
                } else {
                    format!("{cur} {word}")
                };
                if !cur.is_empty() && self.dims(&cand, size, white, family).0 > room {
                    lines.push(std::mem::replace(&mut cur, word.to_string()));
                } else {
                    cur = cand;
                }
            }
            if !cur.is_empty() {
                lines.push(cur);
            }
        }
        lines.truncate(5);
        lines
    }

    fn number_text(&self, el: &Element, v: f64) -> String {
        let What::Number {
            from,
            to,
            prefix,
            suffix,
        } = &el.what
        else {
            return String::new();
        };
        let d = match decimals_for(&[*from, *to]) {
            1 if ["$", "€", "£"].iter().any(|c| prefix.contains(c)) => 2,
            d => d,
        };
        format_value(v, d, prefix, suffix)
    }

    /// An element's own box (w, h) in px and its lines, given the widest
    /// it may be.
    fn measure(&mut self, el: &Element, room: f32, stage: Rect) -> (f32, f32, Vec<String>) {
        let u = self.u;
        let short = stage.width().min(stage.height());
        let label_h = if el.label.is_some() { 24. * u } else { 0. };
        match &el.what {
            What::Text { text, bold, mono } => {
                let fam = family(*bold, *mono);
                let lines = self.wrap(text, el.size, fam, room);
                let w = lines
                    .iter()
                    .map(|l| self.dims(l, el.size, (255, 255, 255), fam).0)
                    .fold(0f32, f32::max);
                let h = lines.len().max(1) as f32 * el.size * u * 1.22;
                (w, h, lines)
            }
            What::Number { to, .. } => {
                let s = self.number_text(el, *to);
                let w = self.dims(&s, el.size, (255, 255, 255), SANS_BOLD).0;
                (w, el.size * u * 1.12 + label_h, vec![])
            }
            What::Icon(_) => {
                let d = el.size * short;
                (d, d + label_h, vec![])
            }
            What::Shape { shape, ratio, .. } => {
                let w = el.size * short;
                let h = match shape {
                    Shape::Line => 4. * u,
                    _ => w / ratio.max(0.05),
                };
                (w, h + label_h, vec![])
            }
            What::Bar { .. } => {
                let w = (el.width.unwrap_or(0.82) * stage.width()).min(room);
                (w, 44. * u, vec![])
            }
            What::Card { detail, .. } => {
                let w = (el.width.unwrap_or(0.86) * stage.width()).min(room) * el.size.min(1.);
                let h = if detail.is_some() { 76. } else { 56. };
                (w, h * u, vec![])
            }
            What::Link { .. } => (0., 0., vec![]),
        }
    }

    fn lay_out(&mut self, elements: &[Element], flow: Flow) -> Laid {
        let stage = self.area();
        let len = self.scene.length as f32 / crate::model::SECOND as f32;
        let times = timing(elements, len);
        let (sw, sh) = (stage.width(), stage.height());
        let flowing: Vec<usize> = elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.at.is_none() && !matches!(e.what, What::Link { .. }))
            .map(|(i, _)| i)
            .collect();
        let n = flowing.len().max(1);
        let cols = match flow {
            Flow::Stack => 1,
            Flow::Row => n,
            Flow::Grid if n <= 4 || sh >= sw => 2,
            Flow::Grid => 3,
        };
        let rows = n.div_ceil(cols);
        let (cell_w, cell_h) = (sw / cols as f32, sh / rows as f32);
        let mut spots: Vec<Spot> = elements
            .iter()
            .zip(&times)
            .map(|(el, &(start, end))| {
                let room = match el.at {
                    Some(_) => el.width.unwrap_or(0.9) * sw,
                    None if flow == Flow::Stack => el.width.unwrap_or(0.96) * sw,
                    None => cell_w * 0.88,
                };
                let (w, h, lines) = self.measure(el, room, stage);
                let (x, y) = el.at.unwrap_or((0.5, 0.5));
                Spot {
                    cx: stage.left() + x * sw,
                    cy: stage.top() + y * sh,
                    w,
                    h,
                    fit: if w > room && w > 0. { room / w } else { 1. },
                    label_h: if el.label.is_some() { 24. * self.u } else { 0. },
                    lines,
                    start,
                    end,
                }
            })
            .collect();
        self.keep_apart(elements, &mut spots, stage);
        match flow {
            Flow::Stack => {
                let gap = (sh * 0.05).min(26. * self.u);
                let total: f32 = flowing
                    .iter()
                    .map(|&i| spots[i].h * spots[i].fit)
                    .sum::<f32>()
                    + gap * (n as f32 - 1.);
                // Too tall for the stage: everything a little smaller.
                let k = if total > sh {
                    (sh / total).max(0.4)
                } else {
                    1.
                };
                let mut y = stage.top() + (sh - total * k) / 2.;
                for &i in &flowing {
                    let s = &mut spots[i];
                    s.fit *= k;
                    s.cx = stage.left() + sw / 2.;
                    s.cy = y + s.h * s.fit / 2.;
                    y += s.h * s.fit + gap * k;
                }
            }
            Flow::Row | Flow::Grid => {
                for (k, &i) in flowing.iter().enumerate() {
                    let (col, row) = (k % cols, k / cols);
                    // A short last row sits in the middle.
                    let in_row = if row + 1 == rows {
                        n - row * cols
                    } else {
                        cols
                    };
                    let shift = (cols - in_row) as f32 * cell_w / 2.;
                    let s = &mut spots[i];
                    s.fit = s.fit.min(if s.h > 0. { cell_h * 0.86 / s.h } else { 1. });
                    s.cx = stage.left() + shift + cell_w * (col as f32 + 0.5);
                    s.cy = stage.top() + cell_h * (row as f32 + 0.5);
                }
            }
        }
        Laid { spots }
    }

    /// Elements placed by hand that would cover each other are moved apart,
    /// top to bottom in the order of their places, and the lot is brought
    /// back onto the stage. (A model sees no sizes: it puts a headline at
    /// 0.5 and its second line at 0.6, inside it.) Shapes are left where
    /// they are: a ring round a number is meant to overlap it.
    fn keep_apart(&self, elements: &[Element], spots: &mut [Spot], stage: Rect) {
        let solid = |e: &Element| {
            e.at.is_some()
                && e.moves.is_empty()
                && !matches!(e.what, What::Shape { .. } | What::Link { .. })
        };
        let mut order: Vec<usize> = (0..elements.len())
            .filter(|&i| solid(&elements[i]))
            .collect();
        if order.len() < 2 {
            return;
        }
        order.sort_by(|&a, &b| spots[a].cy.total_cmp(&spots[b].cy));
        let gap = 12. * self.u;
        let half = |s: &Spot| (s.w * s.fit / 2., s.h * s.fit / 2.);
        let together = |a: &Spot, b: &Spot| {
            a.start < b.end.unwrap_or(f32::MAX) && b.start < a.end.unwrap_or(f32::MAX)
        };
        let mut moved = false;
        for j in 1..order.len() {
            for i in 0..j {
                let (a, b) = (spots[order[i]].clone(), spots[order[j]].clone());
                let ((aw, ah), (bw, bh)) = (half(&a), half(&b));
                if !together(&a, &b) || (a.cx - b.cx).abs() >= aw + bw {
                    continue;
                }
                let need = ah + bh + gap;
                if b.cy - a.cy < need {
                    spots[order[j]].cy = a.cy + need;
                    moved = true;
                }
            }
        }
        if !moved {
            return;
        }
        // Pushed off the bottom: the whole group moves up, and when it is
        // taller than the stage it is drawn smaller about the middle.
        let top = order
            .iter()
            .map(|&i| spots[i].cy - half(&spots[i]).1)
            .fold(f32::MAX, f32::min);
        let bottom = order
            .iter()
            .map(|&i| spots[i].cy + half(&spots[i]).1)
            .fold(f32::MIN, f32::max);
        let tall = bottom - top;
        if tall > stage.height() {
            let k = (stage.height() / tall).max(0.4);
            let mid = (top + bottom) / 2.;
            let centre = stage.top() + stage.height() / 2.;
            for &i in &order {
                spots[i].cy = centre + (spots[i].cy - mid) * k;
                spots[i].fit *= k;
            }
        } else if bottom > stage.bottom() {
            let up = (bottom - stage.bottom()).min((top - stage.top()).max(0.));
            for &i in &order {
                spots[i].cy -= up;
            }
        }
    }

    /// The entrance a kind takes when none was asked for.
    fn own_entrance(el: &Element) -> Enter {
        match &el.what {
            What::Text { .. } if el.size >= 50. => Enter::Grow,
            What::Text { .. } => Enter::Rise,
            What::Number { .. } | What::Icon(_) => Enter::Pop,
            What::Shape {
                shape: Shape::Ring | Shape::Line,
                ..
            } => Enter::Draw,
            What::Shape { .. } => Enter::Pop,
            What::Bar { .. } | What::Link { .. } => Enter::Draw,
            What::Card { .. } => Enter::Rise,
        }
    }

    fn pose(&self, el: &Element, spot: &Spot, i: usize, t: f32, stage: Rect) -> Option<Pose> {
        let u = self.u;
        let lt = t - spot.start;
        if lt < 0. || spot.end.is_some_and(|e| t >= e) {
            return None;
        }
        let mut p = Pose {
            x: spot.cx,
            y: spot.cy,
            scale: 1.,
            rot: 0.,
            alpha: 1.,
            progress: 1.,
            lt,
        };
        // The moves, each from wherever the last one left it.
        for m in &el.moves {
            let k = apply(m.ease, (t - m.at) / m.len);
            if k <= 0. {
                continue;
            }
            if let Some(x) = m.x {
                p.x = lerp(p.x, stage.left() + x * stage.width(), k);
            }
            if let Some(y) = m.y {
                p.y = lerp(p.y, stage.top() + y * stage.height(), k);
            }
            if let Some(s) = m.scale {
                p.scale = lerp(p.scale, s, k);
            }
            if let Some(r) = m.rotate {
                p.rot = lerp(p.rot, r, k);
            }
            if let Some(o) = m.opacity {
                p.alpha = lerp(p.alpha, o, k);
            }
        }
        let enter = match el.enter {
            Enter::Auto => Self::own_entrance(el),
            e => e,
        };
        let words = match &el.what {
            What::Text { text, .. } => text.split_whitespace().count().max(1) as f32,
            _ => 1.,
        };
        let chars = match &el.what {
            What::Text { text, .. } => text.chars().count().max(1) as f32,
            _ => 1.,
        };
        // Arrivals land on a long tail; a fast one reads as a glitch.
        let len_in = match enter {
            Enter::None => 0.,
            Enter::Fade => 0.4,
            Enter::Rise | Enter::Drop | Enter::Grow | Enter::Spin => 0.6,
            Enter::SlideLeft | Enter::SlideRight => 0.55,
            Enter::Pop => 0.45,
            Enter::Draw => {
                if matches!(el.what, What::Link { .. }) {
                    0.55
                } else {
                    0.9
                }
            }
            Enter::Type => (chars / 26.).clamp(0.4, 2.4),
            Enter::Words => 0.09 * (words - 1.) + 0.3,
            Enter::Auto => 0.5,
        };
        let e = if len_in > 0. {
            clamp01(lt / len_in)
        } else {
            1.
        };
        match enter {
            Enter::None | Enter::Auto => {}
            Enter::Fade => p.alpha *= ease_out(e),
            Enter::Rise => {
                p.y += (1. - ease_expo(e)) * 30. * u;
                p.alpha *= clamp01(e * 2.4);
            }
            Enter::Drop => {
                p.y -= (1. - ease_expo(e)) * 30. * u;
                p.alpha *= clamp01(e * 2.4);
            }
            Enter::SlideLeft => {
                p.x -= (1. - ease_expo(e)) * stage.width() * 0.6;
                p.alpha *= clamp01(e * 3.);
            }
            Enter::SlideRight => {
                p.x += (1. - ease_expo(e)) * stage.width() * 0.6;
                p.alpha *= clamp01(e * 3.);
            }
            Enter::Pop => {
                p.scale *= ease_back(e).max(0.);
                p.alpha *= clamp01(e * 4.);
            }
            Enter::Grow => {
                p.scale *= 0.72 + 0.28 * ease_expo(e);
                p.alpha *= clamp01(e * 2.2);
            }
            Enter::Spin => {
                p.rot -= (1. - ease_expo(e)) * 140.;
                p.scale *= ease_out(e);
                p.alpha *= clamp01(e * 3.);
            }
            Enter::Draw => {
                p.progress = ease_out(e);
                // What can't be drawn in a stroke fades in instead.
                if !matches!(
                    el.what,
                    What::Link { .. }
                        | What::Bar { .. }
                        | What::Shape {
                            shape: Shape::Ring | Shape::Line,
                            ..
                        }
                ) {
                    p.alpha *= ease_out(e);
                }
            }
            Enter::Type | Enter::Words => {
                p.progress = e;
                if !matches!(el.what, What::Text { .. }) {
                    p.alpha *= ease_out(e);
                }
            }
        }
        // Leaving takes about half as long as arriving.
        if let Some(end) = spot.end {
            let len_out = (if el.exit == Exit::Cut { 0. } else { 0.3f32 }).min(end - spot.start);
            let x = if len_out > 0. {
                clamp01((t - (end - len_out)) / len_out)
            } else {
                0.
            };
            match el.exit {
                Exit::Cut => {}
                Exit::Fade => p.alpha *= 1. - x,
                Exit::Fall => {
                    p.y += ease_in(x) * 40. * u;
                    p.alpha *= 1. - x;
                }
                Exit::Shrink => {
                    p.scale *= 1. - 0.45 * ease_in(x);
                    p.alpha *= 1. - x;
                }
                Exit::SlideLeft => {
                    p.x -= ease_in(x) * stage.width() * 0.7;
                    p.alpha *= 1. - x * x;
                }
                Exit::SlideRight => {
                    p.x += ease_in(x) * stage.width() * 0.7;
                    p.alpha *= 1. - x * x;
                }
            }
        }
        // A loop eases in once the element has landed.
        let amp = clamp01((lt - len_in) / 0.5);
        let phase = i as f32 * 1.3;
        match el.idle {
            Idle::Still => {}
            Idle::Float => p.y += (t * 1.7 + phase).sin() * 5. * u * amp,
            Idle::Pulse => p.scale *= 1. + 0.045 * (t * 3.2 + phase).sin() * amp,
            Idle::Spin => p.rot += (lt - len_in).max(0.) * 45.,
        }
        p.alpha = clamp01(p.alpha);
        (p.alpha > 0.004 && p.scale > 0.001).then_some(p)
    }

    /// One line of words drawn with its top-left at (`lx`, `ly`) from the
    /// element's centre, under the element's pose.
    #[allow(clippy::too_many_arguments)]
    fn sprite(
        &mut self,
        pix: &mut Pixmap,
        text: &str,
        size: f32,
        color: Rgb,
        family: &str,
        (lx, ly): (f32, f32),
        place: Transform,
        alpha: f32,
    ) {
        if alpha <= 0.01 || text.is_empty() {
            return;
        }
        let Some(w) = self.words_in(text, size, color, family) else {
            return;
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
            Transform::from_translate(lx, ly).post_concat(place),
            None,
        );
    }

    /// Words centred on (`cx`, `cy`) from the element's centre, shrunk to
    /// `max_w`.
    #[allow(clippy::too_many_arguments)]
    fn label_at(
        &mut self,
        pix: &mut Pixmap,
        text: &str,
        size: f32,
        color: Rgb,
        family: &str,
        (cx, cy): (f32, f32),
        max_w: f32,
        place: Transform,
        alpha: f32,
    ) {
        let (w, h) = self.dims(text, size, color, family);
        if w <= 0. {
            return;
        }
        let k = (max_w / w).min(1.);
        let tf = Transform::from_scale(k, k)
            .post_translate(cx, cy)
            .post_concat(place);
        self.sprite(
            pix,
            text,
            size,
            color,
            family,
            (-w / 2., -h / 2.),
            tf,
            alpha,
        );
    }

    fn element(&mut self, pix: &mut Pixmap, el: &Element, spot: &Spot, p: &Pose) {
        let u = self.u;
        let color = self.tint(el.tint);
        let muted = self.tint(Tint::Muted);
        let k = p.scale * spot.fit;
        let label_h = if el.label.is_some() { 24. * u } else { 0. };
        // Local space: the picture's centre at the origin, full size. A
        // label under it stays level while the picture turns (`flat`: the
        // whole box's centre at the origin).
        let place = Transform::from_scale(k, k)
            .post_rotate(p.rot)
            .post_translate(p.x, p.y - label_h / 2. * k);
        let flat = Transform::from_scale(k, k).post_translate(p.x, p.y);
        let label_y = spot.h / 2. - 12. * u;
        let enter = match el.enter {
            Enter::Auto => Self::own_entrance(el),
            e => e,
        };
        match &el.what {
            What::Text { bold, mono, .. } => {
                let fam = family(*bold, *mono);
                let line_h = el.size * u * 1.22;
                let n = spot.lines.len() as f32;
                let total: usize = spot.lines.iter().map(|l| l.chars().count()).sum();
                let mut typed = (p.progress * total as f32).round() as usize;
                let mut word_no = 0;
                for (j, line) in spot.lines.clone().iter().enumerate() {
                    let (w, h) = self.dims(line, el.size, color, fam);
                    let top = (j as f32 - (n - 1.) / 2.) * line_h - h / 2.;
                    match enter {
                        Enter::Type if p.progress < 1. => {
                            let take = typed.min(line.chars().count());
                            typed -= take;
                            let part: String = line.chars().take(take).collect();
                            self.sprite(
                                pix,
                                part.trim_end(),
                                el.size,
                                color,
                                fam,
                                (-w / 2., top),
                                place,
                                p.alpha,
                            );
                        }
                        Enter::Words if p.progress < 1. => {
                            let words: Vec<&str> = line.split(' ').collect();
                            for (g, word) in words.iter().enumerate() {
                                let a = ease_out((p.lt - word_no as f32 * 0.09) / 0.3);
                                word_no += 1;
                                if a <= 0. {
                                    continue;
                                }
                                // Where the word starts on its line.
                                let upto = words[..=g].join(" ");
                                let left = self.dims(&upto, el.size, color, fam).0
                                    - self.dims(word, el.size, color, fam).0;
                                self.sprite(
                                    pix,
                                    word,
                                    el.size,
                                    color,
                                    fam,
                                    (-w / 2. + left, top + (1. - a) * 9. * u),
                                    place,
                                    p.alpha * a,
                                );
                            }
                        }
                        _ => self.sprite(
                            pix,
                            line,
                            el.size,
                            color,
                            fam,
                            (-w / 2., top),
                            place,
                            p.alpha,
                        ),
                    }
                }
            }
            What::Number { from, to, .. } => {
                let run = if enter == Enter::None {
                    1.
                } else {
                    ease_out(p.lt / 1.5)
                };
                let s = self.number_text(el, from + (to - from) * run as f64);
                let (w, h) = self.dims(&s, el.size, color, SANS_BOLD);
                self.sprite(
                    pix,
                    &s,
                    el.size,
                    color,
                    SANS_BOLD,
                    (-w / 2., -h / 2.),
                    place,
                    p.alpha,
                );
                if let Some(label) = el.label.clone() {
                    let room = self.area().width() * 0.9 / spot.fit.max(0.01);
                    self.label_at(
                        pix,
                        &label,
                        (el.size * 0.3).clamp(12., 22.),
                        muted,
                        SANS,
                        (0., label_y),
                        room,
                        flat,
                        p.alpha * clamp01(p.lt / 0.5 - 0.6),
                    );
                }
            }
            What::Icon(icon) => {
                let d = spot.w;
                let top = -d / 2.;
                if let Some(disc) = PathBuilder::from_circle(0., top + d / 2., d / 2.)
                    && let Some(disc) = disc.transform(place)
                {
                    pix.fill_path(
                        &disc,
                        &paint(rgba(color, 0.16 * p.alpha)),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                    let stroke = Stroke {
                        width: 1.2 * u,
                        ..Stroke::default()
                    };
                    pix.stroke_path(
                        &disc,
                        &paint(rgba(color, 0.5 * p.alpha)),
                        &stroke,
                        Transform::identity(),
                        None,
                    );
                }
                let inset = d * 0.25;
                if let Some(r) =
                    Rect::from_xywh(-d / 2. + inset, top + inset, d - inset * 2., d - inset * 2.)
                {
                    icon.draw_placed(pix, r, color, p.alpha, (d * 0.05).max(1.5 * u), place);
                }
                if let Some(label) = el.label.clone() {
                    self.label_at(
                        pix,
                        &label,
                        14.,
                        muted,
                        SANS,
                        (0., label_y),
                        d * 2.4,
                        flat,
                        p.alpha,
                    );
                }
            }
            What::Shape {
                shape,
                ratio,
                filled,
            } => {
                let w = spot.w;
                let h = w / ratio.max(0.05);
                let cy = 0.;
                let stroke_w = (w * 0.07).clamp(2. * u, 9. * u);
                let mut stroked = !*filled;
                let path = match shape {
                    Shape::Circle => PathBuilder::from_circle(0., cy, w / 2.),
                    Shape::Ring => {
                        stroked = true;
                        let steps = (72. * p.progress).ceil().max(2.) as usize;
                        let r = w / 2. - stroke_w / 2.;
                        let mut pb = PathBuilder::new();
                        for s in 0..=steps {
                            let a = -std::f32::consts::FRAC_PI_2
                                + std::f32::consts::TAU * p.progress * s as f32 / steps as f32;
                            let (x, y) = (r * a.cos(), cy + r * a.sin());
                            if s == 0 {
                                pb.move_to(x, y);
                            } else {
                                pb.line_to(x, y);
                            }
                        }
                        pb.finish()
                    }
                    Shape::Rect => Rect::from_xywh(-w / 2., cy - h / 2., w, h)
                        .and_then(|r| text_render::rounded(r, 0.28)),
                    Shape::Pill => Rect::from_xywh(-w / 2., cy - h / 2., w, h)
                        .and_then(|r| text_render::rounded(r, 1.)),
                    Shape::Triangle => {
                        let mut pb = PathBuilder::new();
                        pb.move_to(0., cy - h / 2.);
                        pb.line_to(w / 2., cy + h / 2.);
                        pb.line_to(-w / 2., cy + h / 2.);
                        pb.close();
                        pb.finish()
                    }
                    Shape::Star => {
                        let mut pb = PathBuilder::new();
                        for s in 0..10 {
                            let r = if s % 2 == 0 { w / 2. } else { w * 0.2 };
                            let a =
                                -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * s as f32 / 5.;
                            let (x, y) = (r * a.cos(), cy + r * a.sin());
                            if s == 0 {
                                pb.move_to(x, y);
                            } else {
                                pb.line_to(x, y);
                            }
                        }
                        pb.close();
                        pb.finish()
                    }
                    Shape::Line => {
                        stroked = true;
                        let mut pb = PathBuilder::new();
                        pb.move_to(-w / 2., cy);
                        pb.line_to(-w / 2. + w * p.progress.max(0.002), cy);
                        pb.finish()
                    }
                };
                if let Some(path) = path.and_then(|path| path.transform(place)) {
                    if !self.scene.palette.light {
                        self.glow_path(pix, &path, color, 0.5 * p.alpha);
                    }
                    if stroked {
                        let stroke = Stroke {
                            width: if *shape == Shape::Line {
                                3.2 * u
                            } else {
                                stroke_w * k.min(1.)
                            },
                            line_cap: LineCap::Round,
                            line_join: tiny_skia::LineJoin::Round,
                            ..Stroke::default()
                        };
                        pix.stroke_path(
                            &path,
                            &paint(rgba(color, p.alpha)),
                            &stroke,
                            Transform::identity(),
                            None,
                        );
                    } else {
                        pix.fill_path(
                            &path,
                            &paint(rgba(color, p.alpha)),
                            FillRule::Winding,
                            Transform::identity(),
                            None,
                        );
                    }
                }
                if let Some(label) = el.label.clone() {
                    self.label_at(
                        pix,
                        &label,
                        14.,
                        muted,
                        SANS,
                        (0., label_y),
                        w.max(120. * u) * 1.6,
                        flat,
                        p.alpha,
                    );
                }
            }
            What::Bar { value, shown } => {
                let (w, h) = (spot.w * k, 12. * u * k.min(1.));
                let (left, top) = (p.x - w / 2., p.y + 6. * u * k);
                let text = self.scene.palette.text;
                if let Some(track) =
                    Rect::from_xywh(left, top, w, h).and_then(|r| text_render::rounded(r, 1.))
                {
                    pix.fill_path(
                        &track,
                        &paint(rgba(text, 0.14 * p.alpha)),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
                let fill = (w * value * p.progress).max(h);
                if let Some(bar) =
                    Rect::from_xywh(left, top, fill, h).and_then(|r| text_render::rounded(r, 1.))
                {
                    pix.fill_path(
                        &bar,
                        &paint(rgba(color, p.alpha)),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
                if let Some(label) = el.label.clone() {
                    let (lw, lh) = self.dims(&label, 15., text, SANS);
                    let fitk = (spot.w * 0.72 / lw.max(1.)).min(1.);
                    let tf = Transform::from_scale(fitk, fitk).post_concat(flat);
                    self.sprite(
                        pix,
                        &label,
                        15.,
                        text,
                        SANS,
                        (-spot.w / 2. / fitk, -10. * u / fitk - lh / 2.),
                        tf,
                        p.alpha,
                    );
                }
                let (vw, vh) = self.dims(shown, 15., color, SANS_BOLD);
                self.sprite(
                    pix,
                    shown,
                    15.,
                    color,
                    SANS_BOLD,
                    (spot.w / 2. - vw, -10. * u - vh / 2.),
                    flat,
                    p.alpha * clamp01(p.progress * 2.),
                );
            }
            What::Card { text, detail, icon } => {
                let (w, h) = (spot.w * k, spot.h * k);
                let Some(r) = Rect::from_xywh(p.x - w / 2., p.y - h / 2., w, h) else {
                    return;
                };
                self.glass(pix, r, 16. * u * k, color, 0.55, 0.22, p.alpha);
                let pad = 16. * u;
                let mut left = -spot.w / 2. + pad;
                if let Some(icon) = icon {
                    let d = spot.h - pad * 1.5;
                    if let Some(ir) = Rect::from_xywh(left, -d / 2., d, d) {
                        icon.draw_placed(pix, ir, color, p.alpha, (d * 0.06).max(1.5 * u), flat);
                    }
                    left += d + pad * 0.8;
                }
                let room = spot.w / 2. - pad - left;
                let ink = self.scene.palette.text;
                let title_y = if detail.is_some() { -11. * u } else { 0. };
                let (tw, th) = self.dims(text, 18., ink, SANS_BOLD);
                let tk = (room / tw.max(1.)).min(1.);
                // With an icon the words start after it; alone they're centred.
                let tx = if icon.is_some() { left } else { -tw * tk / 2. };
                self.sprite(
                    pix,
                    text,
                    18.,
                    ink,
                    SANS_BOLD,
                    (0., 0.),
                    Transform::from_scale(tk, tk)
                        .post_translate(tx, title_y - th * tk / 2.)
                        .post_concat(flat),
                    p.alpha,
                );
                if let Some(detail) = detail {
                    let (dw, dh) = self.dims(detail, 13., muted, SANS);
                    let dk = (room / dw.max(1.)).min(1.);
                    let dx = if icon.is_some() { left } else { -dw * dk / 2. };
                    self.sprite(
                        pix,
                        detail,
                        13.,
                        muted,
                        SANS,
                        (0., 0.),
                        Transform::from_scale(dk, dk)
                            .post_translate(dx, 13. * u - dh * dk / 2.)
                            .post_concat(flat),
                        p.alpha * 0.92,
                    );
                }
            }
            What::Link { .. } => {}
        }
    }

    /// Where a line to or from an element stops: at the edge of its box.
    fn link_end(&self, end: &End, toward: Point, elements: &[Element], laid: &Laid) -> Point {
        let stage = self.area();
        match end {
            End::At(x, y) => Point::from_xy(
                stage.left() + x * stage.width(),
                stage.top() + y * stage.height(),
            ),
            End::Id(id) => {
                let Some(i) = elements.iter().position(|e| e.id.as_deref() == Some(id)) else {
                    return toward;
                };
                let s = &laid.spots[i];
                // The picture, not the label under it.
                let c = Point::from_xy(s.cx, s.cy - s.label_h * s.fit / 2.);
                let (dx, dy) = (toward.x - c.x, toward.y - c.y);
                let pad = 10. * self.u;
                let (hw, hh) = (s.w * s.fit / 2. + pad, (s.h - s.label_h) * s.fit / 2. + pad);
                // How far along the way the box's edge is.
                let k = (hw / dx.abs().max(1e-3))
                    .min(hh / dy.abs().max(1e-3))
                    .min(0.5);
                Point::from_xy(c.x + dx * k, c.y + dy * k)
            }
        }
    }

    fn centre_of(&self, end: &End, elements: &[Element], laid: &Laid) -> Point {
        let stage = self.area();
        match end {
            End::At(x, y) => Point::from_xy(
                stage.left() + x * stage.width(),
                stage.top() + y * stage.height(),
            ),
            End::Id(id) => elements
                .iter()
                .position(|e| e.id.as_deref() == Some(id))
                .map_or(Point::from_xy(stage.left(), stage.top()), |i| {
                    let s = &laid.spots[i];
                    Point::from_xy(s.cx, s.cy - s.label_h * s.fit / 2.)
                }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn link(
        &mut self,
        pix: &mut Pixmap,
        el: &Element,
        p: &Pose,
        (from, to, arrow): (&End, &End, bool),
        elements: &[Element],
        laid: &Laid,
    ) {
        let u = self.u;
        let color = self.tint(el.tint);
        let (c0, c1) = (
            self.centre_of(from, elements, laid),
            self.centre_of(to, elements, laid),
        );
        let (a, b) = (
            self.link_end(from, c1, elements, laid),
            self.link_end(to, c0, elements, laid),
        );
        let tip = Point::from_xy(lerp(a.x, b.x, p.progress), lerp(a.y, b.y, p.progress));
        let mut pb = PathBuilder::new();
        pb.move_to(a.x, a.y);
        pb.line_to(tip.x, tip.y);
        let stroke = Stroke {
            width: 2.4 * u,
            line_cap: LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Stroke::default()
        };
        if let Some(path) = pb.finish() {
            pix.stroke_path(
                &path,
                &paint(rgba(color, 0.9 * p.alpha)),
                &stroke,
                Transform::identity(),
                None,
            );
        }
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len = (dx * dx + dy * dy).sqrt();
        if arrow && len > 1. && p.progress > 0.2 {
            let (nx, ny) = (dx / len, dy / len);
            let head = 9. * u;
            let mut pb = PathBuilder::new();
            for side in [-1f32, 1.] {
                pb.move_to(tip.x, tip.y);
                pb.line_to(
                    tip.x - nx * head - ny * head * 0.6 * side,
                    tip.y - ny * head + nx * head * 0.6 * side,
                );
            }
            if let Some(path) = pb.finish() {
                pix.stroke_path(
                    &path,
                    &paint(rgba(color, 0.9 * p.alpha)),
                    &stroke,
                    Transform::identity(),
                    None,
                );
            }
        }
    }

    pub(super) fn canvas(&mut self, pix: &mut Pixmap, t: f32, elements: &[Element], flow: Flow) {
        let laid = match self.canvas.take() {
            Some(l) => l,
            None => self.lay_out(elements, flow),
        };
        let stage = self.area();
        // Lines first: they run under what they join.
        for pass in [true, false] {
            for (i, (el, spot)) in elements.iter().zip(&laid.spots).enumerate() {
                let is_link = matches!(el.what, What::Link { .. });
                if is_link != pass {
                    continue;
                }
                let Some(pose) = self.pose(el, spot, i, t, stage) else {
                    continue;
                };
                match &el.what {
                    What::Link { from, to, arrow } => {
                        self.link(pix, el, &pose, (from, to, *arrow), elements, &laid)
                    }
                    _ => self.element(pix, el, spot, &pose),
                }
            }
        }
        self.canvas = Some(laid);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Backdrop, PALETTES, Scene, Visual, save_still, still};
    use super::*;
    use crate::model::SECOND;
    use serde_json::json;

    fn elements(v: Value) -> Vec<Element> {
        read(v.as_array().unwrap(), false).unwrap().0
    }

    fn scene(v: Value, flow: Flow, secs: i64) -> Scene {
        Scene {
            visual: Visual::Canvas {
                elements: elements(v),
                flow,
            },
            palette: &PALETTES[0],
            backdrop: Backdrop::Glow,
            seed: 3,
            prefix: String::new(),
            suffix: String::new(),
            length: secs * SECOND,
            width: 540,
            height: 960,
            fps: 30,
        }
    }

    #[test]
    fn reads_elements_by_the_key_they_carry() {
        let els = elements(json!([
            "Just words",
            { "text": "Big", "size": "hero", "color": "accent2", "in": "words" },
            { "number": 87, "suffix": "%", "label": "finish" },
            { "icon": "rocket", "id": "r", "at": "top_right", "loop": "float" },
            { "shape": "ring", "size": 40, "color": "#f80" },
            { "bar": 72, "label": "Battery" },
            { "card": "Step 1", "detail": "Plan it", "icon": "bulb", "id": "s1" },
            { "line": ["r", "s1"] },
        ]));
        assert_eq!(els.len(), 8);
        assert!(
            matches!(&els[0].what, What::Text { text, bold: true, .. } if text == "Just words")
        );
        assert_eq!(els[1].size, 60.);
        assert_eq!(els[1].tint, Tint::Accent2);
        assert_eq!(els[1].enter, Enter::Words);
        assert!(
            matches!(&els[2].what, What::Number { to, suffix, .. } if *to == 87. && suffix == "%")
        );
        assert_eq!(els[2].label.as_deref(), Some("finish"));
        assert_eq!(els[3].at, Some((0.8, 0.14)));
        assert_eq!(els[3].idle, Idle::Float);
        assert!(matches!(
            els[4].what,
            What::Shape {
                shape: Shape::Ring,
                filled: false,
                ..
            }
        ));
        assert_eq!(els[4].size, 0.4);
        assert_eq!(els[4].tint, Tint::Custom((255, 136, 0)));
        assert!(
            matches!(&els[5].what, What::Bar { value, shown } if (*value - 0.72).abs() < 1e-6 && shown == "72%")
        );
        assert!(matches!(&els[6].what, What::Card { icon: Some(_), .. }));
        assert!(matches!(&els[7].what, What::Link { arrow: true, .. }));
    }

    #[test]
    fn says_what_it_left_out_and_how_to_fix_it() {
        let raw = json!([
            { "text": "Hello", "in": "wobble", "color": "sparkly" },
            { "line": ["a", "nowhere"] },
            { "icon": "rocket", "at": [1.6, 0.5] },
            { "shape": "blob" },
        ]);
        let (els, notes) = read(raw.as_array().unwrap(), false).unwrap();
        assert_eq!(els.len(), 2);
        let all = notes.join("\n");
        assert!(all.contains("no entrance \"wobble\""), "{all}");
        assert!(all.contains("no colour \"sparkly\""), "{all}");
        assert!(all.contains("no element has that id"), "{all}");
        assert!(all.contains("off the stage"), "{all}");
        assert!(all.contains("no shape \"blob\""), "{all}");
        assert!(notes.iter().filter(|n| n.contains("Fix:")).count() >= 4);
        // An icon that isn't there is drawn as the nearest: said, not a redo.
        let (_, notes) = read(&[json!({ "icon": "bee" })], false).unwrap();
        assert_eq!(
            notes,
            ["There is no icon \"bee\": the leaf icon is drawn instead."]
        );
        assert!(read(&[], false).is_err());
        assert!(read(&[json!(3)], false).is_err());
    }

    #[test]
    fn words_on_one_spot_are_laid_out() {
        let raw = json!([
            { "text": "Track spending", "at": "center" },
            { "text": "Set a goal", "at": "center" },
            { "shape": "ring", "at": "center" },
            { "text": "First", "at": "top", "end": 2 },
            { "text": "Then", "at": "top", "start": 2 },
        ]);
        let (els, notes) = read(raw.as_array().unwrap(), false).unwrap();
        assert_eq!((els[0].at, els[1].at), (None, None));
        // A shape behind words, and words that take turns, stay put.
        assert!(els[2].at.is_some() && els[3].at.is_some() && els[4].at.is_some());
        assert!(
            notes[0].starts_with("Elements 1, 2 were all given the same spot"),
            "{notes:?}"
        );
    }

    #[test]
    fn elements_come_in_one_after_another() {
        let els = elements(json!([
            { "text": "One" }, { "text": "Two" }, { "text": "Three", "start": 5, "end": 7 },
            { "text": "Four" },
        ]));
        let t = timing(&els, 8.);
        assert_eq!(t[0].0, 0.25);
        assert!(t[1].0 > t[0].0 && t[3].0 > t[1].0);
        assert_eq!(t[2], (5., Some(7.)));
        // Never after the scene is over.
        assert!(timing(&els, 2.).iter().all(|(s, _)| *s <= 1.6));
        assert_eq!(beats(&els, 8.).len(), 4);
    }

    #[test]
    fn a_canvas_draws_moves_and_ends_where_it_says() {
        let s = scene(
            json!([
                { "text": "Compound interest", "size": "large", "in": "words" },
                { "number": 1825, "prefix": "$", "label": "after one year" },
                { "icon": "coin", "at": [0.2, 0.85], "moves": [{ "at": 1.5, "x": 0.8, "duration": 1 }] },
                { "shape": "ring", "at": [0.5, 0.5], "size": 0.9, "start": 0.2, "end": 1.2 },
            ]),
            Flow::Stack,
            4,
        );
        let (a, b, c) = (still(&s, 0.1), still(&s, 1.), still(&s, 3.5));
        assert!(c.pixels().iter().all(|p| p.alpha() == 255));
        assert_ne!(a.data(), b.data());
        assert_ne!(b.data(), c.data());
        // The same time is the same frame, whatever was drawn before.
        assert_eq!(still(&s, 3.5).data(), c.data());
        // The coin has crossed: the right of its row changed, the ring is gone.
        let Visual::Canvas { elements, .. } = &s.visual else {
            unreachable!()
        };
        let mut painter = super::super::Painter::new(&s);
        let laid = painter.lay_out(elements, Flow::Stack);
        let stage = painter.area();
        let coin = painter
            .pose(&elements[2], &laid.spots[2], 2, 3.5, stage)
            .unwrap();
        assert!((coin.x - (stage.left() + 0.8 * stage.width())).abs() < 1.);
        assert!(
            painter
                .pose(&elements[3], &laid.spots[3], 3, 1.3, stage)
                .is_none()
        );
        assert!(
            painter
                .pose(&elements[3], &laid.spots[3], 3, 0.1, stage)
                .is_none()
        );
    }

    #[test]
    fn hand_placed_elements_are_kept_apart() {
        // What a small model writes: a headline in the middle, its second
        // line and an icon a tenth away, inside it.
        let s = scene(
            json!([
                { "text": "1 in 5", "size": "hero", "at": "center" },
                { "text": "of our food", "size": "large", "at": "0.5,0.6" },
                { "icon": "bee", "label": "Bees", "at": "0.5,0.3" },
                { "shape": "ring", "at": "center", "size": "hero" },
                { "text": "Aside", "size": "small", "at": "0.1,0.5" },
            ]),
            Flow::Stack,
            6,
        );
        let Visual::Canvas { elements, .. } = &s.visual else {
            unreachable!()
        };
        assert!(matches!(elements[2].what, What::Icon(Icon::Leaf)));
        let mut painter = super::super::Painter::new(&s);
        let laid = painter.lay_out(elements, Flow::Stack);
        let stage = painter.area();
        let edge = |i: usize| {
            let sp = &laid.spots[i];
            (sp.cy - sp.h * sp.fit / 2., sp.cy + sp.h * sp.fit / 2.)
        };
        // The icon, then the headline, then its second line: none inside
        // another, all on the stage.
        assert!(
            edge(2).1 <= edge(0).0 && edge(0).1 <= edge(1).0,
            "{:?}",
            laid.spots
        );
        assert!(edge(2).0 >= stage.top() - 1. && edge(1).1 <= stage.bottom() + 1.);
        // The ring stays round the headline's place; the aside, beside it.
        assert_eq!(laid.spots[3].cy, stage.top() + stage.height() / 2.);
        assert_eq!(laid.spots[4].cy, stage.top() + stage.height() / 2.);
    }

    #[test]
    fn a_stack_too_tall_is_shrunk_to_the_stage() {
        let many: Vec<Value> = (0..9)
            .map(|i| json!({ "text": format!("Line number {i}"), "size": "large" }))
            .collect();
        let s = scene(Value::Array(many), Flow::Stack, 5);
        let Visual::Canvas { elements, .. } = &s.visual else {
            unreachable!()
        };
        let mut painter = super::super::Painter::new(&s);
        let laid = painter.lay_out(elements, Flow::Stack);
        let stage = painter.area();
        for spot in &laid.spots {
            assert!(spot.fit < 1.);
            assert!(spot.cy - spot.h * spot.fit / 2. >= stage.top() - 1.);
            assert!(spot.cy + spot.h * spot.fit / 2. <= stage.bottom() + 1.);
        }
    }

    /// Frames of a few canvases for a look by eye.
    #[test]
    #[ignore]
    fn save_canvas_looks() {
        let dir = std::env::var("REEL_LOOKS_DIR").unwrap_or_else(|_| "/tmp".into());
        let looks: Vec<(&str, Value, Flow, usize)> = vec![
            (
                "stat",
                json!([
                    { "text": "Finish rate", "size": "small", "color": "muted" },
                    { "number": 87, "suffix": "%", "label": "of learners finish the course" },
                    { "bar": 87, "label": "This course" },
                    { "bar": 31, "label": "Average course", "color": "muted" },
                    { "text": "Small steps beat big plans", "in": "words" },
                ]),
                Flow::Stack,
                0,
            ),
            (
                "row",
                json!([
                    { "icon": "bulb", "label": "Idea", "id": "a" },
                    { "icon": "gear", "label": "Build", "id": "b", "loop": "spin" },
                    { "icon": "rocket", "label": "Launch", "id": "c" },
                    { "line": ["a", "b"] }, { "line": ["b", "c"] },
                    { "text": "From idea to launch in a week", "at": [0.5, 0.86], "size": "medium" },
                ]),
                Flow::Row,
                3,
            ),
            (
                "grid",
                json!([
                    { "card": "Sleep", "detail": "7–9 hours", "icon": "moon" },
                    { "card": "Water", "detail": "2 litres", "icon": "drop" },
                    { "card": "Move", "detail": "30 minutes", "icon": "heart" },
                    { "card": "Sun", "detail": "10 minutes", "icon": "sun" },
                ]),
                Flow::Grid,
                4,
            ),
            (
                "free",
                json!([
                    { "shape": "ring", "at": "center", "size": 0.8, "color": "accent2" },
                    { "text": "3×", "size": "hero", "at": [0.5, 0.45], "color": "accent" },
                    { "text": "faster than last year", "at": [0.5, 0.62], "size": "small" },
                    { "shape": "star", "at": [0.15, 0.15], "size": 0.12, "color": "accent3", "loop": "spin" },
                    { "shape": "circle", "at": [0.88, 0.85], "size": 0.1, "loop": "float" },
                    { "shape": "line", "at": [0.5, 0.72], "size": 0.5, "color": "accent" },
                ]),
                Flow::Stack,
                8,
            ),
        ];
        for (name, v, flow, palette) in looks {
            let mut s = scene(v, flow, 6);
            (s.width, s.height) = (1080, 1920);
            s.palette = &PALETTES[palette];
            for t in [0.9, 4.5] {
                let path = std::path::Path::new(&dir).join(format!("canvas-{name}-{t}.png"));
                save_still(&s, t, &path).unwrap();
            }
            (s.width, s.height) = (1920, 1080);
            let path = std::path::Path::new(&dir).join(format!("canvas-{name}-wide.png"));
            save_still(&s, 4.5, &path).unwrap();
        }
    }
}
