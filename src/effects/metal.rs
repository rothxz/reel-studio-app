//! The preview's pictures on the GPU (Metal): each decoded frame goes up as
//! a texture and through its own looks (each pass draws into the next of
//! two render targets), then they're composed on the canvas over its
//! colour or Background, the effect-lane passes run over that, and the
//! result comes back as BGRA for the player. Programs are compiled once per
//! source (so once per effect / filter) and kept; render targets are kept
//! per size.

use std::collections::HashMap;

use metal::{
    Buffer, CommandBufferRef, CompileOptions, Device, MTLBlitOption, MTLClearColor, MTLLoadAction,
    MTLOrigin, MTLPixelFormat, MTLPrimitiveType, MTLRegion, MTLResourceOptions,
    MTLSamplerAddressMode, MTLSamplerMinMagFilter, MTLSize, MTLStorageMode, MTLStoreAction,
    MTLTextureUsage, RenderPassDescriptor, RenderPipelineDescriptor, RenderPipelineState,
    SamplerDescriptor, SamplerState, Texture, TextureDescriptor, TextureRef,
};

use super::msl;
use super::stack::{Backdrop, Pass, Placement, Stack};

/// The fragment uniforms (`FxUniforms` in msl.rs).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Uniforms {
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

/// A picture in or out: tightly packed BGRA.
pub struct Image<'a> {
    pub width: u32,
    pub height: u32,
    pub bgra: &'a [u8],
}

pub struct Renderer {
    device: Device,
    queue: metal::CommandQueue,
    sampler: SamplerState,
    /// Compiled programs by source (None: it didn't compile; skipped).
    programs: HashMap<String, Option<RenderPipelineState>>,
    /// One upload texture per picture of a job.
    uploads: Vec<Option<Texture>>,
    /// Render-target pairs by (slot, width, height).
    targets: HashMap<(usize, u32, u32), [Texture; 2]>,
    readback: Option<Buffer>,
    dummy: Option<Texture>,
}

impl Renderer {
    pub fn new() -> Option<Self> {
        let device = Device::system_default()?;
        let queue = device.new_command_queue();
        let desc = SamplerDescriptor::new();
        desc.set_min_filter(MTLSamplerMinMagFilter::Linear);
        desc.set_mag_filter(MTLSamplerMinMagFilter::Linear);
        desc.set_address_mode_s(MTLSamplerAddressMode::ClampToEdge);
        desc.set_address_mode_t(MTLSamplerAddressMode::ClampToEdge);
        let sampler = device.new_sampler(&desc);
        Some(Renderer {
            device,
            queue,
            sampler,
            programs: HashMap::new(),
            uploads: vec![],
            targets: HashMap::new(),
            readback: None,
            dummy: None,
        })
    }

    /// Compiles `source` (once); None if it doesn't compile.
    pub fn program(&mut self, source: String) -> Option<RenderPipelineState> {
        if let Some(p) = self.programs.get(&source) {
            return p.clone();
        }
        let compiled = self.compile(&source);
        if let Err(e) = &compiled {
            eprintln!("effect shader didn't compile: {e}");
        }
        let program = compiled.ok();
        self.programs.insert(source, program.clone());
        program
    }

    fn compile(&self, source: &str) -> Result<RenderPipelineState, String> {
        let library = self
            .device
            .new_library_with_source(source, &CompileOptions::new())?;
        let vertex = library.get_function("fx_vertex", None)?;
        let fragment = library.get_function("fx_fragment", None)?;
        let desc = RenderPipelineDescriptor::new();
        desc.set_vertex_function(Some(&vertex));
        desc.set_fragment_function(Some(&fragment));
        desc.color_attachments()
            .object_at(0)
            .ok_or("no colour attachment")?
            .set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        self.device.new_render_pipeline_state(&desc)
    }

    fn texture(&self, width: u32, height: u32, target: bool) -> Texture {
        let desc = TextureDescriptor::new();
        desc.set_width(width as u64);
        desc.set_height(height as u64);
        desc.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        if target {
            desc.set_usage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
            desc.set_storage_mode(MTLStorageMode::Private);
        } else {
            desc.set_usage(MTLTextureUsage::ShaderRead);
            desc.set_storage_mode(MTLStorageMode::Managed);
        }
        self.device.new_texture(&desc)
    }

    /// The two render targets of a slot and size (kept; a few at most).
    fn targets(&mut self, slot: usize, width: u32, height: u32) -> [Texture; 2] {
        let key = (slot, width, height);
        if !self.targets.contains_key(&key) {
            if self.targets.len() >= 12 {
                self.targets.clear();
            }
            let pair = [
                self.texture(width, height, true),
                self.texture(width, height, true),
            ];
            self.targets.insert(key, pair);
        }
        self.targets[&key].clone()
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
        }
        u
    }

    fn pass_program(&mut self, pass: &Pass) -> Option<RenderPipelineState> {
        let source = match pass {
            Pass::Matrix(_) => msl::matrix(),
            Pass::Filter { body, .. } => msl::filter(body),
            Pass::Effect { package, .. } => msl::effect(package.kind, &package.body),
        };
        self.program(source)
    }

    /// Draws `input` through `program` into `target` (over `rect`, 0–1
    /// units; the rest cleared to `clear` if given).
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        cmd: &CommandBufferRef,
        program: &RenderPipelineState,
        input: &TextureRef,
        target: &TextureRef,
        uniforms: &Uniforms,
        rect: [f32; 4],
        clear: Option<[f32; 4]>,
    ) {
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (uniforms as *const Uniforms).cast::<u8>(),
                std::mem::size_of::<Uniforms>(),
            )
        };
        self.draw_with(cmd, program, &[input], target, bytes, rect, clear);
    }

    /// `draw` with any number of inputs (texture 0, 1, …) and uniforms.
    #[allow(clippy::too_many_arguments)]
    fn draw_with(
        &self,
        cmd: &CommandBufferRef,
        program: &RenderPipelineState,
        inputs: &[&TextureRef],
        target: &TextureRef,
        uniforms: &[u8],
        rect: [f32; 4],
        clear: Option<[f32; 4]>,
    ) {
        let desc = RenderPassDescriptor::new();
        let att = desc.color_attachments().object_at(0).expect("attachment 0");
        att.set_texture(Some(target));
        match clear {
            Some([r, g, b, a]) => {
                att.set_load_action(MTLLoadAction::Clear);
                att.set_clear_color(MTLClearColor::new(r as f64, g as f64, b as f64, a as f64));
            }
            None => att.set_load_action(MTLLoadAction::DontCare),
        }
        att.set_store_action(MTLStoreAction::Store);
        let enc = cmd.new_render_command_encoder(desc);
        enc.set_render_pipeline_state(program);
        enc.set_vertex_bytes(0, 16, rect.as_ptr().cast());
        for (i, input) in inputs.iter().enumerate() {
            enc.set_fragment_texture(i as u64, Some(input));
        }
        enc.set_fragment_sampler_state(0, Some(&self.sampler));
        enc.set_fragment_bytes(0, uniforms.len() as u64, uniforms.as_ptr().cast());
        enc.draw_primitives(MTLPrimitiveType::TriangleStrip, 0, 4);
        enc.end_encoding();
    }

    /// Upload texture `slot` holding `image` (one per picture of a job, so
    /// several can be in flight in one command buffer).
    fn upload(&mut self, slot: usize, image: &Image) -> Texture {
        let (w, h) = (image.width, image.height);
        if self.uploads.len() <= slot {
            self.uploads.resize(slot + 1, None);
        }
        let texture = match &self.uploads[slot] {
            Some(t) if t.width() == w as u64 && t.height() == h as u64 => t.clone(),
            _ => {
                let t = self.texture(w, h, false);
                self.uploads[slot] = Some(t.clone());
                t
            }
        };
        texture.replace_region(
            MTLRegion::new_2d(0, 0, w as u64, h as u64),
            0,
            image.bgra.as_ptr().cast(),
            w as u64 * 4,
        );
        texture
    }

    /// Runs `passes` over `input` (`w`×`h`) in the render targets of
    /// `slot`; the last one drawn (or `input` when there are none).
    fn run_passes(
        &mut self,
        cmd: &CommandBufferRef,
        slot: usize,
        input: Texture,
        (w, h): (u32, u32),
        passes: &[Pass],
    ) -> Texture {
        if passes.is_empty() {
            return input;
        }
        let targets = self.targets(slot, w, h);
        let full = [0., 0., 1., 1.];
        let mut current = input;
        let mut next = 0;
        for pass in passes {
            let Some(program) = self.pass_program(pass) else {
                continue;
            };
            let u = Self::uniforms(pass, w as f32 / h as f32);
            self.draw(cmd, &program, &current, &targets[next], &u, full, None);
            current = targets[next].clone();
            next = 1 - next;
        }
        current
    }

    /// Composes the pictures of `stack` (`frames`, back to front, and the
    /// Background photo if any): (width, height, BGRA). Passes whose
    /// program doesn't compile are left out.
    pub fn render(
        &mut self,
        frames: &[Image],
        backdrop: Option<Image>,
        stack: &Stack,
    ) -> (u32, u32, Vec<u8>) {
        objc::rc::autoreleasepool(|| self.render_inner(frames, backdrop, stack))
    }

    fn render_inner(
        &mut self,
        frames: &[Image],
        backdrop: Option<Image>,
        stack: &Stack,
    ) -> (u32, u32, Vec<u8>) {
        let queue = self.queue.clone();
        let cmd = queue.new_command_buffer();

        // Each picture through its own looks, at its own size.
        let mut pictures: Vec<(Texture, u32, u32)> = vec![];
        for (i, frame) in frames.iter().enumerate() {
            let up = self.upload(i, frame);
            let passes = stack.layers.get(i).map_or(&[][..], |l| &l.passes[..]);
            let size = (frame.width, frame.height);
            let done = self.run_passes(cmd, i, up, size, passes);
            pictures.push((done, frame.width, frame.height));
        }

        let Some(canvas) = &stack.canvas else {
            // One picture as it is: back at its own size.
            return match pictures.into_iter().next() {
                Some((t, w, h)) => self.read_back(cmd, &t, w, h),
                None => (0, 0, vec![]),
            };
        };

        let (cw, ch) = (canvas.width.max(2), canvas.height.max(2));
        let targets = self.targets(CANVAS_SLOT, cw, ch);
        let copy = self.program(msl::copy()).expect("the copy shader compiles");
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
                    self.draw(
                        cmd,
                        &copy,
                        t,
                        &targets[0],
                        &none,
                        cover(*w, *h),
                        Some(canvas.background),
                    );
                    let blur = self.program(msl::blur()).expect("the blur shader compiles");
                    let px = sigma * cw.min(ch) as f32;
                    for (from, to, dir) in
                        [(0, 1, [1. / cw as f32, 0.]), (1, 0, [0., 1. / ch as f32])]
                    {
                        let u: [[f32; 4]; 2] = [[dir[0], dir[1], 0., 0.], [px, 0., 0., 0.]];
                        self.draw_with(
                            cmd,
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
                    let t = self.upload(frames.len(), img);
                    self.draw(
                        cmd,
                        &copy,
                        &t,
                        &targets[0],
                        &none,
                        cover(img.width, img.height),
                        Some(canvas.background),
                    );
                    filled = true;
                }
            }
            None => {}
        }
        if !filled {
            // Just the colour (a quad of no size, the rest cleared).
            let dummy = self.dummy();
            self.draw(
                cmd,
                &copy,
                &dummy,
                &targets[0],
                &none,
                [0.; 4],
                Some(canvas.background),
            );
        }

        // Each picture placed over what's below.
        let compose = self
            .program(msl::compose())
            .expect("the compose shader compiles");
        let mut current = targets[0].clone();
        let mut next = 1;
        let after = canvas.passes_after.min(stack.layers.len());
        for (i, (layer, (t, w, h))) in stack.layers.iter().zip(&pictures).enumerate() {
            if i == after {
                self.canvas_passes(
                    cmd,
                    &canvas.passes,
                    &targets,
                    &mut current,
                    &mut next,
                    (cw, ch),
                );
            }
            let u = compose_uniforms(&layer.place, (*w, *h), (cw, ch));
            self.draw_with(
                cmd,
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
            self.canvas_passes(
                cmd,
                &canvas.passes,
                &targets,
                &mut current,
                &mut next,
                (cw, ch),
            );
        }
        self.read_back(cmd, &current, cw, ch)
    }

    /// The effect-lane clips over the canvas composed so far.
    fn canvas_passes(
        &mut self,
        cmd: &CommandBufferRef,
        passes: &[Pass],
        targets: &[Texture; 2],
        current: &mut Texture,
        next: &mut usize,
        (cw, ch): (u32, u32),
    ) {
        for pass in passes {
            let Some(program) = self.pass_program(pass) else {
                continue;
            };
            let u = Self::uniforms(pass, cw as f32 / ch as f32);
            self.draw(cmd, &program, current, &targets[*next], &u, FULL, None);
            *current = targets[*next].clone();
            *next = 1 - *next;
        }
    }

    /// A 1×1 texture to bind where a pass reads nothing.
    fn dummy(&mut self) -> Texture {
        if self.dummy.is_none() {
            self.dummy = Some(self.texture(1, 1, false));
        }
        self.dummy.clone().unwrap()
    }

    /// Ends `cmd` copying `texture` back to the CPU, and waits for it.
    fn read_back(
        &mut self,
        cmd: &CommandBufferRef,
        texture: &TextureRef,
        w: u32,
        h: u32,
    ) -> (u32, u32, Vec<u8>) {
        let len = w as u64 * h as u64 * 4;
        let readback = match &self.readback {
            Some(b) if b.length() >= len => b.clone(),
            _ => {
                let b = self
                    .device
                    .new_buffer(len, MTLResourceOptions::StorageModeShared);
                self.readback = Some(b.clone());
                b
            }
        };
        let blit = cmd.new_blit_command_encoder();
        blit.copy_from_texture_to_buffer(
            texture,
            0,
            0,
            MTLOrigin { x: 0, y: 0, z: 0 },
            MTLSize {
                width: w as u64,
                height: h as u64,
                depth: 1,
            },
            &readback,
            0,
            w as u64 * 4,
            len,
            MTLBlitOption::empty(),
        );
        blit.end_encoding();
        cmd.commit();
        cmd.wait_until_completed();
        let bytes =
            unsafe { std::slice::from_raw_parts(readback.contents() as *const u8, len as usize) };
        (w, h, bytes.to_vec())
    }
}

const FULL: [f32; 4] = [0., 0., 1., 1.];
/// The render-target slot of the canvas (pictures use 0, 1, …).
const CANVAS_SLOT: usize = usize::MAX;

fn as_bytes<T: Copy>(v: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts((v as *const T).cast::<u8>(), std::mem::size_of::<T>()) }
}

/// The compose pass's uniforms (`ComposeUniforms` in msl.rs) for a picture
/// `w`×`h` placed by `p` on a canvas `cw`×`ch`: contain-fitted (its crop
/// box), then scaled, turned clockwise about its centre and moved.
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
        [cw / 2. + p.x * cw, ch / 2. + p.y * ch, sw, sh],
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

#[cfg(test)]
mod tests {
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

    #[test]
    fn pictures_are_placed_on_the_canvas() {
        let Some(mut r) = Renderer::new() else {
            eprintln!("no Metal device: skipped");
            return;
        };
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
        let (w, h, out) = r.render(&[frame()], None, &on_canvas(vec![plain(vec![])], None));
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
        let (_, _, out) = r.render(&[frame()], None, &on_canvas(vec![placed(small)], None));
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
        let (_, _, out) = r.render(&[frame()], None, &on_canvas(vec![placed(natural)], None));
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
        let (_, _, out) = r.render(&[frame()], None, &on_canvas(vec![placed(mirror)], None));
        assert!(near(at(&out, 1, 4), green));

        // Turned a quarter clockwise: the left (blue) half ends up on top.
        let turned = Placement {
            rotation: 90.,
            ..Placement::default()
        };
        let (_, _, out) = r.render(&[frame()], None, &on_canvas(vec![placed(turned)], None));
        assert!(near(at(&out, 4, 2), blue), "{:?}", at(&out, 4, 2));
        assert!(near(at(&out, 4, 5), green), "{:?}", at(&out, 4, 5));
        assert!(near(at(&out, 1, 4), red));

        // Half opacity over red: blue + red halves.
        let faint = Placement {
            opacity: 0.5,
            ..Placement::default()
        };
        let (_, _, out) = r.render(&[frame()], None, &on_canvas(vec![placed(faint)], None));
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
        let (_, _, out) = r.render(&[frame()], None, &on_canvas(vec![placed(lighten)], None));
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
        let (_, _, out) = r.render(&[frame()], None, &on_canvas(vec![placed(masked)], None));
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
        let (_, _, out) = r.render(
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
        let (_, _, out) = r.render(
            &[frame()],
            None,
            &on_canvas(vec![placed(small)], Some(backdrop)),
        );
        let top = at(&out, 4, 0);
        assert!(top[2] < 30 && top[0] > 40 && top[1] > 40, "{top:?}");
    }

    #[test]
    fn passes_change_the_picture() {
        let Some(mut r) = Renderer::new() else {
            eprintln!("no Metal device: skipped");
            return;
        };
        // Mid grey (BGRA).
        let px = solid(4, 2, [128, 128, 128, 255]);
        let frame = || Image {
            width: 4,
            height: 2,
            bgra: &px,
        };
        let px_at = |b: &[u8], i: usize| [b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]];

        // Nothing to do: the frame comes back as it went.
        let (w, h, out) = r.render(&[frame()], None, &Stack::default());
        assert_eq!((w, h), (4, 2));
        assert_eq!(out, px);

        // A matrix that zeroes blue and maxes red.
        let mut m = [0f32; 20];
        (m[0], m[4], m[6], m[18]) = (1., 1., 1., 1.);
        let stack = Stack {
            layers: vec![plain(vec![Pass::Matrix(m)])],
            canvas: None,
        };
        let (_, _, out) = r.render(&[frame()], None, &stack);
        assert_eq!(px_at(&out, 0), [0, 128, 255, 255]);

        // A filter at half strength towards black.
        let stack = Stack {
            layers: vec![plain(vec![Pass::Filter {
                body: "return vec3(0.0);".into(),
                k: 0.5,
            }])],
            canvas: None,
        };
        let (_, _, out) = r.render(&[frame()], None, &stack);
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
        let (w, h, out) = r.render(&[frame()], None, &stack);
        assert_eq!((w, h), (4, 4));
        // Top band: red + a quarter green.
        let top = px_at(&out, 0);
        assert_eq!((top[2], top[0]), (255, 0));
        assert!(top[1].abs_diff(64) <= 1, "{top:?}");
        // Middle: grey, blue from MOD (floored: 0.75), green + 0.25.
        let mid = px_at(&out, 4 * 1 + 1);
        assert!(mid[0].abs_diff(191) <= 1, "{mid:?}");
        assert!(mid[1].abs_diff(192) <= 1, "{mid:?}");
    }

    /// Compiles every hosted effect and filter this machine can fetch: the
    /// translation and helpers must take all of them. Needs the asset host
    /// (`cargo t -- --ignored every_hosted_package`).
    #[test]
    #[ignore]
    fn every_hosted_package_compiles() {
        use crate::effects::package::{fetch_text, parse_effect, parse_filter};
        let mut r = Renderer::new().expect("Metal");
        let mut failed = vec![];
        let mut count = 0;
        for (name, key) in crate::catalog::filter_packages() {
            let body = parse_filter(&fetch_text(&key).unwrap()).unwrap();
            count += 1;
            if r.program(msl::filter(&body)).is_none() {
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
            if r.program(msl::effect(pkg.kind, &pkg.body)).is_none() {
                failed.push(format!("effect {}", row.name));
            }
        }
        eprintln!("{count} packages compiled");
        assert!(failed.is_empty(), "{failed:#?}");
    }
}
