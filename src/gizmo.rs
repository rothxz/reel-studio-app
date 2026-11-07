//! The on-canvas manipulator's math: where a layer's box sits on the
//! canvas, hit tests, and what a drag does to its transform (move with
//! centre snapping, the corner handle's scale + turn about the centre with
//! rotation snapping). The player draws and drives it (`ui/gizmo.rs`).
//!
//! Space: canvas px of the project canvas, +y down, rotation clockwise.
//! Tolerances are given in screen points and divided by the view scale
//! (screen points per canvas px) by the caller.

/// A layer's transform as the gizmo edits it: centre offset in canvas
/// widths / heights (+y down), zoom over its fit, degrees clockwise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xform {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub rotation: f32,
}

/// Where a layer is on the canvas: centre, size, clockwise turn (radians).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GizmoBox {
    pub cx: f32,
    pub cy: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
}

/// How a layer's size follows from its picture (the compositor's rule).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// Contain-fitted in the canvas: the picture's (cropped) width, height.
    Fit { width: f32, height: f32 },
    /// Its own size at zoom 1, in canvas widths / heights (text, stickers).
    Natural { width: f32, height: f32 },
}

pub const MIN_SCALE: f32 = 0.1;
/// Far enough that one letter of a caption can fill the whole preview.
pub const MAX_SCALE: f32 = 100.;
/// Centre snap distance, screen points.
pub const SNAP_PX: f32 = 10.;
/// Slop around a box for picking it, screen points.
pub const HIT_SLOP: f32 = 12.;
/// Outline padding around text and stickers (pictures keep a tight box:
/// the main video fills the preview, a padded one would put its handles
/// off it), screen points.
pub const OVERLAY_PAD: f32 = 2.;
/// Rotation snaps to every multiple of this (degrees)…
const ROTATION_STEP: f32 = 45.;
/// …when within this many degrees of it.
const ROTATION_TOLERANCE: f32 = 4.;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Pt {
    pub fn new(x: f32, y: f32) -> Self {
        Pt { x, y }
    }
}

/// The box of a layer with transform `t` on a `canvas` (w, h) canvas,
/// placed as the compositor places it (`effects::metal::compose_uniforms`).
pub fn layer_box(t: &Xform, shape: Shape, canvas: (f32, f32)) -> GizmoBox {
    let (cw, ch) = canvas;
    let (width, height) = match shape {
        Shape::Fit { width, height } => {
            let (w, h) = (width.max(1.), height.max(1.));
            let fit = (cw / w).min(ch / h) * t.scale;
            (w * fit, h * fit)
        }
        Shape::Natural { width, height } => (width * cw * t.scale, height * ch * t.scale),
    };
    GizmoBox {
        cx: cw / 2. + t.x * cw,
        cy: ch / 2. + t.y * ch,
        width,
        height,
        rotation: t.rotation.to_radians(),
    }
}

impl GizmoBox {
    /// A point in the box's own space (centred, turned with it).
    pub fn to_local(self, p: Pt) -> Pt {
        let (s, c) = self.rotation.sin_cos();
        let (dx, dy) = (p.x - self.cx, p.y - self.cy);
        Pt::new(c * dx + s * dy, -s * dx + c * dy)
    }

    /// A point of the box's own space back on the canvas.
    pub fn to_canvas(self, p: Pt) -> Pt {
        let (s, c) = self.rotation.sin_cos();
        Pt::new(self.cx + c * p.x - s * p.y, self.cy + s * p.x + c * p.y)
    }

    pub fn contains(&self, p: Pt, slop: f32) -> bool {
        let l = self.to_local(p);
        l.x.abs() <= self.width / 2. + slop && l.y.abs() <= self.height / 2. + slop
    }

    /// A corner (sx, sy = ±1), `pad` outside the box, on the canvas.
    pub fn corner(&self, sx: f32, sy: f32, pad: f32) -> Pt {
        self.to_canvas(Pt::new(
            sx * (self.width / 2. + pad),
            sy * (self.height / 2. + pad),
        ))
    }
}

/// A drag around a pivot (the corner handle): how much the pointer's
/// distance from it grew, and how far it turned (radians, clockwise).
pub fn pivot_delta(pivot: Pt, from: Pt, to: Pt, min_radius: f32) -> (f32, f32) {
    let (ax, ay) = (from.x - pivot.x, from.y - pivot.y);
    let (bx, by) = (to.x - pivot.x, to.y - pivot.y);
    let r0 = ax.hypot(ay).max(min_radius);
    (bx.hypot(by) / r0, by.atan2(bx) - ay.atan2(ax))
}

/// `v` pulled onto the nearest of 0 (the canvas centre) and `anchors`
/// within `tol`; the anchor it landed on, if any.
pub fn snap_axis(v: f32, anchors: &[f32], tol: f32) -> (f32, Option<f32>) {
    let mut best = None;
    let mut best_dist = tol;
    for a in std::iter::once(0.).chain(anchors.iter().copied()) {
        let d = (v - a).abs();
        if d < best_dist {
            best = Some(a);
            best_dist = d;
        }
    }
    (best.unwrap_or(v), best)
}

/// `deg` on the nearest multiple of 45° when within 4° of it (and
/// whether it snapped: the player then shows a level cross).
pub fn snap_rotation(deg: f32) -> (f32, bool) {
    let nearest = (deg / ROTATION_STEP).round() * ROTATION_STEP;
    if (deg - nearest).abs() < ROTATION_TOLERANCE {
        (nearest, true)
    } else {
        (deg, false)
    }
}

/// What a move snapped to (transform units), for the guide lines.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Snapped {
    pub x: Option<f32>,
    pub y: Option<f32>,
    /// The rotation sits on a snap angle.
    pub level: bool,
}

/// The body drag: `start` moved by a canvas-px delta, its centre snapped
/// to the canvas centre and the other layers' centres (`xs`, `ys`, in
/// transform units) within `tol` canvas px.
pub fn moved(
    start: &Xform,
    (dx, dy): (f32, f32),
    canvas: (f32, f32),
    (xs, ys): (&[f32], &[f32]),
    tol: f32,
) -> (Xform, Snapped) {
    let (cw, ch) = (canvas.0.max(1.), canvas.1.max(1.));
    let (x, sx) = snap_axis(start.x + dx / cw, xs, tol / cw);
    let (y, sy) = snap_axis(start.y + dy / ch, ys, tol / ch);
    (
        Xform { x, y, ..*start },
        Snapped {
            x: sx,
            y: sy,
            level: false,
        },
    )
}

/// The corner handle: `start` scaled and turned about its box's centre by
/// the pointer going from `from` to `to` (canvas px).
pub fn scaled_and_turned(
    start: &Xform,
    start_box: &GizmoBox,
    from: Pt,
    to: Pt,
    min_radius: f32,
) -> (Xform, Snapped) {
    let (k, turn) = pivot_delta(Pt::new(start_box.cx, start_box.cy), from, to, min_radius);
    let (rotation, level) = snap_rotation(start.rotation + turn.to_degrees());
    (
        Xform {
            scale: (start.scale * k).clamp(MIN_SCALE, MAX_SCALE),
            rotation,
            ..*start
        },
        Snapped {
            level,
            ..Snapped::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: Xform = Xform {
        x: 0.,
        y: 0.,
        scale: 1.,
        rotation: 0.,
    };

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn boxes_follow_the_compositor() {
        // A 16:9 picture on a 9:16 canvas: fitted to the width.
        let b = layer_box(
            &ID,
            Shape::Fit {
                width: 1920.,
                height: 1080.,
            },
            (1080., 1920.),
        );
        assert!(near(b.width, 1080.) && near(b.height, 607.5));
        assert!(near(b.cx, 540.) && near(b.cy, 960.));
        // Moved a quarter canvas right, zoomed 2×, turned 90°.
        let t = Xform {
            x: 0.25,
            scale: 2.,
            rotation: 90.,
            ..ID
        };
        let b = layer_box(
            &t,
            Shape::Natural {
                width: 0.1,
                height: 0.05,
            },
            (1000., 2000.),
        );
        assert!(near(b.cx, 750.) && near(b.width, 200.) && near(b.height, 200.));
        assert!(near(b.rotation, std::f32::consts::FRAC_PI_2));
    }

    #[test]
    fn hit_tests_turn_with_the_box() {
        let b = GizmoBox {
            cx: 100.,
            cy: 100.,
            width: 100.,
            height: 10.,
            rotation: std::f32::consts::FRAC_PI_2,
        };
        // Turned upright: a point above the centre is inside, one beside isn't.
        assert!(b.contains(Pt::new(100., 140.), 0.));
        assert!(!b.contains(Pt::new(140., 100.), 0.));
        assert!(b.contains(Pt::new(108., 100.), 4.));
        // Its bottom-right corner (box space) is at the canvas's lower left.
        let c = b.corner(1., 1., 0.);
        assert!(near(c.x, 95.) && near(c.y, 150.));
    }

    #[test]
    fn moves_snap_to_centres() {
        let canvas = (1000., 1000.);
        let (t, s) = moved(&ID, (4., 300.), canvas, (&[], &[0.31]), 10.);
        assert_eq!((t.x, s.x), (0., Some(0.)));
        assert!(near(t.y, 0.31) && s.y == Some(0.31));
        let (t, s) = moved(&ID, (50., 0.), canvas, (&[], &[]), 10.);
        assert!(near(t.x, 0.05) && s.x.is_none());
    }

    #[test]
    fn the_corner_scales_and_turns_about_the_centre() {
        let b = layer_box(
            &ID,
            Shape::Natural {
                width: 0.1,
                height: 0.1,
            },
            (1000., 1000.),
        );
        // Out to twice the distance: twice the size.
        let (t, _) = scaled_and_turned(&ID, &b, Pt::new(550., 550.), Pt::new(600., 600.), 8.);
        assert!(near(t.scale, 2.) && near(t.rotation, 0.));
        // A quarter turn clockwise (screen y down), snapped onto 90°.
        let (t, s) = scaled_and_turned(&ID, &b, Pt::new(550., 500.), Pt::new(501., 550.), 8.);
        assert!(near(t.rotation, 90.) && s.level);
        // Clamped.
        let (t, _) = scaled_and_turned(&ID, &b, Pt::new(600., 500.), Pt::new(500.5, 500.), 8.);
        assert!(near(t.scale, MIN_SCALE));
    }

    #[test]
    fn rotation_snaps_near_45s() {
        assert_eq!(snap_rotation(43.), (45., true));
        assert_eq!(snap_rotation(-92.), (-90., true));
        assert_eq!(snap_rotation(30.), (30., false));
    }
}
