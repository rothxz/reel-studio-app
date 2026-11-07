//! A visual clip's look in the document: its transform, mirror / flip,
//! shape mask, Splice blend mode, Background (main lane) and Remove BG
//! cutout, in the same JSON fields as the app (`Clip::extra`). And the
//! keyframe-aware write of the animatable numbers: on a clip that has
//! keyframes an edit lands on the diamond under the playhead, or adds one
//! (auto keyframe), as the inspector does in the old editor.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::model::{Clip, Micros, new_id};

// ---- transform ---------------------------------------------------------------

/// Where the clip sits: `x` / `y` in canvas widths / heights from the
/// centre (y down), `scale` 1 = contain-fit, `rotation` in degrees,
/// `opacity` 0–1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
    pub rotation: f64,
    pub opacity: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Transform {
            x: 0.,
            y: 0.,
            scale: 1.,
            rotation: 0.,
            opacity: 1.,
        }
    }
}

// ---- mask -----------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MaskShape {
    Horizontal,
    Mirror,
    Circle,
    Rectangle,
    Heart,
    Star,
}

impl MaskShape {
    pub const ALL: [MaskShape; 6] = [
        MaskShape::Horizontal,
        MaskShape::Mirror,
        MaskShape::Circle,
        MaskShape::Rectangle,
        MaskShape::Heart,
        MaskShape::Star,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MaskShape::Horizontal => "Horizontal",
            MaskShape::Mirror => "Mirror",
            MaskShape::Circle => "Circle",
            MaskShape::Rectangle => "Rectangle",
            MaskShape::Heart => "Heart",
            MaskShape::Star => "Stars",
        }
    }

    /// Shapes with a width (a straight edge has no size; a mirror band
    /// only a thickness, its height).
    pub fn has_width(self) -> bool {
        matches!(
            self,
            MaskShape::Circle | MaskShape::Rectangle | MaskShape::Heart | MaskShape::Star
        )
    }

    pub fn has_height(self) -> bool {
        self.has_width() || self == MaskShape::Mirror
    }
}

/// A shape that hides part of the clip, in the clip's own 0–1 frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipMask {
    pub shape: MaskShape,
    pub center_x: f64,
    pub center_y: f64,
    pub width: f64,
    pub height: f64,
    /// Degrees, clockwise.
    pub rotation: f64,
    /// Edge softness, 0–1.
    pub feather: f64,
    pub invert: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The mask's animatable fields (`mask.<field>` keyframe tracks).
pub const MASK_FIELDS: [&str; 6] = [
    "centerX", "centerY", "width", "height", "rotation", "feather",
];

impl ClipMask {
    /// A fresh mask: centred, 60 % of the frame's shorter side and square
    /// on screen (`aspect` = the clip's width / height); a mirror is a
    /// band.
    pub fn new(shape: MaskShape, aspect: f64) -> Self {
        let side = 0.6 * aspect.min(1.);
        let (width, height) = match shape {
            MaskShape::Mirror => (0.6, 0.3),
            _ if aspect > 0. => (side / aspect, side),
            _ => (0.6, 0.6),
        };
        ClipMask {
            shape,
            center_x: 0.5,
            center_y: 0.5,
            width,
            height,
            rotation: 0.,
            feather: 0.,
            invert: false,
            extra: Map::new(),
        }
    }

    pub fn get(&self, field: &str) -> Option<f64> {
        Some(match field {
            "centerX" => self.center_x,
            "centerY" => self.center_y,
            "width" => self.width,
            "height" => self.height,
            "rotation" => self.rotation,
            "feather" => self.feather,
            _ => return None,
        })
    }

    fn set(&mut self, field: &str, v: f64) {
        match field {
            "centerX" => self.center_x = v,
            "centerY" => self.center_y = v,
            "width" => self.width = v,
            "height" => self.height = v,
            "rotation" => self.rotation = v,
            "feather" => self.feather = v,
            _ => {}
        }
    }
}

// ---- splice (blend) -------------------------------------------------------------------

/// How an overlay combines with what's under it: (id in the document, label).
pub const BLEND_MODES: [(&str, &str); 11] = [
    ("normal", "Normal"),
    ("darken", "Darken"),
    ("lighten", "Lighten"),
    ("filter", "Filter"),
    ("overlay", "Overlay"),
    ("hard", "Hard"),
    ("soft", "Soft"),
    ("burn", "Burn"),
    ("colorBurn", "Color burn"),
    ("dodge", "Dodge"),
    ("slide", "Slide"),
];

// ---- background ------------------------------------------------------------------------

/// What fills the canvas around a main-lane clip's picture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Background {
    Color {
        color: String,
    },
    Pattern {
        id: String,
    },
    Image {
        uri: String,
        width: f64,
        height: f64,
    },
    Blur {
        level: u8,
    },
}

impl Background {
    /// The same pick (colours compare case-blind).
    pub fn same(a: Option<&Background>, b: Option<&Background>) -> bool {
        match (a, b) {
            (Some(Background::Color { color: x }), Some(Background::Color { color: y })) => {
                x.eq_ignore_ascii_case(y)
            }
            _ => a == b,
        }
    }
}

// ---- cutout stroke --------------------------------------------------------------------------

/// The outline around a cut-out subject.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CutoutStroke {
    pub style: String,
    pub color: String,
    /// 0–100.
    pub size: f64,
    /// 0–100.
    pub spacing: f64,
}

// ---- properties -------------------------------------------------------------------------------

/// The limits a keyed value is held to (the app's property registry).
fn clamp_property(id: &str, v: f64) -> f64 {
    let (lo, hi) = match id {
        "transform.scale" => (0.01, 10.),
        "transform.opacity" => (0., 1.),
        "mask.centerX" | "mask.centerY" | "mask.feather" => (0., 1.),
        "mask.width" | "mask.height" => (0.05, 1.),
        _ => match id
            .strip_prefix(crate::adjust::PROPERTY_PREFIX)
            .and_then(crate::adjust::control)
        {
            Some(c) => (c.min, c.max),
            None => return v,
        },
    };
    v.clamp(lo, hi)
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
}

impl Clip {
    /// A video or a photo (what Visual's panels edit).
    pub fn is_visual(&self) -> bool {
        self.kind == "video" || self.kind == "image"
    }

    pub fn transform(&self) -> Transform {
        let d = Transform::default();
        let t = self.extra.get("transform");
        let f = |k: &str, def: f64| num(t.and_then(|t| t.get(k))).unwrap_or(def);
        Transform {
            x: f("x", d.x),
            y: f("y", d.y),
            scale: f("scale", d.scale),
            rotation: f("rotation", d.rotation),
            opacity: f("opacity", d.opacity),
        }
    }

    fn set_transform_field(&mut self, field: &str, v: f64) {
        let t = self
            .extra
            .entry("transform")
            .or_insert_with(|| json!({ "x": 0, "y": 0, "scale": 1, "rotation": 0, "opacity": 1 }));
        if let Some(o) = t.as_object_mut() {
            o.insert(field.into(), json!(v));
        }
    }

    /// Mirror (`vertical` false: left ↔ right) or Flip (upside down).
    pub fn flipped(&self, vertical: bool) -> bool {
        let key = if vertical {
            "flipVertical"
        } else {
            "flipHorizontal"
        };
        self.extra
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    pub fn set_flipped(&mut self, vertical: bool, on: bool) {
        let key = if vertical {
            "flipVertical"
        } else {
            "flipHorizontal"
        };
        self.extra.insert(key.into(), Value::Bool(on));
    }

    pub fn mask(&self) -> Option<ClipMask> {
        serde_json::from_value(self.extra.get("mask")?.clone()).ok()
    }

    pub fn set_mask(&mut self, mask: Option<&ClipMask>) {
        match mask.and_then(|m| serde_json::to_value(m).ok()) {
            Some(v) => self.extra.insert("mask".into(), v),
            None => self.extra.remove("mask"),
        };
    }

    pub fn blend_mode(&self) -> &str {
        self.extra
            .get("blendMode")
            .and_then(Value::as_str)
            .unwrap_or("normal")
    }

    /// "normal" is the default and isn't written.
    pub fn set_blend_mode(&mut self, mode: &str) {
        if mode == "normal" {
            self.extra.remove("blendMode");
        } else {
            self.extra.insert("blendMode".into(), json!(mode));
        }
    }

    pub fn background(&self) -> Option<Background> {
        serde_json::from_value(self.extra.get("background")?.clone()).ok()
    }

    pub fn set_background(&mut self, bg: Option<&Background>) {
        match bg.and_then(|b| serde_json::to_value(b).ok()) {
            Some(v) => self.extra.insert("background".into(), v),
            None => self.extra.remove("background"),
        };
    }

    /// The cutout while it removes the background (a matte kept only for
    /// body effects doesn't).
    pub fn cutout(&self) -> Option<&Map<String, Value>> {
        let c = self.extra.get("cutout")?.as_object()?;
        (!c.get("matteOnly").and_then(Value::as_bool).unwrap_or(false)).then_some(c)
    }

    /// Stops removing the background. With `keep_matte` (a body effect
    /// reads a video's person matte) the mask stays, marked matte-only.
    pub fn remove_cutout(&mut self, keep_matte: bool) -> bool {
        let Some(c) = self.extra.get_mut("cutout") else {
            return false;
        };
        let video = c.get("kind").and_then(Value::as_str) == Some("video");
        if keep_matte && video {
            c["matteOnly"] = Value::Bool(true);
        } else {
            self.extra.remove("cutout");
        }
        true
    }

    pub fn cutout_stroke(&self) -> Option<CutoutStroke> {
        serde_json::from_value(self.cutout()?.get("stroke")?.clone()).ok()
    }

    pub fn set_cutout_stroke(&mut self, stroke: Option<&CutoutStroke>) -> bool {
        let Some(c) = self.extra.get_mut("cutout").and_then(Value::as_object_mut) else {
            return false;
        };
        match stroke.and_then(|s| serde_json::to_value(s).ok()) {
            Some(v) => c.insert("stroke".into(), v),
            None => c.remove("stroke"),
        };
        true
    }

    /// A property's own (un-keyed) value: `transform.<field>`,
    /// `mask.<field>` (None without a mask) or `adjust.<slider>`.
    pub fn static_property(&self, id: &str) -> Option<f64> {
        if let Some(f) = id.strip_prefix("transform.") {
            let t = self.transform();
            return Some(match f {
                "x" => t.x,
                "y" => t.y,
                "scale" => t.scale,
                "rotation" => t.rotation,
                "opacity" => t.opacity,
                _ => return None,
            });
        }
        if let Some(f) = id.strip_prefix("mask.") {
            return self.mask()?.get(f);
        }
        if let Some(key) = id.strip_prefix(crate::adjust::PROPERTY_PREFIX) {
            return crate::adjust::control(key).map(|_| self.adjust_value(key));
        }
        None
    }

    fn write_static(&mut self, id: &str, v: f64) {
        if let Some(f) = id.strip_prefix("transform.") {
            self.set_transform_field(f, v);
        } else if let Some(f) = id.strip_prefix("mask.")
            && let Some(mut m) = self.mask()
        {
            m.set(f, v);
            self.set_mask(Some(&m));
        } else if let Some(key) = id.strip_prefix(crate::adjust::PROPERTY_PREFIX) {
            self.set_adjust_value(key, v);
        }
    }

    /// Whether any property has keyframes (edits then go through them).
    pub fn is_animated(&self) -> bool {
        self.tracks().iter().any(|t| {
            t.get("keyframes")
                .and_then(Value::as_array)
                .is_some_and(|k| !k.is_empty())
        })
    }

    fn tracks(&self) -> &[Value] {
        self.extra
            .get("keyframes")
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }

    /// The animated (enabled, non-empty) tracks' property names.
    pub fn animated_properties(&self) -> Vec<String> {
        self.tracks()
            .iter()
            .filter(|t| live_track(t))
            .filter_map(|t| t.get("property").and_then(Value::as_str).map(String::from))
            .collect()
    }

    /// The diamond within `eps` of `t` nearest it.
    pub fn diamond_near(&self, t: Micros, eps: Micros) -> Option<Micros> {
        self.keyframe_times(eps)
            .into_iter()
            .filter(|d| (d - t).abs() <= eps)
            .min_by_key(|d| (d - t).abs())
    }

    /// Sets properties at clip-local `t`. A property with keyframes gets
    /// the diamond under `t` updated, or (`auto_key`) a new one; one
    /// without keyframes on a clip that has diamonds joins the animation
    /// the same way (keyed at every diamond with its old value first);
    /// otherwise the plain value changes.
    pub fn set_properties_at(
        &mut self,
        patch: &[(&str, f64)],
        t: Micros,
        auto_key: bool,
        eps: Micros,
    ) {
        let diamonds = self.keyframe_times(eps);
        let on = self.diamond_near(t, eps);
        for &(id, raw) in patch {
            if !raw.is_finite() {
                continue;
            }
            // Keyed values are held to the property's limits; a plain one
            // (zoom typed past 1000 %) isn't, as in the old editor.
            let value = clamp_property(id, raw);
            let mut tracks = self.tracks().to_vec();
            let index = tracks
                .iter()
                .position(|tr| tr.get("property").and_then(Value::as_str) == Some(id));
            let animated = index.is_some_and(|i| live_track(&tracks[i]));
            if animated {
                let i = index.unwrap();
                if on.is_some() || near_index(&tracks[i], t, eps).is_some() || auto_key {
                    upsert(&mut tracks[i], on.unwrap_or(t), value, eps);
                    self.extra.insert("keyframes".into(), Value::Array(tracks));
                } else {
                    self.write_static(id, raw);
                }
            } else if !diamonds.is_empty() && (on.is_some() || auto_key) {
                let Some(old) = self.static_property(id) else {
                    continue;
                };
                let mut fresh = match index {
                    Some(i) => tracks[i].clone(),
                    None => json!({ "property": id }),
                };
                fresh["keyframes"] = json!([]);
                fresh["enabled"] = Value::Bool(true);
                for &d in &diamonds {
                    upsert(&mut fresh, d, old, eps);
                }
                upsert(&mut fresh, on.unwrap_or(t), value, eps);
                match index {
                    Some(i) => tracks[i] = fresh,
                    None => tracks.push(fresh),
                }
                self.extra.insert("keyframes".into(), Value::Array(tracks));
            } else {
                self.write_static(id, raw);
            }
        }
    }

    /// A diamond at `t` (or the one near it): every given property keyed
    /// with its value there.
    pub fn add_keyframe_at(&mut self, t: Micros, eps: Micros, values: &[(String, f64)]) {
        let at = self.diamond_near(t, eps).unwrap_or(t);
        let mut tracks = self.tracks().to_vec();
        for (id, v) in values {
            let i = match tracks
                .iter()
                .position(|tr| tr.get("property").and_then(Value::as_str) == Some(id.as_str()))
            {
                Some(i) => i,
                None => {
                    tracks.push(json!({ "property": id, "keyframes": [], "enabled": true }));
                    tracks.len() - 1
                }
            };
            upsert(&mut tracks[i], at, *v, eps);
        }
        self.extra.insert("keyframes".into(), Value::Array(tracks));
    }

    /// Deletes every track's keyframe within `eps` of `t`.
    pub fn delete_keyframes_at(&mut self, t: Micros, eps: Micros) -> bool {
        let mut changed = false;
        if let Some(tracks) = self
            .extra
            .get_mut("keyframes")
            .and_then(Value::as_array_mut)
        {
            for tr in tracks {
                if let Some(kfs) = tr.get_mut("keyframes").and_then(Value::as_array_mut) {
                    let n = kfs.len();
                    kfs.retain(|k| {
                        num(k.get("time")).is_none_or(|kt| (kt as Micros - t).abs() > eps)
                    });
                    changed |= kfs.len() != n;
                }
            }
        }
        changed
    }

    /// Drops the keyframe tracks of the given properties.
    pub fn clear_tracks(&mut self, ids: &[String]) {
        if let Some(tracks) = self
            .extra
            .get_mut("keyframes")
            .and_then(Value::as_array_mut)
        {
            tracks.retain(|tr| {
                !tr.get("property")
                    .and_then(Value::as_str)
                    .is_some_and(|p| ids.iter().any(|id| id == p))
            });
        }
    }
}

fn live_track(t: &Value) -> bool {
    t.get("enabled").and_then(Value::as_bool).unwrap_or(true)
        && t.get("keyframes")
            .and_then(Value::as_array)
            .is_some_and(|k| !k.is_empty())
}

fn near_index(track: &Value, t: Micros, eps: Micros) -> Option<usize> {
    let kfs = track.get("keyframes")?.as_array()?;
    kfs.iter()
        .enumerate()
        .filter_map(|(i, k)| Some((i, (num(k.get("time"))? as Micros - t).abs())))
        .filter(|&(_, d)| d <= eps)
        .min_by_key(|&(_, d)| d)
        .map(|(i, _)| i)
}

/// Sets the keyframe near `t` to `value`, or inserts one there (eased
/// like the keyframe before it), keeping them sorted.
fn upsert(track: &mut Value, t: Micros, value: f64, eps: Micros) {
    if let Some(i) = near_index(track, t, eps) {
        track["keyframes"][i]["value"] = json!(value);
        return;
    }
    if !track.get("keyframes").is_some_and(Value::is_array) {
        track["keyframes"] = json!([]);
    }
    let kfs = track["keyframes"].as_array_mut().unwrap();
    let at = kfs
        .iter()
        .position(|k| num(k.get("time")).is_some_and(|kt| kt as Micros > t))
        .unwrap_or(kfs.len());
    let mut kf = json!({ "id": new_id("kf"), "time": t, "value": value, "easing": "linear" });
    if at > 0 {
        let prev = &kfs[at - 1];
        for key in ["easing", "bezier", "graph"] {
            if let Some(v) = prev.get(key) {
                kf[key] = v.clone();
            }
        }
    }
    kfs.insert(at, kf);
}

/// A property's keyframes at clip-local `t`, interpolated linearly (holds
/// hold): the stand-in engine's answer; the C++ engine eases.
#[cfg_attr(feature = "cpp-engine", allow(dead_code))]
pub fn linear_track_value(clip: &Clip, property: &str, t: Micros) -> Option<f64> {
    let track = clip.tracks().iter().find(|tr| {
        live_track(tr) && tr.get("property").and_then(Value::as_str) == Some(property)
    })?;
    let mut kfs: Vec<(f64, f64, bool)> = track["keyframes"]
        .as_array()?
        .iter()
        .filter_map(|k| {
            Some((
                num(k.get("time"))?,
                num(k.get("value"))?,
                k.get("easing").and_then(Value::as_str) == Some("hold"),
            ))
        })
        .collect();
    kfs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let t = t as f64;
    let (first, last) = (kfs.first()?, kfs.last()?);
    if t <= first.0 {
        return Some(first.1);
    }
    if t >= last.0 {
        return Some(last.1);
    }
    let i = kfs.iter().rposition(|k| k.0 <= t)?;
    let (a, b) = (kfs[i], kfs[i + 1]);
    if a.2 || b.0 <= a.0 {
        return Some(a.1);
    }
    Some(a.1 + (b.1 - a.1) * (t - a.0) / (b.0 - a.0))
}

// ---- rotate 90° ---------------------------------------------------------------------------

/// Degrees into (-180, 180].
pub fn normalize_degrees(d: f64) -> f64 {
    let d = d.rem_euclid(360.);
    if d > 180. { d - 360. } else { d }
}

/// The frame's on-screen box at `rotation` (to the nearest quarter).
fn footprint(frame: (f64, f64), rotation: f64) -> (f64, f64) {
    let quarter = ((rotation / 90.).round() as i64).rem_euclid(4);
    if quarter % 2 == 1 {
        (frame.1, frame.0)
    } else {
        frame
    }
}

/// The scale at which the frame turned to `rotation` exactly fits `canvas`
/// (the renderers contain-fit the upright frame, then spin it).
fn turned_fit(frame: (f64, f64), canvas: (f64, f64), rotation: f64) -> f64 {
    let base = (canvas.0 / frame.0).min(canvas.1 / frame.1);
    let fp = footprint(frame, rotation);
    (canvas.0 / fp.0).min(canvas.1 / fp.1) / base
}

pub struct QuarterTurn {
    pub rotation: f64,
    pub scale: f64,
    /// The canvas turned with the clip (it had the clip's shape).
    pub canvas: Option<(u32, u32)>,
}

/// A quarter turn (`delta` ±90) that keeps the clip framed as it was: its
/// scale stays the same multiple of "exactly fits". With
/// `canvas_follows` (the main lane's only clip) a canvas of the clip's
/// own shape turns too.
pub fn quarter_turn(
    frame: (f64, f64),
    canvas: (u32, u32),
    rotation: f64,
    scale: f64,
    delta: f64,
    canvas_follows: bool,
) -> QuarterTurn {
    let next = normalize_degrees(rotation + delta);
    let c = (canvas.0 as f64, canvas.1 as f64);
    if frame.0 <= 0. || frame.1 <= 0. || c.0 <= 0. || c.1 <= 0. {
        return QuarterTurn {
            rotation: next,
            scale,
            canvas: None,
        };
    }
    let fp = footprint(frame, rotation);
    let same_shape = ((c.0 / c.1) / (fp.0 / fp.1) - 1.).abs() < 0.02;
    let turn_canvas = canvas_follows && canvas.0 != canvas.1 && same_shape;
    let next_canvas = if turn_canvas { (c.1, c.0) } else { c };
    let relative = scale / turned_fit(frame, c, rotation);
    let scale = relative * turned_fit(frame, next_canvas, next);
    QuarterTurn {
        rotation: next,
        scale: (scale * 10_000.).round() / 10_000.,
        canvas: turn_canvas.then_some((canvas.1, canvas.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(extra: Value) -> Clip {
        let mut v = json!({
            "id": "c", "kind": "video", "timelineStart": 0, "timelineDuration": 4_000_000,
        });
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn transform_defaults_and_writes_keep_other_fields() {
        let mut c = clip(json!({}));
        assert_eq!(c.transform(), Transform::default());
        c.set_properties_at(&[("transform.x", 0.25)], 0, true, 50_000);
        assert_eq!(c.transform().x, 0.25);
        assert_eq!(c.transform().scale, 1.);
        assert!(!c.is_animated());
    }

    #[test]
    fn mask_round_trips_and_defaults_square_on_screen() {
        let m = ClipMask::new(MaskShape::Circle, 16. / 9.);
        assert!((m.height - 0.6).abs() < 1e-9);
        assert!((m.width - 0.6 * 9. / 16.).abs() < 1e-9);
        let mut c = clip(json!({}));
        c.set_mask(Some(&m));
        assert_eq!(c.extra["mask"]["shape"], "circle");
        assert_eq!(c.extra["mask"]["centerX"], 0.5);
        assert_eq!(c.mask(), Some(m));
        assert_eq!(ClipMask::new(MaskShape::Mirror, 1.).height, 0.3);
    }

    #[test]
    fn background_json_is_the_apps() {
        let bg = Background::Blur { level: 2 };
        assert_eq!(
            serde_json::to_value(&bg).unwrap(),
            json!({ "kind": "blur", "level": 2 })
        );
        let c = clip(json!({ "background": { "kind": "color", "color": "#FF0000" } }));
        assert!(Background::same(
            c.background().as_ref(),
            Some(&Background::Color {
                color: "#ff0000".into()
            })
        ));
    }

    #[test]
    fn edits_on_an_animated_clip_key_the_playhead() {
        let mut c = clip(json!({ "keyframes": [
            { "property": "transform.scale", "enabled": true, "keyframes": [
                { "id": "a", "time": 0, "value": 1, "easing": "easeIn" },
                { "id": "b", "time": 2_000_000, "value": 2, "easing": "linear" } ] } ] }));
        // On a diamond: its value changes.
        c.set_properties_at(&[("transform.scale", 3.)], 2_010_000, false, 50_000);
        assert_eq!(c.extra["keyframes"][0]["keyframes"][1]["value"], 3.);
        // Between diamonds, auto keyframe: a new one, eased like the one before.
        c.set_properties_at(&[("transform.scale", 1.5)], 1_000_000, true, 50_000);
        let kfs = c.extra["keyframes"][0]["keyframes"].as_array().unwrap();
        assert_eq!(kfs.len(), 3);
        assert_eq!(kfs[1]["time"], 1_000_000);
        assert_eq!(kfs[1]["easing"], "easeIn");
        // A property without keyframes joins in at every diamond.
        c.set_properties_at(&[("transform.x", 0.5)], 0, true, 50_000);
        let x = &c.extra["keyframes"][1];
        assert_eq!(x["property"], "transform.x");
        assert_eq!(x["keyframes"].as_array().unwrap().len(), 3);
        assert_eq!(x["keyframes"][0]["value"], 0.5);
        assert_eq!(x["keyframes"][1]["value"], 0.);
        // Between diamonds without auto keyframe: the plain value.
        c.set_properties_at(&[("transform.opacity", 0.5)], 1_500_000, false, 50_000);
        assert_eq!(c.transform().opacity, 0.5);
        // Scale is held to its limits when keyed.
        c.set_properties_at(&[("transform.scale", 50.)], 0, false, 50_000);
        assert_eq!(c.extra["keyframes"][0]["keyframes"][0]["value"], 10.);
    }

    #[test]
    fn diamonds_add_and_delete() {
        let mut c = clip(json!({}));
        c.add_keyframe_at(1_000_000, 50_000, &[("transform.x".into(), 0.)]);
        assert!(c.is_animated());
        assert_eq!(linear_track_value(&c, "transform.x", 0), Some(0.));
        assert!(c.delete_keyframes_at(1_020_000, 50_000));
        assert!(!c.is_animated());
    }

    #[test]
    fn stand_in_interpolates_linearly() {
        let c = clip(json!({ "keyframes": [
            { "property": "transform.x", "enabled": true, "keyframes": [
                { "id": "a", "time": 0, "value": 0 },
                { "id": "b", "time": 1_000_000, "value": 1 } ] } ] }));
        assert_eq!(linear_track_value(&c, "transform.x", 250_000), Some(0.25));
        assert_eq!(linear_track_value(&c, "transform.x", 5_000_000), Some(1.));
        assert_eq!(linear_track_value(&c, "transform.y", 0), None);
    }

    #[test]
    fn quarter_turn_keeps_the_fit_and_turns_a_matching_canvas() {
        // A landscape clip filling a landscape canvas: the canvas turns and
        // the clip still fills it (scale is over the upright contain-fit,
        // which is small in a portrait canvas).
        let t = quarter_turn((1920., 1080.), (1920, 1080), 0., 1., 90., true);
        assert_eq!(t.rotation, 90.);
        assert_eq!(t.canvas, Some((1080, 1920)));
        assert!((t.scale - 1.7778).abs() < 1e-3);
        // An overlay turned in a landscape canvas: shrunk to keep fitting.
        let t = quarter_turn((1920., 1080.), (1920, 1080), 0., 1., -90., false);
        assert_eq!(t.rotation, -90.);
        assert_eq!(t.canvas, None);
        assert!((t.scale - 0.5625).abs() < 1e-3);
        assert_eq!(normalize_degrees(270.), -90.);
        assert_eq!(normalize_degrees(-180.), 180.);
    }

    #[test]
    fn cutout_removal_keeps_a_matte_for_body_effects() {
        let mut c = clip(json!({ "cutout": { "kind": "video", "mode": "person",
            "stroke": { "style": "solid", "color": "#ffffff", "size": 20, "spacing": 0 } } }));
        assert!(c.cutout().is_some());
        assert_eq!(c.cutout_stroke().unwrap().size, 20.);
        assert!(c.remove_cutout(true));
        assert!(c.cutout().is_none());
        assert_eq!(c.extra["cutout"]["matteOnly"], true);
    }
}
