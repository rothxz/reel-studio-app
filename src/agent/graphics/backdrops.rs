//! The backdrops besides the drifting glow (`Painter::backdrop`): a tech
//! grid, a dot field and sweeping beams. Soft parts are drawn small and
//! scaled up (as the glow is); fine lines and dots are drawn full size.

use tiny_skia::{
    FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point, RadialGradient,
    Rect, SpreadMode, Stroke, Transform,
};

use super::{Painter, Rgb, paint, rgba, upscale};

impl Painter<'_> {
    /// A small canvas with the palette's gradient and `pools` of slowly
    /// moving light, scaled up into `pix`.
    fn soft_base(&self, pix: &mut Pixmap, t: f32, pools: &[(Rgb, f32)], vignette: u8) {
        let p = self.scene.palette;
        let (bw, bh) = (
            (self.scene.width / 4).max(2),
            (self.scene.height / 4).max(2),
        );
        let Some(mut small) = Pixmap::new(bw, bh) else {
            return;
        };
        let (fw, fh) = (bw as f32, bh as f32);
        let full = Rect::from_xywh(0., 0., fw, fh).unwrap();
        let fill = |small: &mut Pixmap, shader| {
            let paint = Paint {
                shader,
                ..Paint::default()
            };
            small.fill_rect(full, &paint, Transform::identity(), None);
        };
        if let Some(sh) = LinearGradient::new(
            Point::from_xy(0., 0.),
            Point::from_xy(0., fh),
            vec![
                GradientStop::new(0., rgba(p.bg[0], 1.)),
                GradientStop::new(1., rgba(p.bg[1], 1.)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            fill(&mut small, sh);
        }
        for (i, (color, strength)) in pools.iter().enumerate() {
            let k = i as f32;
            let ph = self.seed * 0.61 + k * 2.4;
            let cx = fw * (0.5 + 0.34 * (t * 0.11 * (k + 1.) + ph).sin());
            let cy = fh * (0.45 + 0.3 * (t * 0.08 * (k + 1.4) + ph * 1.7).cos());
            let r = fw.max(fh) * (0.32 + 0.05 * k);
            if let Some(sh) = RadialGradient::new(
                Point::from_xy(cx, cy),
                Point::from_xy(cx, cy),
                r,
                vec![
                    GradientStop::new(0., rgba(*color, *strength)),
                    GradientStop::new(1., rgba(*color, 0.)),
                ],
                SpreadMode::Pad,
                Transform::identity(),
            ) {
                fill(&mut small, sh);
            }
        }
        if vignette > 0
            && let Some(sh) = RadialGradient::new(
                Point::from_xy(fw / 2., fh / 2.),
                Point::from_xy(fw / 2., fh / 2.),
                fw.max(fh) * 0.72,
                vec![
                    GradientStop::new(0.5, tiny_skia::Color::from_rgba8(0, 0, 0, 0)),
                    GradientStop::new(1., tiny_skia::Color::from_rgba8(0, 0, 0, vignette)),
                ],
                SpreadMode::Pad,
                Transform::identity(),
            )
        {
            fill(&mut small, sh);
        }
        upscale(&small, pix);
    }

    pub(super) fn ink(&self) -> Rgb {
        if self.scene.palette.light {
            (0, 0, 0)
        } else {
            (255, 255, 255)
        }
    }

    /// A dark field, a faint grid drifting down and pools of light.
    pub(super) fn grid_backdrop(&self, pix: &mut Pixmap, t: f32) {
        let p = self.scene.palette;
        let glow = if p.light { 0.1 } else { 0.13 };
        self.soft_base(pix, t, &[(p.accent, glow), (p.accent2, glow * 0.8)], 120);
        let (w, h) = (self.w, self.h);
        let step = 34. * self.u;
        let drift = (t * 5. * self.u) % step;
        let mut pb = PathBuilder::new();
        let mut x = (w / 2.) % step;
        while x < w {
            pb.move_to(x, 0.);
            pb.line_to(x, h);
            x += step;
        }
        let mut y = drift - step;
        while y < h {
            pb.move_to(0., y);
            pb.line_to(w, y);
            y += step;
        }
        if let Some(path) = pb.finish() {
            let stroke = Stroke {
                width: (self.u * 0.45).max(1.),
                ..Stroke::default()
            };
            pix.stroke_path(
                &path,
                &paint(rgba(self.ink(), 0.045)),
                &stroke,
                Transform::identity(),
                None,
            );
        }
        // A few crossings glint in turn.
        let mut pb = PathBuilder::new();
        let cols = (w / step) as i32 + 1;
        let rows = (h / step) as i32 + 2;
        for k in 0..14 {
            let n = (self.seed as i32 * 7 + k * 131) % (cols * rows).max(1);
            let (cx, cy) = (
                (w / 2.) % step + (n % cols) as f32 * step,
                drift - step + (n / cols) as f32 * step,
            );
            let on = ((t * 0.7 + k as f32 * 0.37).sin() * 0.5 + 0.5).powi(4);
            if on > 0.05 {
                pb.push_circle(cx, cy, self.u * 1.3 * on);
            }
        }
        if let Some(path) = pb.finish() {
            pix.fill_path(
                &path,
                &paint(rgba(p.accent, 0.5)),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    /// A field of dots; a slow wave brightens them as it passes.
    pub(super) fn dots_backdrop(&self, pix: &mut Pixmap, t: f32) {
        let p = self.scene.palette;
        let glow = if p.light { 0.12 } else { 0.16 };
        self.soft_base(pix, t, &[(p.accent2, glow), (p.accent, glow * 0.7)], 100);
        let step = 22. * self.u;
        let (w, h) = (self.w, self.h);
        // Dots in three brightness bands (one path each: cheap).
        let mut bands = [PathBuilder::new(), PathBuilder::new(), PathBuilder::new()];
        let angle = self.seed * 0.01;
        let (dx, dy) = (angle.cos(), angle.sin());
        let mut y = step / 2.;
        while y < h {
            let mut x = step / 2.;
            while x < w {
                let wave = ((x * dx + y * dy) / (180. * self.u) - t * 0.9).sin();
                let band = if wave > 0.6 {
                    2
                } else if wave > -0.2 {
                    1
                } else {
                    0
                };
                bands[band].push_circle(x, y, self.u * (0.8 + band as f32 * 0.35));
                x += step;
            }
            y += step;
        }
        for (k, pb) in bands.into_iter().enumerate() {
            if let Some(path) = pb.finish() {
                let a = [0.07, 0.12, 0.22][k];
                let c = if k == 2 { p.accent } else { self.ink() };
                pix.fill_path(
                    &path,
                    &paint(rgba(c, a)),
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
    }

    /// Soft beams of light sweeping slowly across.
    pub(super) fn beams_backdrop(&self, pix: &mut Pixmap, t: f32) {
        let p = self.scene.palette;
        let glow = if p.light { 0.1 } else { 0.12 };
        self.soft_base(pix, t, &[(p.accent, glow)], 110);
        let (w, h) = (self.w, self.h);
        let full = Rect::from_xywh(0., 0., w, h).unwrap();
        let tilt = 0.35 + (self.seed % 7.) * 0.06;
        for k in 0..3 {
            let k = k as f32;
            // Where the beam crosses, 0–1 across the frame, drifting.
            let c = (0.2 + k * 0.3 + t * 0.03 * (1. + k * 0.4) + self.seed * 0.013) % 1.3 - 0.15;
            let width = 0.05 + 0.03 * k;
            let color = if k as i32 == 1 { p.accent2 } else { p.accent };
            let a = if p.light { 0.08 } else { 0.07 };
            let stops = vec![
                GradientStop::new((c - width).clamp(0., 1.), rgba(color, 0.)),
                GradientStop::new(c.clamp(0., 1.), rgba(color, a)),
                GradientStop::new((c + width).clamp(0., 1.), rgba(color, 0.)),
            ];
            if let Some(sh) = LinearGradient::new(
                Point::from_xy(0., h * tilt),
                Point::from_xy(w, h * (1. - tilt)),
                stops,
                SpreadMode::Pad,
                Transform::identity(),
            ) {
                let paint = Paint {
                    shader: sh,
                    anti_alias: false,
                    ..Paint::default()
                };
                pix.fill_rect(full, &paint, Transform::identity(), None);
            }
        }
    }
}
