//! What the preview's GPU renderers share: the composition of a stack's
//! pictures, written once over a small `Gpu` trait that each backend
//! implements (Metal on macOS in `metal.rs`, wgpu elsewhere in
//! `portable.rs`).
//!
//! Each decoded frame goes up as a texture and through its own looks (each
//! pass draws into the next of two render targets), then they're composed
//! on the canvas over its colour or Background, the effect-lane passes run
//! over that, and the result comes back as BGRA for the player.

use super::package::EffectKind;
use super::stack::{Backdrop, Pass, Placement, Stack};

/// A picture in or out: tightly packed BGRA.
pub struct Image<'a> {
    pub width: u32,
    pub height: u32,
    pub bgra: &'a [u8],
}

/// A shader program, each backend writing it in its own language.
#[derive(Clone, Copy, Debug)]
pub enum Program<'a> {
    /// Copies its input (a picture fitted onto the canvas).
    Copy,
    /// A 4×5 colour matrix on straight colour (a Classic filter).
    Matrix,
    /// One direction of a Gaussian blur.
    Blur,
    /// One picture placed on the canvas over what's below.
    Compose,
    /// A packaged filter's body.
    Filter(&'a str),
    /// A video effect's body.
    Effect(EffectKind, &'a str),
    /// A transition's reveal of the incoming picture.
    Reveal(&'a str),
}

/// The operations the composition needs from a GPU.
pub trait Gpu {
    type Texture: Clone;
    type Pipeline: Clone;
    /// The commands of one `render` (a command buffer / encoder).
    type Frame;

    fn begin(&mut self) -> Self::Frame;
    /// The compiled `program` (kept); None if it doesn't compile.
    fn program(&mut self, program: Program) -> Option<Self::Pipeline>;
    /// Upload texture `slot` holding `image` (one per picture of a job, so
    /// several can be in flight in one frame).
    fn upload(&mut self, slot: usize, image: &Image) -> Self::Texture;
    /// The two render targets of a slot and size (kept; a few at most).
    fn targets(&mut self, slot: usize, width: u32, height: u32) -> [Self::Texture; 2];
    /// Draws `inputs` (texture 0, 1, …) through `pipeline` into `target`
    /// over `rect` (0–1 units, y down), the rest cleared to `clear` if
    /// given, with the fragment `uniforms`.
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        frame: &mut Self::Frame,
        pipeline: &Self::Pipeline,
        inputs: &[&Self::Texture],
        target: &Self::Texture,
        uniforms: &[u8],
        rect: [f32; 4],
        clear: Option<[f32; 4]>,
    );
    /// Fills `target` with a straight RGBA colour.
    fn clear(&mut self, frame: &mut Self::Frame, target: &Self::Texture, colour: [f32; 4]);
    /// Ends the frame copying `texture` back to the CPU and waits for it:
    /// (width, height, BGRA).
    fn finish(
        &mut self,
        frame: Self::Frame,
        texture: &Self::Texture,
        width: u32,
        height: u32,
    ) -> (u32, u32, Vec<u8>);
}

/// The fragment uniforms of the look passes (`FxUniforms` in the shaders).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Uniforms {
    matrix: [[f32; 4]; 5],
    time: f32,
    k: f32,
    ar: f32,
    pr: f32,
    dur: f32,
    p: [f32; 4],
    pad: [f32; 3],
}

const _: () = assert!(std::mem::size_of::<Uniforms>() == 128);

pub const FULL: [f32; 4] = [0., 0., 1., 1.];
/// The render-target slot of the canvas (pictures use 0, 1, …).
const CANVAS_SLOT: usize = usize::MAX;

pub fn as_bytes<T: Copy>(v: &T) -> &[u8] {
    // SAFETY: plain `#[repr(C)]` / array data of floats, read as bytes.
    unsafe { std::slice::from_raw_parts((v as *const T).cast::<u8>(), std::mem::size_of::<T>()) }
}

fn uniforms(pass: &Pass, ar: f32) -> Uniforms {
    let mut u = Uniforms {
        ar,
        ..Default::default()
    };
    match pass {
        Pass::Matrix(m) => {
            for (row, out) in u.matrix.iter_mut().take(4).enumerate() {
                out.copy_from_slice(&m[row * 5..row * 5 + 4]);
            }
            u.matrix[4] = [m[4], m[9], m[14], m[19]];
        }
        Pass::Filter { k, .. } => u.k = *k,
        Pass::Effect { inputs, .. } => {
            u.time = inputs.time;
            u.k = inputs.k;
            u.pr = inputs.progress;
            u.dur = inputs.duration;
            u.p = inputs.p;
        }
        Pass::Reveal {
            progress, duration, ..
        } => {
            u.time = progress * duration;
            u.pr = *progress;
            u.dur = *duration;
        }
    }
    u
}

fn pass_program(pass: &Pass) -> Program<'_> {
    match pass {
        Pass::Matrix(_) => Program::Matrix,
        Pass::Filter { body, .. } => Program::Filter(body),
        Pass::Effect { package, .. } => Program::Effect(package.kind, &package.body),
        Pass::Reveal { body, .. } => Program::Reveal(body),
    }
}

/// Runs `passes` over `input` (`w`×`h`) in the render targets of `slot`;
/// the last one drawn (or `input` when there are none).
fn run_passes<G: Gpu>(
    gpu: &mut G,
    frame: &mut G::Frame,
    slot: usize,
    input: G::Texture,
    (w, h): (u32, u32),
    passes: &[Pass],
) -> G::Texture {
    if passes.is_empty() {
        return input;
    }
    let targets = gpu.targets(slot, w, h);
    let mut current = input;
    let mut next = 0;
    for pass in passes {
        let Some(program) = gpu.program(pass_program(pass)) else {
            continue;
        };
        let u = uniforms(pass, w as f32 / h as f32);
        gpu.draw(
            frame,
            &program,
            &[&current],
            &targets[next],
            as_bytes(&u),
            FULL,
            None,
        );
        current = targets[next].clone();
        next = 1 - next;
    }
    current
}

/// The effect-lane clips over the canvas composed so far.
fn canvas_passes<G: Gpu>(
    gpu: &mut G,
    frame: &mut G::Frame,
    passes: &[Pass],
    targets: &[G::Texture; 2],
    current: &mut G::Texture,
    next: &mut usize,
    (cw, ch): (u32, u32),
) {
    for pass in passes {
        let Some(program) = gpu.program(pass_program(pass)) else {
            continue;
        };
        let u = uniforms(pass, cw as f32 / ch as f32);
        gpu.draw(
            frame,
            &program,
            &[current],
            &targets[*next],
            as_bytes(&u),
            FULL,
            None,
        );
        *current = targets[*next].clone();
        *next = 1 - *next;
    }
}

/// Composes the pictures of `stack` (`frames`, back to front, and the
/// Background photo if any): (width, height, BGRA). Passes whose program
/// doesn't compile are left out.
pub fn render<G: Gpu>(
    gpu: &mut G,
    frames: &[Image],
    backdrop: Option<Image>,
    stack: &Stack,
) -> (u32, u32, Vec<u8>) {
    let mut frame = gpu.begin();

    // Each picture through its own looks, at its own size.
    let mut pictures: Vec<(G::Texture, u32, u32)> = vec![];
    for (i, image) in frames.iter().enumerate() {
        let up = gpu.upload(i, image);
        let passes = stack.layers.get(i).map_or(&[][..], |l| &l.passes[..]);
        let size = (image.width, image.height);
        let done = run_passes(gpu, &mut frame, i, up, size, passes);
        pictures.push((done, image.width, image.height));
    }

    let Some(canvas) = &stack.canvas else {
        // One picture as it is: back at its own size.
        return match pictures.into_iter().next() {
            Some((t, w, h)) => gpu.finish(frame, &t, w, h),
            None => (0, 0, vec![]),
        };
    };

    let (cw, ch) = (canvas.width.max(2), canvas.height.max(2));
    let targets = gpu.targets(CANVAS_SLOT, cw, ch);
    let copy = gpu
        .program(Program::Copy)
        .expect("the copy shader compiles");
    let none = Uniforms::default();
    // A picture covering the canvas (the Background's photo or blur).
    let cover = |w: u32, h: u32| {
        let scale = (cw as f32 / w as f32).max(ch as f32 / h as f32);
        let (rw, rh) = (w as f32 * scale / cw as f32, h as f32 * scale / ch as f32);
        [
            (1. - rw) / 2.,
            (1. - rh) / 2.,
            (1. + rw) / 2.,
            (1. + rh) / 2.,
        ]
    };
    let mut filled = false;
    match &canvas.backdrop {
        Some(Backdrop::Blur { layer, sigma }) => {
            if let Some((t, w, h)) = pictures.get(*layer) {
                gpu.draw(
                    &mut frame,
                    &copy,
                    &[t],
                    &targets[0],
                    as_bytes(&none),
                    cover(*w, *h),
                    Some(canvas.background),
                );
                let blur = gpu
                    .program(Program::Blur)
                    .expect("the blur shader compiles");
                let px = sigma * cw.min(ch) as f32;
                for (from, to, dir) in [(0, 1, [1. / cw as f32, 0.]), (1, 0, [0., 1. / ch as f32])]
                {
                    let u: [[f32; 4]; 2] = [[dir[0], dir[1], 0., 0.], [px, 0., 0., 0.]];
                    gpu.draw(
                        &mut frame,
                        &blur,
                        &[&targets[from]],
                        &targets[to],
                        as_bytes(&u),
                        FULL,
                        None,
                    );
                }
                filled = true;
            }
        }
        Some(Backdrop::Image(_)) => {
            if let Some(img) = &backdrop {
                let t = gpu.upload(frames.len(), img);
                gpu.draw(
                    &mut frame,
                    &copy,
                    &[&t],
                    &targets[0],
                    as_bytes(&none),
                    cover(img.width, img.height),
                    Some(canvas.background),
                );
                filled = true;
            }
        }
        None => {}
    }
    if !filled {
        gpu.clear(&mut frame, &targets[0], canvas.background);
    }

    // Each picture placed over what's below.
    let compose = gpu
        .program(Program::Compose)
        .expect("the compose shader compiles");
    let mut current = targets[0].clone();
    let mut next = 1;
    let after = canvas.passes_after.min(stack.layers.len());
    for (i, (layer, (t, w, h))) in stack.layers.iter().zip(&pictures).enumerate() {
        if i == after {
            canvas_passes(
                gpu,
                &mut frame,
                &canvas.passes,
                &targets,
                &mut current,
                &mut next,
                (cw, ch),
            );
        }
        let u = compose_uniforms(&layer.place, (*w, *h), (cw, ch));
        gpu.draw(
            &mut frame,
            &compose,
            &[&current, t],
            &targets[next],
            as_bytes(&u),
            FULL,
            None,
        );
        current = targets[next].clone();
        next = 1 - next;
    }

    if after >= stack.layers.len() {
        canvas_passes(
            gpu,
            &mut frame,
            &canvas.passes,
            &targets,
            &mut current,
            &mut next,
            (cw, ch),
        );
    }
    gpu.finish(frame, &current, cw, ch)
}

/// The compose pass's uniforms (`ComposeUniforms` in the shaders) for a
/// picture `w`×`h` placed by `p` on a canvas `cw`×`ch`: contain-fitted (its
/// crop box), then scaled, turned clockwise about its centre and moved.
fn compose_uniforms(p: &Placement, (w, h): (u32, u32), (cw, ch): (u32, u32)) -> [[f32; 4]; 7] {
    let (cw, ch) = (cw as f32, ch as f32);
    let fw = (w as f32 * p.crop[2]).max(1.);
    let fh = (h as f32 * p.crop[3]).max(1.);
    let size = (cw / fw).min(ch / fh) * p.scale;
    // Shown size: contain-fitted, or the picture's own size (text, stickers).
    let (sw, sh) = match p.natural {
        Some([nw, nh]) => (nw * cw * p.scale, nh * ch * p.scale),
        None => (fw * size, fh * size),
    };
    // A transition's motion: moved by its own drawn size, stretched.
    let [tx, ty, sx, sy] = p.motion;
    let (cx, cy) = (cw / 2. + p.x * cw + tx * sw, ch / 2. + p.y * ch + ty * sh);
    let (sw, sh) = (sw * sx.max(1e-4), sh * sy.max(1e-4));
    let rad = p.rotation.to_radians();
    let mut flags = 0u32;
    if p.flip_x {
        flags |= 1;
    }
    if p.flip_y {
        flags |= 2;
    }
    let (mask0, mask1) = match &p.mask {
        Some(m) => {
            flags |= 4;
            if m.invert {
                flags |= 8;
            }
            let r = m.rotation.to_radians();
            (
                [m.center[0], m.center[1], m.size[0], m.size[1]],
                [r.cos(), r.sin(), m.feather, m.shape as f32],
            )
        }
        None => ([0.; 4], [1., 0., 0., 0.]),
    };
    [
        [cw, ch, fw / fh, p.blend as f32],
        [cx, cy, sw, sh],
        [rad.cos(), rad.sin(), p.opacity, flags as f32],
        p.crop,
        mask0,
        mask1,
        straighten(p, (w, h)),
    ]
}

/// The crop's straighten for the shader: cos, sin of the turn undone, the
/// cover scale, the source's aspect.
fn straighten(p: &Placement, (w, h): (u32, u32)) -> [f32; 4] {
    let (sw, sh) = (w.max(1) as f64, h.max(1) as f64);
    let [x, y, bw, bh] = p.crop.map(f64::from);
    let k = crate::crop::cover_scale(
        crate::crop::CropBox { x, y, w: bw, h: bh },
        p.crop_turn as f64,
        sw,
        sh,
    );
    let rad = (-p.crop_turn).to_radians();
    [rad.cos(), rad.sin(), k as f32, (sw / sh) as f32]
}

/// The renderer checks every backend runs (`check_*` on a live GPU).
#[cfg(test)]
pub mod checks {
    use std::sync::Arc;

    use super::*;
    use crate::effects::package::{EffectKind, EffectPackage};
    use crate::effects::stack::{CanvasPasses, EffectInputs, LayerPlan, MaskPlan};

    fn solid(w: u32, h: u32, bgra: [u8; 4]) -> Vec<u8> {
        bgra.repeat((w * h) as usize)
    }

    fn effect(kind: EffectKind, body: &str, k: f32) -> Pass {
        Pass::Effect {
            package: Arc::new(EffectPackage {
                kind,
                body: body.into(),
            }),
            inputs: EffectInputs {
                time: 0.,
                k,
                progress: 0.,
                duration: 3.,
                p: [0.5; 4],
            },
        }
    }

    fn plain(passes: Vec<Pass>) -> LayerPlan {
        LayerPlan {
            passes,
            place: Placement::default(),
        }
    }

    fn on_canvas(layers: Vec<LayerPlan>, backdrop: Option<Backdrop>) -> Stack {
        Stack {
            layers,
            canvas: Some(CanvasPasses {
                width: 8,
                height: 8,
                // Red (straight RGBA).
                background: [1., 0., 0., 1.],
                backdrop,
                passes: vec![],
                passes_after: 0,
            }),
        }
    }

    fn placed(place: Placement) -> LayerPlan {
        LayerPlan {
            passes: vec![],
            place,
        }
    }

    /// BGRA at (x, y) of an 8-wide picture.
    fn at(b: &[u8], x: usize, y: usize) -> [u8; 4] {
        let i = (y * 8 + x) * 4;
        [b[i], b[i + 1], b[i + 2], b[i + 3]]
    }

    fn near(a: [u8; 4], b: [u8; 4]) -> bool {
        a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 2)
    }

    pub fn pictures_are_placed_on_the_canvas<G: Gpu>(r: &mut G) {
        // 4×2: the left half blue, the right half green (BGRA).
        let mut px = vec![];
        for _ in 0..2 {
            px.extend([
                255, 0, 0, 255, 255, 0, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255,
            ]);
        }
        let frame = || Image {
            width: 4,
            height: 2,
            bgra: &px,
        };
        let (blue, green, red) = ([255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]);

        // Plain: contain-fitted, 8×4 across the middle on the red canvas.
        let (w, h, out) = render(r, &[frame()], None, &on_canvas(vec![plain(vec![])], None));
        assert_eq!((w, h), (8, 8));
        assert!(near(at(&out, 1, 4), blue), "{:?}", at(&out, 1, 4));
        assert!(near(at(&out, 6, 4), green));
        assert!(near(at(&out, 4, 0), red));

        // Half size, moved a quarter canvas right: spans x 4..8, y 3..5.
        let small = Placement {
            scale: 0.5,
            x: 0.25,
            ..Placement::default()
        };
        let (_, _, out) = render(r, &[frame()], None, &on_canvas(vec![placed(small)], None));
        assert!(near(at(&out, 2, 4), red));
        assert!(near(at(&out, 4, 4), blue), "{:?}", at(&out, 4, 4));
        assert!(near(at(&out, 7, 4), green));
        assert!(near(at(&out, 5, 1), red));

        // Its own size (text, stickers): a quarter of the canvas wide and
        // high whatever its pixels, centred: x 3..5, y 3..5.
        let natural = Placement {
            natural: Some([0.25, 0.25]),
            ..Placement::default()
        };
        let (_, _, out) = render(r, &[frame()], None, &on_canvas(vec![placed(natural)], None));
        assert!(near(at(&out, 2, 4), red), "{:?}", at(&out, 2, 4));
        assert!(near(at(&out, 3, 4), blue), "{:?}", at(&out, 3, 4));
        assert!(near(at(&out, 4, 4), green));
        assert!(near(at(&out, 5, 4), red), "{:?}", at(&out, 5, 4));
        assert!(near(at(&out, 4, 2), red));

        // Mirrored: green on the left.
        let mirror = Placement {
            flip_x: true,
            ..Placement::default()
        };
        let (_, _, out) = render(r, &[frame()], None, &on_canvas(vec![placed(mirror)], None));
        assert!(near(at(&out, 1, 4), green));

        // Turned a quarter clockwise: the left (blue) half ends up on top.
        let turned = Placement {
            rotation: 90.,
            ..Placement::default()
        };
        let (_, _, out) = render(r, &[frame()], None, &on_canvas(vec![placed(turned)], None));
        assert!(near(at(&out, 4, 2), blue), "{:?}", at(&out, 4, 2));
        assert!(near(at(&out, 4, 5), green), "{:?}", at(&out, 4, 5));
        assert!(near(at(&out, 1, 4), red));

        // Half opacity over red: blue + red halves.
        let faint = Placement {
            opacity: 0.5,
            ..Placement::default()
        };
        let (_, _, out) = render(r, &[frame()], None, &on_canvas(vec![placed(faint)], None));
        assert!(
            near(at(&out, 1, 4), [128, 0, 128, 255]),
            "{:?}",
            at(&out, 1, 4)
        );

        // Splice "Lighten" (2) of blue over red: magenta.
        let lighten = Placement {
            blend: 2,
            ..Placement::default()
        };
        let (_, _, out) = render(r, &[frame()], None, &on_canvas(vec![placed(lighten)], None));
        assert!(near(at(&out, 1, 4), [255, 0, 255, 255]));

        // An inverted small circle in the middle: the centre shows red, the
        // corners of the picture still show.
        let masked = Placement {
            mask: Some(MaskPlan {
                shape: 2,
                center: [0.5, 0.5],
                size: [0.25, 0.5],
                rotation: 0.,
                feather: 0.,
                invert: true,
            }),
            ..Placement::default()
        };
        let (_, _, out) = render(r, &[frame()], None, &on_canvas(vec![placed(masked)], None));
        assert!(
            at(&out, 4, 4)[2] > 200 && at(&out, 4, 4)[1] < 60,
            "{:?}",
            at(&out, 4, 4)
        );
        assert!(near(at(&out, 0, 2), blue));

        // Two layers: the second (green-left mirror, half size) over the first.
        let over = Placement {
            scale: 0.5,
            flip_x: true,
            ..Placement::default()
        };
        let (_, _, out) = render(
            r,
            &[frame(), frame()],
            None,
            &on_canvas(vec![plain(vec![]), placed(over)], None),
        );
        assert!(near(at(&out, 0, 4), blue));
        assert!(near(at(&out, 3, 4), green));

        // A blurred Background: the picture, half size, over its own blur;
        // the canvas row at the top is a mix of blue and green, no red.
        let backdrop = Backdrop::Blur {
            layer: 0,
            sigma: 0.2,
        };
        let small = Placement {
            scale: 0.5,
            ..Placement::default()
        };
        let (_, _, out) = render(
            r,
            &[frame()],
            None,
            &on_canvas(vec![placed(small.clone())], Some(backdrop)),
        );
        let top = at(&out, 4, 0);
        assert!(top[2] < 30 && top[0] > 40 && top[1] > 40, "{top:?}");

        // A Background photo (all green) covering the canvas under the
        // half-size picture.
        let photo = solid(2, 2, [0, 255, 0, 255]);
        let (_, _, out) = render(
            r,
            &[frame()],
            Some(Image {
                width: 2,
                height: 2,
                bgra: &photo,
            }),
            &on_canvas(vec![placed(small)], Some(Backdrop::Image("photo".into()))),
        );
        assert!(near(at(&out, 4, 0), green), "{:?}", at(&out, 4, 0));
        assert!(near(at(&out, 3, 4), blue), "{:?}", at(&out, 3, 4));
    }

    pub fn passes_change_the_picture<G: Gpu>(r: &mut G) {
        // Mid grey (BGRA).
        let px = solid(4, 2, [128, 128, 128, 255]);
        let frame = || Image {
            width: 4,
            height: 2,
            bgra: &px,
        };
        let px_at = |b: &[u8], i: usize| [b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]];

        // Nothing to do: the frame comes back as it went.
        let (w, h, out) = render(r, &[frame()], None, &Stack::default());
        assert_eq!((w, h), (4, 2));
        assert_eq!(out, px);

        // A matrix that zeroes blue and maxes red.
        let mut m = [0f32; 20];
        (m[0], m[4], m[6], m[18]) = (1., 1., 1., 1.);
        let stack = Stack {
            layers: vec![plain(vec![Pass::Matrix(m)])],
            canvas: None,
        };
        let (_, _, out) = render(r, &[frame()], None, &stack);
        assert_eq!(px_at(&out, 0), [0, 128, 255, 255]);

        // A filter at half strength towards black, using a helper.
        let stack = Stack {
            layers: vec![plain(vec![Pass::Filter {
                body: "return fl_sat(vec3(0.0), 1.0);".into(),
                k: 0.5,
            }])],
            canvas: None,
        };
        let (_, _, out) = render(r, &[frame()], None, &stack);
        assert!(px_at(&out, 0)[0].abs_diff(64) <= 1, "{:?}", px_at(&out, 0));

        // Distortion reading S(), then a plus blend over the canvas: the
        // 4×2 frame fits a 4×4 canvas with red bands above and below.
        let stack = Stack {
            layers: vec![plain(vec![effect(
                EffectKind::Distortion,
                "vec3 c = S(uv);\nreturn vec3(c.r, c.g, MOD(-0.25, 1.0));",
                1.,
            )])],
            canvas: Some(CanvasPasses {
                width: 4,
                height: 4,
                background: [1., 0., 0., 1.],
                backdrop: None,
                passes: vec![effect(EffectKind::Plus, "return vec3(0.0, k, 0.0);", 0.25)],
                passes_after: 1,
            }),
        };
        let (w, h, out) = render(r, &[frame()], None, &stack);
        assert_eq!((w, h), (4, 4));
        // Top band: red + a quarter green.
        let top = px_at(&out, 0);
        assert_eq!((top[2], top[0]), (255, 0));
        assert!(top[1].abs_diff(64) <= 1, "{top:?}");
        // Middle: grey, blue from MOD (floored: 0.75), green + 0.25.
        let mid = px_at(&out, 4 + 1);
        assert!(mid[0].abs_diff(191) <= 1, "{mid:?}");
        assert!(mid[1].abs_diff(192) <= 1, "{mid:?}");

        // A body that doesn't compile is left out (the picture shows).
        let stack = Stack {
            layers: vec![plain(vec![effect(
                EffectKind::Distortion,
                "return nonsense(;",
                1.,
            )])],
            canvas: None,
        };
        let (_, _, out) = render(r, &[frame()], None, &stack);
        assert_eq!(out, px);
    }

    /// Compiles every hosted effect and filter this machine can fetch: the
    /// translation and helpers must take all of them.
    pub fn every_hosted_package_compiles<G: Gpu>(r: &mut G) {
        use crate::effects::package::{fetch_text, parse_effect, parse_filter};
        let mut failed = vec![];
        let mut count = 0;
        for (name, key) in crate::catalog::filter_packages() {
            let body = parse_filter(&fetch_text(&key).unwrap()).unwrap();
            count += 1;
            if r.program(Program::Filter(&body)).is_none() {
                failed.push(format!("filter {name}"));
            }
        }
        let rows = crate::catalog::fetch_all_effects().expect("effects table");
        for row in rows {
            let Ok(text) = fetch_text(&row.package_key) else {
                failed.push(format!("effect {} (fetch)", row.name));
                continue;
            };
            let pkg = parse_effect(&text).unwrap();
            count += 1;
            if r.program(Program::Effect(pkg.kind, &pkg.body)).is_none() {
                failed.push(format!("effect {}", row.name));
            }
        }
        // The transitions' reveals.
        for p in crate::transitions::PRESETS.iter() {
            let crate::transitions::Motion::Reveal(shader) = &p.motion else {
                continue;
            };
            let key = crate::transitions::reveal_package_key(shader).expect("a package key");
            let Ok(text) = fetch_text(key) else {
                failed.push(format!("transition {} (fetch)", p.id));
                continue;
            };
            let body = crate::effects::package::parse_reveal(&text).unwrap();
            count += 1;
            if r.program(Program::Reveal(&body)).is_none() {
                failed.push(format!("transition {}", p.id));
            }
        }
        eprintln!("{count} packages compiled");
        assert!(failed.is_empty(), "{failed:#?}");
    }
}
