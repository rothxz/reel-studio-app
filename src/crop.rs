//! Crop math, shared by the Crop dialog and the compositor. Every box is
//! normalized 0–1 within the source frame as it's seen; `sw` / `sh` (the
//! source size) only matter for their ratio, so a small preview frame is
//! as good as the real one.

/// The Rotate slider's range, degrees either way.
pub const MAX_STRAIGHTEN: f64 = 20.;
/// Smallest crop side, as a part of the source side.
pub const MIN_SIDE: f64 = 0.08;

/// The crop dialog's ratio buttons after Custom: (label, width / height).
pub const RATIOS: &[(&str, f64)] = &[
    ("9:16", 9. / 16.),
    ("16:9", 16. / 9.),
    ("1:1", 1.),
    ("4:3", 4. / 3.),
    ("3:4", 3. / 4.),
    ("2:1", 2.),
    ("5.8\"", 1125. / 2436.),
    ("2.35:1", 2.35),
    ("1.85:1", 1.85),
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

pub const FULL: CropBox = CropBox {
    x: 0.,
    y: 0.,
    w: 1.,
    h: 1.,
};

/// A drag target on the crop frame: an edge or a corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    T,
    B,
    L,
    R,
    TL,
    TR,
    BL,
    BR,
}

impl Handle {
    pub const ALL: [Handle; 8] = [
        Handle::T,
        Handle::B,
        Handle::L,
        Handle::R,
        Handle::TL,
        Handle::TR,
        Handle::BL,
        Handle::BR,
    ];

    fn sides(self) -> (bool, bool, bool, bool) {
        use Handle::*;
        (
            matches!(self, TL | BL | L),
            matches!(self, TR | BR | R),
            matches!(self, TL | TR | T),
            matches!(self, BL | BR | B),
        )
    }
}

/// No crop and no straighten.
pub fn is_identity(b: CropBox, rotation: f64) -> bool {
    let eps = 1e-4;
    b.x.abs() < eps
        && b.y.abs() < eps
        && (b.w - 1.).abs() < eps
        && (b.h - 1.).abs() < eps
        && rotation.abs() < eps
}

/// How much the source must grow (about the crop's centre) so that, turned
/// by `angle` degrees, it still covers the whole crop box: straightening
/// never shows empty corners. 1 without a turn.
///
/// A box corner at offset d from the box centre comes from the source
/// point C + R(−angle)·d / k; each axis of that point must stay inside the
/// source, which bounds k from below per corner and axis.
pub fn cover_scale(b: CropBox, angle: f64, sw: f64, sh: f64) -> f64 {
    if angle.abs() < 1e-6 {
        return 1.;
    }
    let rad = (-angle).to_radians();
    let (sin, cos) = rad.sin_cos();
    let (cx, cy) = ((b.x + b.w / 2.) * sw, (b.y + b.h / 2.) * sh);
    let (hw, hh) = (b.w * sw / 2., b.h * sh / 2.);
    let mut k: f64 = 1.;
    for (dx, dy) in [(-hw, -hh), (hw, -hh), (-hw, hh), (hw, hh)] {
        let vx = dx * cos - dy * sin;
        let vy = dx * sin + dy * cos;
        let room_x = if vx > 0. { sw - cx } else { cx };
        let room_y = if vy > 0. { sh - cy } else { cy };
        if room_x > 0. {
            k = k.max(vx.abs() / room_x);
        }
        if room_y > 0. {
            k = k.max(vy.abs() / room_y);
        }
    }
    k
}

/// The box moved by a normalized delta, kept inside the source.
pub fn move_box(b: CropBox, dx: f64, dy: f64) -> CropBox {
    CropBox {
        x: (b.x + dx).clamp(0., 1. - b.w),
        y: (b.y + dy).clamp(0., 1. - b.h),
        ..b
    }
}

/// One handle dragged by a normalized delta. `ratio` (pixel width /
/// height) locks the shape: a corner keeps the opposite corner and follows
/// whichever axis moved further; an edge resizes about the box's centre
/// on the other axis. None: free.
pub fn resize_box(
    b: CropBox,
    handle: Handle,
    dx: f64,
    dy: f64,
    ratio: Option<f64>,
    sw: f64,
    sh: f64,
) -> CropBox {
    let (has_l, has_r, has_t, has_b) = handle.sides();
    let (left, top, right, bottom) = (b.x, b.y, b.x + b.w, b.y + b.h);
    let clamp = |v: f64, lo: f64, hi: f64| v.max(lo).min(hi);

    let Some(ratio) = ratio else {
        let (mut l, mut t, mut r, mut bt) = (left, top, right, bottom);
        if has_l {
            l = clamp(l + dx, 0., r - MIN_SIDE);
        }
        if has_r {
            r = clamp(r + dx, l + MIN_SIDE, 1.);
        }
        if has_t {
            t = clamp(t + dy, 0., bt - MIN_SIDE);
        }
        if has_b {
            bt = clamp(bt + dy, t + MIN_SIDE, 1.);
        }
        return CropBox {
            x: l,
            y: t,
            w: r - l,
            h: bt - t,
        };
    };

    // The normalized w / h a pixel ratio means for this source.
    let rn = ratio * sh / sw;
    let min_w = MIN_SIDE.max(MIN_SIDE * rn);

    if (has_l || has_r) && (has_t || has_b) {
        let ax = if has_l { right } else { left };
        let ay = if has_t { bottom } else { top };
        let px = if has_l { left } else { right } + dx;
        let py = if has_t { top } else { bottom } + dy;
        let room_w = if has_l { ax } else { 1. - ax };
        let room_h = if has_t { ay } else { 1. - ay };
        let max_w = room_w.min(room_h * rn);
        let w = clamp(
            (px - ax).abs().max((py - ay).abs() * rn),
            min_w.min(max_w),
            max_w,
        );
        let h = w / rn;
        return CropBox {
            x: if has_l { ax - w } else { ax },
            y: if has_t { ay - h } else { ay },
            w,
            h,
        };
    }

    if has_l || has_r {
        let ax = if has_l { right } else { left };
        let room_w = if has_l { ax } else { 1. - ax };
        let want = (if has_l { left } else { right } + dx) - ax;
        let max_w = room_w.min(rn);
        let w = clamp(want.abs(), min_w.min(max_w), max_w);
        let h = w / rn;
        let cy = clamp(top + b.h / 2., h / 2., 1. - h / 2.);
        return CropBox {
            x: if has_l { ax - w } else { ax },
            y: cy - h / 2.,
            w,
            h,
        };
    }

    let ay = if has_t { bottom } else { top };
    let room_h = if has_t { ay } else { 1. - ay };
    let want = (if has_t { top } else { bottom } + dy) - ay;
    let max_h = room_h.min(1. / rn);
    let min_h = MIN_SIDE.max(MIN_SIDE / rn);
    let h = clamp(want.abs(), min_h.min(max_h), max_h);
    let w = h * rn;
    let cx = clamp(b.x + b.w / 2., w / 2., 1. - w / 2.);
    CropBox {
        x: cx - w / 2.,
        y: if has_t { ay - h } else { ay },
        w,
        h,
    }
}

/// The largest box of pixel `ratio` in the source, centred as near the
/// current box's centre as the edges allow (a ratio button's pick).
pub fn fit_ratio(b: CropBox, ratio: f64, sw: f64, sh: f64) -> CropBox {
    let rn = ratio * sh / sw;
    let (w, h) = if rn >= 1. { (1., 1. / rn) } else { (rn, 1.) };
    let cx = (b.x + b.w / 2.).clamp(w / 2., 1. - w / 2.);
    let cy = (b.y + b.h / 2.).clamp(h / 2., 1. - h / 2.);
    CropBox {
        x: cx - w / 2.,
        y: cy - h / 2.,
        w,
        h,
    }
}

/// Rounded as saved (4 decimals; the angle to 0.01°), so projects don't
/// carry float noise from a drag.
pub fn rounded(b: CropBox, rotation: f64) -> (CropBox, f64) {
    let r = |n: f64| (n * 1e4).round() / 1e4;
    (
        CropBox {
            x: r(b.x),
            y: r(b.y),
            w: r(b.w),
            h: r(b.h),
        },
        (rotation * 100.).round() / 100.,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn cover_scale_grows_with_the_turn() {
        assert_eq!(cover_scale(FULL, 0., 16., 9.), 1.);
        let k = cover_scale(FULL, 10., 16., 9.);
        assert!(k > 1. && k < 1.5, "{k}");
        // A small box in the middle has room to turn without growing.
        let small = CropBox {
            x: 0.4,
            y: 0.4,
            w: 0.2,
            h: 0.2,
        };
        assert_eq!(cover_scale(small, 10., 16., 9.), 1.);
    }

    #[test]
    fn free_resize_keeps_inside_and_min_size() {
        let b = resize_box(FULL, Handle::TL, 0.3, 0.2, None, 16., 9.);
        assert!(near(b.x, 0.3) && near(b.y, 0.2) && near(b.w, 0.7) && near(b.h, 0.8));
        let b = resize_box(FULL, Handle::R, -5., 0., None, 16., 9.);
        assert!(near(b.w, MIN_SIDE));
        let m = move_box(b, 2., 2.);
        assert!(near(m.x, 1. - MIN_SIDE) && near(m.y, 0.));
    }

    #[test]
    fn locked_ratio_holds_the_pixel_shape() {
        let (sw, sh) = (1920., 1080.);
        let b = fit_ratio(FULL, 1., sw, sh);
        assert!(near(b.w * sw, b.h * sh) && near(b.h, 1.));
        assert!(near(b.x + b.w / 2., 0.5));
        let r = resize_box(b, Handle::BR, -0.1, 0., Some(1.), sw, sh);
        assert!(near(r.w * sw, r.h * sh));
        assert!(near(r.x, b.x) && near(r.y, b.y));
        let e = resize_box(b, Handle::T, 0., 0.2, Some(1.), sw, sh);
        assert!(near(e.w * sw, e.h * sh) && near(e.y + e.h, 1.));
    }

    #[test]
    fn identity_and_rounding() {
        assert!(is_identity(FULL, 0.));
        assert!(!is_identity(FULL, 1.));
        let (b, r) = rounded(
            CropBox {
                x: 0.123456,
                y: 0.,
                w: 0.5,
                h: 1.,
            },
            1.23456,
        );
        assert_eq!((b.x, r), (0.1235, 1.23));
    }
}
