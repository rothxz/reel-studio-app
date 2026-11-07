//! Text clips as pictures: the clip's words laid out (cosmic-text: shaping,
//! fallback fonts, wrapping) and drawn with its look (tiny-skia): fill,
//! stroke, glow, drop shadow, a background box or one per line,
//! underline, letter / line spacing and the arc bend. The result is a
//! premultiplied BGRA picture the compositor places like any other layer
//! (`Placement::natural`), centred on the text block so a growing glow or
//! shadow never moves the words.
//!
//! Sizes follow the document: `fontSize` is in text units, one unit being
//! the canvas's short side / 390 (a phone-width preview's point), so a
//! caption is the same share of the picture at any resolution. The look's
//! sliders (0–1) scale with the font size, as in the app.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use cosmic_text::{
    Attrs, Buffer, CacheKeyFlags, Command, Family, FontSystem, Metrics, Shaping, Style, SwashCache,
    SwashContent, Weight, Wrap,
};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform};

use crate::model::Clip;

/// Canvas px per text unit (see the module note).
pub fn unit_px(canvas_w: u32, canvas_h: u32) -> f32 {
    canvas_w.min(canvas_h).max(1) as f32 / 390.
}

/// The text wraps within this share of the canvas width…
const MAX_WIDTH_FRACTION: f32 = 0.86;
/// …less this much padding a side (units).
const PADDING: f32 = 8.;
/// The font's own line height when Line spacing is 0 (× size).
const NATURAL_LINE_HEIGHT: f32 = 1.25;

// Each slider's 0–1 range as multiples of the font size (the app's).
const STROKE_MAX: f32 = 0.2;
const GLOW_SIGMA_MIN: f32 = 0.04;
const GLOW_SIGMA_SPAN: f32 = 0.36;
const SHADOW_DISTANCE_MAX: f32 = 0.5;
const SHADOW_SIGMA_MAX: f32 = 0.25;
const BG_PAD_Y_MIN: f32 = 0.08;
const BG_PAD_Y_SPAN: f32 = 0.6;
const BG_PAD_X_MIN: f32 = 0.12;
const BG_PAD_X_SPAN: f32 = 0.8;
const LINE_HEIGHT_BASE: f32 = 1.2;
const LINE_HEIGHT_SPAN: f32 = 1.3;
pub const LETTER_SPACING_MIN: f64 = -0.1;
pub const LETTER_SPACING_MAX: f64 = 0.6;

/// Where each unset style field rests (the app's text defaults).
pub mod defaults {
    pub const TEXT_OPACITY: f64 = 1.;
    pub const BACKGROUND_OPACITY: f64 = 1.;
    pub const BACKGROUND_RADIUS: f64 = 0.4;
    pub const BACKGROUND_HEIGHT: f64 = 0.3;
    pub const BACKGROUND_WIDTH: f64 = 0.35;
    pub const OUTLINE_WIDTH: f64 = 0.3;
    pub const GLOW_INTENSITY: f64 = 0.7;
    pub const GLOW_RANGE: f64 = 0.4;
    pub const SHADOW_OPACITY: f64 = 0.8;
    pub const SHADOW_BLUR: f64 = 0.4;
    pub const SHADOW_DISTANCE: f64 = 0.2;
    pub const SHADOW_ANGLE: f64 = 45.;
}

/// The fonts a text clip can pick (family as stored, label).
pub const FONTS: &[(&str, &str)] = &[
    ("Nunito_400Regular", "Nunito"),
    ("Nunito_700Bold", "Nunito Bold"),
    ("IBM Plex Mono", "IBM Plex Mono"),
    ("IBMPlexMono_600SemiBold", "Plex Mono SemiBold"),
    ("Georgia", "Georgia"),
    ("Impact", "Impact"),
    ("Helvetica", "Helvetica"),
    ("Courier New", "Courier"),
];

pub fn font_label(family: &str) -> &str {
    FONTS
        .iter()
        .find(|(f, _)| *f == family)
        .map_or(family, |(_, l)| l)
}

/// A stored family ("Nunito_700Bold") → the font family and weight.
fn font_face(family: &str) -> (String, u16) {
    if let Some((name, style)) = family.split_once('_') {
        let digits: String = style.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(weight) = digits.parse::<u16>() {
            let name = match name {
                "IBMPlexMono" => "IBM Plex Mono".to_string(),
                n => n.to_string(),
            };
            return (name, weight);
        }
    }
    (family.to_string(), 400)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BgShape {
    Box,
    Lines,
}

/// A text clip's look, every unset field at its default.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub text: String,
    pub family: String,
    pub size: f64,
    pub color: String,
    pub opacity: f64,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub align: Align,
    /// Colour, thickness 0–1.
    pub stroke: Option<(String, f64)>,
    /// Colour, intensity 0–1, range 0–1.
    pub glow: Option<(String, f64, f64)>,
    pub background: Option<Background>,
    pub shadow: Option<Shadow>,
    /// Fraction of the size (-0.1 … 0.6).
    pub letter_spacing: f64,
    /// 0 = the font's own, 1 = very loose.
    pub line_spacing: f64,
    /// -1 (a smile) … 1 (an arch).
    pub curve: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Background {
    pub shape: BgShape,
    pub color: String,
    pub opacity: f64,
    pub radius: f64,
    pub height: f64,
    pub width: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shadow {
    pub color: String,
    pub opacity: f64,
    pub blur: f64,
    pub distance: f64,
    /// Degrees clockwise from pointing right.
    pub angle: f64,
}

fn num(clip: &Clip, key: &str) -> Option<f64> {
    clip.extra.get(key).and_then(|v| v.as_f64())
}

fn string(clip: &Clip, key: &str) -> Option<String> {
    clip.extra
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn flag(clip: &Clip, key: &str) -> bool {
    clip.extra
        .get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

impl TextStyle {
    pub fn from_clip(clip: &Clip) -> Self {
        use defaults::*;
        TextStyle {
            text: clip.text.clone().unwrap_or_default(),
            family: string(clip, "fontFamily").unwrap_or_else(|| "Nunito_400Regular".into()),
            size: num(clip, "fontSize").unwrap_or(24.).max(1.),
            color: string(clip, "color").unwrap_or_else(|| "#ffffff".into()),
            opacity: num(clip, "textOpacity")
                .unwrap_or(TEXT_OPACITY)
                .clamp(0., 1.),
            bold: flag(clip, "bold"),
            italic: flag(clip, "italic"),
            underline: flag(clip, "underline"),
            align: match clip.extra.get("alignment").and_then(|v| v.as_str()) {
                Some("left") => Align::Left,
                Some("right") => Align::Right,
                _ => Align::Center,
            },
            stroke: string(clip, "outlineColor")
                .map(|c| (c, num(clip, "outlineWidth").unwrap_or(OUTLINE_WIDTH))),
            glow: string(clip, "glowColor").map(|c| {
                (
                    c,
                    num(clip, "glowIntensity").unwrap_or(GLOW_INTENSITY),
                    num(clip, "glowRange").unwrap_or(GLOW_RANGE),
                )
            }),
            background: string(clip, "backgroundColor").map(|color| Background {
                shape: match clip.extra.get("backgroundStyle").and_then(|v| v.as_str()) {
                    Some("lines") => BgShape::Lines,
                    _ => BgShape::Box,
                },
                color,
                opacity: num(clip, "backgroundOpacity").unwrap_or(BACKGROUND_OPACITY),
                radius: num(clip, "backgroundRadius").unwrap_or(BACKGROUND_RADIUS),
                height: num(clip, "backgroundHeight").unwrap_or(BACKGROUND_HEIGHT),
                width: num(clip, "backgroundWidth").unwrap_or(BACKGROUND_WIDTH),
            }),
            shadow: string(clip, "shadowColor").map(|color| Shadow {
                color,
                opacity: num(clip, "shadowOpacity").unwrap_or(SHADOW_OPACITY),
                blur: num(clip, "shadowBlur").unwrap_or(SHADOW_BLUR),
                distance: num(clip, "shadowDistance").unwrap_or(SHADOW_DISTANCE),
                angle: num(clip, "shadowAngle").unwrap_or(SHADOW_ANGLE),
            }),
            letter_spacing: num(clip, "letterSpacing")
                .unwrap_or(0.)
                .clamp(LETTER_SPACING_MIN, LETTER_SPACING_MAX),
            line_spacing: num(clip, "lineSpacing").unwrap_or(0.).clamp(0., 1.),
            curve: num(clip, "curve").unwrap_or(0.).clamp(-1., 1.),
        }
    }

    /// Everything the picture depends on (the cache key).
    pub fn key(&self) -> String {
        format!("{self:?}")
    }
}

/// "#rrggbb" / "#rrggbbaa" → straight RGBA 0–1 (white when it doesn't parse).
fn rgba(s: &str, alpha: f32) -> [f32; 4] {
    let hex = s.trim().trim_start_matches('#');
    let v = u32::from_str_radix(hex, 16).ok();
    let [r, g, b, a] = match (hex.len(), v) {
        (6, Some(v)) => [(v >> 16) & 255, (v >> 8) & 255, v & 255, 255],
        (8, Some(v)) => [(v >> 24) & 255, (v >> 16) & 255, (v >> 8) & 255, v & 255],
        (3, Some(v)) => [
            ((v >> 8) & 15) * 17,
            ((v >> 4) & 15) * 17,
            (v & 15) * 17,
            255,
        ],
        _ => [255, 255, 255, 255],
    };
    [
        r as f32 / 255.,
        g as f32 / 255.,
        b as f32 / 255.,
        a as f32 / 255. * alpha.clamp(0., 1.),
    ]
}

fn paint(color: [f32; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(
        tiny_skia::Color::from_rgba(color[0], color[1], color[2], color[3])
            .unwrap_or(tiny_skia::Color::WHITE),
    );
    p.anti_alias = true;
    p
}

struct Fonts {
    system: FontSystem,
    swash: SwashCache,
}

fn fonts() -> &'static Mutex<Fonts> {
    static FONTS: OnceLock<Mutex<Fonts>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut system = FontSystem::new();
        for data in crate::theme::embedded_fonts() {
            system.db_mut().load_font_data(data.into_owned());
        }
        Mutex::new(Fonts {
            system,
            swash: SwashCache::new(),
        })
    })
}

/// Loads the fonts off the UI thread (the system's take a moment).
pub fn warm_up() {
    std::thread::Builder::new()
        .name("fonts".into())
        .spawn(|| {
            let _ = fonts();
        })
        .ok();
}

/// A drawn text clip: premultiplied BGRA, `px_per_unit` pixels per text
/// unit, centred on the text block.
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    /// The words and their background (not the glow or shadow around
    /// them), px, centred like the picture: the on-canvas selection box.
    pub content: [f32; 2],
}

/// One laid-out line (pixels, block space: y down from the block's top).
struct Line {
    left: f32,
    width: f32,
    top: f32,
    baseline: f32,
}

/// Separable box blur ×3 (≈ Gaussian of `sigma` px) of an alpha mask.
fn blur(mask: &mut [f32], w: usize, h: usize, sigma: f32) {
    if sigma < 0.5 || w == 0 || h == 0 {
        return;
    }
    // Box widths for three passes (the usual sigma → boxes formula).
    let n = 3.;
    let ideal = (12. * sigma * sigma / n + 1.).sqrt();
    let mut wl = ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let m = ((12. * sigma * sigma - n * (wl * wl) as f32 - 4. * n * wl as f32 - 3. * n)
        / (-4. * wl as f32 - 4.))
        .round() as i32;
    let mut tmp = vec![0f32; mask.len()];
    for i in 0..3 {
        let r = (if i < m { wl } else { wu } - 1) / 2;
        box_pass(mask, &mut tmp, w, h, r.max(0) as usize, true);
        box_pass(&tmp, mask, w, h, r.max(0) as usize, false);
    }
}

fn box_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, horizontal: bool) {
    let (len, count, step, stride) = if horizontal {
        (w, h, 1, w)
    } else {
        (h, w, w, 1)
    };
    let norm = 1. / (2 * r + 1) as f32;
    for line in 0..count {
        let base = line * stride;
        let at = |i: isize| -> f32 {
            if i < 0 || i >= len as isize {
                0.
            } else {
                src[base + i as usize * step]
            }
        };
        let mut sum: f32 = (-(r as isize)..=r as isize).map(at).sum();
        for i in 0..len {
            dst[base + i * step] = sum * norm;
            sum += at(i as isize + r as isize + 1) - at(i as isize - r as isize);
        }
    }
}

/// Draws an alpha mask tinted `color` over `target`, moved by (dx, dy).
fn draw_mask(target: &mut Pixmap, mask: &[f32], color: [f32; 4], dx: i32, dy: i32) {
    let (w, h) = (target.width(), target.height());
    let Some(mut layer) = Pixmap::new(w, h) else {
        return;
    };
    let data = layer.data_mut();
    for (i, a) in mask.iter().enumerate() {
        let a = (a.clamp(0., 1.) * color[3]).clamp(0., 1.);
        let o = i * 4;
        data[o] = (color[0] * a * 255.).round() as u8;
        data[o + 1] = (color[1] * a * 255.).round() as u8;
        data[o + 2] = (color[2] * a * 255.).round() as u8;
        data[o + 3] = (a * 255.).round() as u8;
    }
    target.draw_pixmap(
        dx,
        dy,
        layer.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
}

/// The alpha of `path` filled (and stroked `stroke` px wide) on a
/// `w`×`h` mask.
fn path_mask(path: &tiny_skia::Path, stroke: f32, w: u32, h: u32) -> Vec<f32> {
    let Some(mut m) = Pixmap::new(w, h) else {
        return vec![];
    };
    let white = paint([1., 1., 1., 1.]);
    m.fill_path(path, &white, FillRule::Winding, Transform::identity(), None);
    if stroke > 0. {
        let s = Stroke {
            width: stroke,
            line_join: tiny_skia::LineJoin::Round,
            ..Stroke::default()
        };
        m.stroke_path(path, &white, &s, Transform::identity(), None);
    }
    m.pixels().iter().map(|p| p.alpha() as f32 / 255.).collect()
}

/// A rounded rectangle (`roundness` 0–1 of the shorter half side).
fn rounded(r: Rect, roundness: f32) -> Option<tiny_skia::Path> {
    let rad = (r.width().min(r.height()) / 2. * roundness.clamp(0., 1.)).max(0.);
    if rad <= 0.01 {
        return Some(PathBuilder::from_rect(r));
    }
    let k = 0.552_284_8 * rad;
    let (l, t, rr, b) = (r.left(), r.top(), r.right(), r.bottom());
    let mut pb = PathBuilder::new();
    pb.move_to(l + rad, t);
    pb.line_to(rr - rad, t);
    pb.cubic_to(rr - rad + k, t, rr, t + rad - k, rr, t + rad);
    pb.line_to(rr, b - rad);
    pb.cubic_to(rr, b - rad + k, rr - rad + k, b, rr - rad, b);
    pb.line_to(l + rad, b);
    pb.cubic_to(l + rad - k, b, l, b - rad + k, l, b - rad);
    pb.line_to(l, t + rad);
    pb.cubic_to(l, t + rad - k, l + rad - k, t, l + rad, t);
    pb.close();
    pb.finish()
}

/// Draws a text clip. `canvas` is the project's size (the wrap width);
/// `px_per_unit` the output pixels per text unit. None: nothing to draw.
pub fn rasterize(style: &TextStyle, canvas: (u32, u32), px_per_unit: f32) -> Option<Raster> {
    if style.text.trim().is_empty() || px_per_unit <= 0. {
        return None;
    }
    let s = px_per_unit;
    let fs = style.size as f32 * s;
    let max_width = ((canvas.0 as f32 / unit_px(canvas.0, canvas.1)) * MAX_WIDTH_FRACTION
        - PADDING * 2.)
        .max(1.)
        * s;
    let line_height = fs
        * if style.line_spacing > 0. {
            LINE_HEIGHT_BASE + LINE_HEIGHT_SPAN * style.line_spacing as f32
        } else {
            NATURAL_LINE_HEIGHT
        };

    let mut guard = fonts().lock().ok()?;
    let Fonts { system, swash } = &mut *guard;
    let (family, weight) = font_face(&style.family);
    let weight = if style.bold { weight.max(700) } else { weight };
    // Italic: the family's own italic face, else slanted.
    let has_italic = style.italic
        && system.db().faces().any(|f| {
            f.style == cosmic_text::fontdb::Style::Italic
                && f.families
                    .iter()
                    .any(|(n, _)| n.eq_ignore_ascii_case(&family))
        });
    let slant = style.italic && !has_italic;
    let attrs = Attrs::new()
        .family(Family::Name(&family))
        .weight(Weight(weight))
        .style(if has_italic {
            Style::Italic
        } else {
            Style::Normal
        })
        .letter_spacing(style.letter_spacing as f32)
        .cache_key_flags(CacheKeyFlags::empty());
    let mut buffer = Buffer::new(system, Metrics::new(fs, line_height));
    // Curved text sits on one line (the app's rule): no wrapping.
    let curved = style.curve.abs() > 0.01;
    let text = if curved {
        style.text.replace(['\n', '\r'], " ")
    } else {
        style.text.clone()
    };
    buffer.set_wrap(
        system,
        if curved {
            Wrap::None
        } else {
            Wrap::WordOrGlyph
        },
    );
    buffer.set_size(system, (!curved).then_some(max_width), None);
    buffer.set_text(system, &text, &attrs, Shaping::Advanced);
    buffer.shape_until_scroll(system, false);

    // Lines and glyph outlines in block space.
    let mut lines: Vec<Line> = vec![];
    let runs: Vec<_> = buffer.layout_runs().collect();
    let block_w = runs.iter().map(|r| r.line_w).fold(0., f32::max);
    let mut text_path = PathBuilder::new();
    let mut bitmaps: Vec<(f32, f32, cosmic_text::CacheKey)> = vec![];
    for run in &runs {
        let left = match style.align {
            Align::Left => 0.,
            Align::Center => (block_w - run.line_w) / 2.,
            Align::Right => block_w - run.line_w,
        };
        let line = Line {
            left,
            width: run.line_w,
            top: run.line_top,
            baseline: run.line_y,
        };
        // The arc: a glyph at x from the line's middle sits on a circle
        // whose length over the line is |curve| × half a turn.
        let bend = style.curve as f32;
        let radius = if bend.abs() > 0.01 && run.line_w > 0. {
            Some(run.line_w / (bend.abs() * std::f32::consts::PI))
        } else {
            None
        };
        let mid = left + run.line_w / 2.;
        for g in run.glyphs.iter() {
            let phys = g.physical((0., 0.), 1.);
            let gx = left + g.x + g.font_size * g.x_offset;
            let gy = run.line_y - g.font_size * g.y_offset;
            // Glyph placement: translate, then (curved) rotate about its centre.
            let place = match radius {
                Some(r) => {
                    let cx = gx + g.w / 2. - mid;
                    let theta = cx / r;
                    let sign = bend.signum();
                    // Arch (+): the ends drop; smile (-): they rise.
                    let px = mid + r * theta.sin();
                    let py = run.line_y + sign * r * (1. - theta.cos());
                    Transform::from_translate(-g.w / 2., 0.)
                        .post_rotate((sign * theta).to_degrees())
                        .post_translate(px, py)
                }
                None => Transform::from_translate(gx, gy),
            };
            let place = if slant {
                Transform::from_row(1., 0., -0.2, 1., 0., 0.).post_concat(place)
            } else {
                place
            };
            match swash.get_outline_commands(system, phys.cache_key) {
                Some(cmds) if !cmds.is_empty() => {
                    let mut pb = PathBuilder::new();
                    // Outlines are y-up; the block is y-down.
                    for c in cmds {
                        match *c {
                            Command::MoveTo(p) => pb.move_to(p.x, -p.y),
                            Command::LineTo(p) => pb.line_to(p.x, -p.y),
                            Command::QuadTo(a, p) => pb.quad_to(a.x, -a.y, p.x, -p.y),
                            Command::CurveTo(a, b, p) => {
                                pb.cubic_to(a.x, -a.y, b.x, -b.y, p.x, -p.y)
                            }
                            Command::Close => pb.close(),
                        }
                    }
                    if let Some(p) = pb.finish().and_then(|p| p.transform(place)) {
                        text_path.push_path(&p);
                    }
                }
                // No outline (colour emoji): its bitmap, unbent.
                _ => bitmaps.push((gx, gy, phys.cache_key)),
            }
        }
        lines.push(line);
    }
    let block_h = lines.last().map_or(line_height, |l| l.top + line_height);
    let text_path = text_path.finish();

    // What surrounds the words.
    let size_f = fs;
    let stroke_w = style
        .stroke
        .as_ref()
        .map_or(0., |(_, w)| (w.clamp(0., 1.) as f32) * STROKE_MAX * size_f);
    let bg = style.background.as_ref().map(|b| {
        let pad_x = size_f * (BG_PAD_X_MIN + BG_PAD_X_SPAN * b.width.clamp(0., 1.) as f32);
        let pad_y = size_f * (BG_PAD_Y_MIN + BG_PAD_Y_SPAN * b.height.clamp(0., 1.) as f32);
        let rects: Vec<Rect> = match b.shape {
            BgShape::Box => {
                let (l, t, r, bt) = match &text_path {
                    Some(p) if style.curve.abs() > 0.01 => {
                        let bb = p.bounds();
                        (bb.left(), bb.top(), bb.right(), bb.bottom())
                    }
                    _ => (0., 0., block_w, block_h),
                };
                Rect::from_ltrb(l - pad_x, t - pad_y, r + pad_x, bt + pad_y)
                    .into_iter()
                    .collect()
            }
            BgShape::Lines => lines
                .iter()
                .filter(|l| l.width > 0.)
                .filter_map(|l| {
                    Rect::from_ltrb(
                        l.left - pad_x,
                        l.top - pad_y,
                        l.left + l.width + pad_x,
                        l.top + line_height + pad_y,
                    )
                })
                .collect(),
        };
        (b, rects)
    });
    let glow = style.glow.as_ref().map(|(c, k, range)| {
        let strength = k.clamp(0., 1.) as f32;
        let alphas = if strength > 0.5 {
            vec![1., strength * 2. - 1.]
        } else {
            vec![strength * 2.]
        };
        let sigma = size_f * (GLOW_SIGMA_MIN + GLOW_SIGMA_SPAN * range.clamp(0., 1.) as f32);
        (c.clone(), alphas, sigma)
    });
    let shadow = style.shadow.as_ref().map(|sh| {
        let distance = sh.distance.clamp(0., 1.) as f32 * SHADOW_DISTANCE_MAX * size_f;
        let angle = (sh.angle as f32).to_radians();
        let sigma = sh.blur.clamp(0., 1.) as f32 * SHADOW_SIGMA_MAX * size_f;
        (sh, angle.cos() * distance, angle.sin() * distance, sigma)
    });

    // The picture: centred on the block, big enough for every part.
    let (cx, cy) = (block_w / 2., block_h / 2.);
    let mut half_w = block_w / 2.;
    let mut half_h = block_h / 2.;
    let mut grow = |l: f32, t: f32, r: f32, b: f32, margin: f32| {
        half_w = half_w.max(cx - l + margin).max(r - cx + margin);
        half_h = half_h.max(cy - t + margin).max(b - cy + margin);
    };
    // The content box: the same, without what glows or shadows past it.
    let (mut content_w, mut content_h) = (block_w / 2., block_h / 2.);
    let mut grow_content = |l: f32, t: f32, r: f32, b: f32, margin: f32| {
        content_w = content_w.max(cx - l + margin).max(r - cx + margin);
        content_h = content_h.max(cy - t + margin).max(b - cy + margin);
    };
    if let Some(p) = &text_path {
        let b = p.bounds();
        grow_content(b.left(), b.top(), b.right(), b.bottom(), stroke_w / 2.);
    }
    if let Some((_, rects)) = &bg {
        for r in rects {
            grow_content(r.left(), r.top(), r.right(), r.bottom(), 0.);
        }
    }
    let mut effect_margin = stroke_w;
    if let Some((_, _, sigma)) = &glow {
        effect_margin = effect_margin.max(sigma * 3.);
    }
    if let Some((_, dx, dy, sigma)) = &shadow {
        effect_margin = effect_margin.max(sigma * 3. + dx.abs().max(dy.abs()));
    }
    if let Some(p) = &text_path {
        let b = p.bounds();
        grow(b.left(), b.top(), b.right(), b.bottom(), effect_margin + 2.);
    }
    if let Some((_, rects)) = &bg {
        for r in rects {
            grow(r.left(), r.top(), r.right(), r.bottom(), 1.);
        }
    }
    let width = ((half_w * 2.).ceil() as u32).clamp(1, 4096);
    let height = ((half_h * 2.).ceil() as u32).clamp(1, 4096);
    let mut pixmap = Pixmap::new(width, height)?;
    let shift = Transform::from_translate(width as f32 / 2. - cx, height as f32 / 2. - cy);

    // Background (drawn opaque, then faded, so lines that touch merge).
    if let Some((b, rects)) = &bg {
        let mut layer = Pixmap::new(width, height)?;
        let p = paint(rgba(&b.color, 1.));
        for r in rects {
            if let Some(path) = rounded(*r, b.radius as f32) {
                layer.fill_path(&path, &p, FillRule::Winding, shift, None);
            }
        }
        let fade = PixmapPaint {
            opacity: b.opacity.clamp(0., 1.) as f32,
            ..PixmapPaint::default()
        };
        pixmap.draw_pixmap(0, 0, layer.as_ref(), &fade, Transform::identity(), None);
    }

    if let Some(path) = text_path.as_ref().and_then(|p| p.clone().transform(shift)) {
        let (w, h) = (width as usize, height as usize);
        // Shadow, then glow, under the letters.
        if let Some((sh, dx, dy, sigma)) = &shadow {
            let mut m = path_mask(&path, stroke_w, width, height);
            blur(&mut m, w, h, *sigma);
            draw_mask(
                &mut pixmap,
                &m,
                rgba(&sh.color, sh.opacity.clamp(0., 1.) as f32),
                dx.round() as i32,
                dy.round() as i32,
            );
        }
        if let Some((color, alphas, sigma)) = &glow {
            let mut m = path_mask(&path, stroke_w, width, height);
            blur(&mut m, w, h, *sigma);
            for a in alphas {
                draw_mask(&mut pixmap, &m, rgba(color, *a), 0, 0);
            }
        }
        let fill = paint(rgba(&style.color, style.opacity as f32));
        if let Some((color, _)) = &style.stroke
            && stroke_w > 0.
        {
            let s = Stroke {
                width: stroke_w,
                line_join: tiny_skia::LineJoin::Round,
                ..Stroke::default()
            };
            pixmap.stroke_path(
                &path,
                &paint(rgba(color, style.opacity as f32)),
                &s,
                Transform::identity(),
                None,
            );
        }
        pixmap.fill_path(&path, &fill, FillRule::Winding, Transform::identity(), None);
        if style.underline && style.curve.abs() <= 0.01 {
            let thick = (fs * 0.06).max(1.);
            for l in &lines {
                if let Some(r) = Rect::from_xywh(l.left, l.baseline + fs * 0.1, l.width, thick) {
                    pixmap.fill_rect(r, &fill, shift, None);
                }
            }
        }
    }

    // Colour glyphs (emoji) as bitmaps.
    for (gx, gy, key) in bitmaps {
        let Some(img) = swash.get_image(system, key).clone() else {
            continue;
        };
        let (iw, ih) = (img.placement.width, img.placement.height);
        if iw == 0 || ih == 0 {
            continue;
        }
        let ox = (gx + width as f32 / 2. - cx) as i32 + img.placement.left;
        let oy = (gy + height as f32 / 2. - cy) as i32 - img.placement.top;
        let Some(mut glyph) = Pixmap::new(iw, ih) else {
            continue;
        };
        let alpha = style.opacity.clamp(0., 1.) as f32;
        let tint = rgba(&style.color, 1.);
        for (i, px) in glyph
            .data_mut()
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .enumerate()
        {
            let (r, g, b, a) = match img.content {
                SwashContent::Color => {
                    let o = i * 4;
                    (
                        img.data[o],
                        img.data[o + 1],
                        img.data[o + 2],
                        img.data[o + 3],
                    )
                }
                _ => {
                    let a = img.data.get(i).copied().unwrap_or(0);
                    (
                        (tint[0] * 255.) as u8,
                        (tint[1] * 255.) as u8,
                        (tint[2] * 255.) as u8,
                        a,
                    )
                }
            };
            let a = a as f32 / 255. * alpha;
            px[0] = (r as f32 * a) as u8;
            px[1] = (g as f32 * a) as u8;
            px[2] = (b as f32 * a) as u8;
            px[3] = (a * 255.) as u8;
        }
        pixmap.draw_pixmap(
            ox,
            oy,
            glyph.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }

    // Premultiplied RGBA → premultiplied BGRA.
    let mut bgra = pixmap.take();
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    Some(Raster {
        width,
        height,
        bgra,
        content: [
            (content_w * 2.).min(width as f32),
            (content_h * 2.).min(height as f32),
        ],
    })
}

/// Recently drawn text pictures, by what they depend on (a slider drag
/// redraws one per step; a playing timeline reuses them).
#[derive(Default)]
pub struct RasterCache {
    entries: HashMap<String, (std::sync::Arc<crate::decoder::DecodedFrame>, [f32; 2])>,
    order: Vec<String>,
}

impl RasterCache {
    const KEEP: usize = 24;

    pub fn get_or_draw(
        &mut self,
        style: &TextStyle,
        canvas: (u32, u32),
        px_per_unit: f32,
    ) -> Option<(std::sync::Arc<crate::decoder::DecodedFrame>, [f32; 2])> {
        let key = format!("{}|{canvas:?}|{px_per_unit:.4}", style.key());
        if let Some(f) = self.entries.get(&key) {
            return Some(f.clone());
        }
        let r = rasterize(style, canvas, px_per_unit)?;
        let frame = std::sync::Arc::new(crate::decoder::DecodedFrame {
            source: String::new(),
            key: key.clone(),
            pts: 0,
            width: r.width,
            height: r.height,
            bgra: r.bgra,
        });
        self.entries.insert(key.clone(), (frame.clone(), r.content));
        self.order.push(key);
        while self.order.len() > Self::KEEP {
            let old = self.order.remove(0);
            self.entries.remove(&old);
        }
        Some((frame, r.content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(text: &str) -> TextStyle {
        let mut p = crate::model::Project::empty();
        let id = p.add_text_clip(0, text);
        TextStyle::from_clip(p.find_clip(&id).unwrap().1)
    }

    fn opaque_bounds(r: &Raster) -> Option<(u32, u32, u32, u32)> {
        let (mut l, mut t, mut rr, mut b) = (u32::MAX, u32::MAX, 0, 0);
        for y in 0..r.height {
            for x in 0..r.width {
                if r.bgra[((y * r.width + x) * 4 + 3) as usize] > 128 {
                    l = l.min(x);
                    t = t.min(y);
                    rr = rr.max(x);
                    b = b.max(y);
                }
            }
        }
        (l <= rr).then_some((l, t, rr, b))
    }

    #[test]
    fn families_map_to_faces() {
        assert_eq!(font_face("Nunito_700Bold"), ("Nunito".into(), 700));
        assert_eq!(
            font_face("IBMPlexMono_600SemiBold"),
            ("IBM Plex Mono".into(), 600)
        );
        assert_eq!(font_face("Georgia"), ("Georgia".into(), 400));
    }

    #[test]
    fn draws_white_words_centred() {
        let s = style("Hello");
        let r = rasterize(&s, (1080, 1920), 2.).unwrap();
        let (l, t, rr, b) = opaque_bounds(&r).expect("some ink");
        // Centred: the ink's middle near the picture's middle.
        let mid_x = (l + rr) as f32 / 2.;
        assert!(
            (mid_x - r.width as f32 / 2.).abs() < r.width as f32 * 0.1,
            "{mid_x} of {}",
            r.width
        );
        assert!(b > t);
        // White, premultiplied (B = G = R = A on full ink).
        let i = (((t + b) / 2 * r.width + (l + rr) / 2) * 4) as usize;
        let px = &r.bgra[i..i + 4];
        assert!(px[3] == 0 || (px[0] == px[3] && px[2] == px[3]), "{px:?}");
    }

    #[test]
    fn a_background_and_shadow_grow_the_picture() {
        let plain = rasterize(&style("Hi"), (1080, 1920), 2.).unwrap();
        let mut s = style("Hi");
        s.background = Some(Background {
            shape: BgShape::Box,
            color: "#000000".into(),
            opacity: 1.,
            radius: 0.4,
            height: 0.3,
            width: 0.35,
        });
        let boxed = rasterize(&s, (1080, 1920), 2.).unwrap();
        assert!(boxed.width > plain.width && boxed.height > plain.height);
        // Its corner region is opaque black (premultiplied 0,0,0,255) near
        // the box's edge, not the picture's corner (rounded).
        let mid = ((boxed.height / 2 * boxed.width + 2) * 4) as usize;
        assert_eq!(boxed.bgra[mid + 3], 255);
    }

    #[test]
    fn long_text_wraps_inside_the_canvas() {
        let s = style(&"word ".repeat(60));
        let r = rasterize(&s, (1080, 1920), 1.).unwrap();
        let unit = unit_px(1080, 1920);
        // At 1 px per unit the block is at most 86 % of the canvas width.
        assert!((r.width as f32) < 1080. / unit * 0.9, "{}", r.width);
        assert!(r.height > 100);
    }

    #[test]
    fn empty_text_draws_nothing() {
        assert!(rasterize(&style("  "), (1080, 1920), 2.).is_none());
    }

    #[test]
    fn blur_keeps_mass() {
        let (w, h) = (40, 40);
        let mut m = vec![0f32; w * h];
        m[20 * w + 20] = 1.;
        blur(&mut m, w, h, 3.);
        let total: f32 = m.iter().sum();
        assert!((total - 1.).abs() < 0.05, "{total}");
        assert!(m[20 * w + 20] < 0.2);
    }
}
