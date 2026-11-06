//! The passes of a `Stack` on the GPU (Metal): the decoded frame goes up as
//! a texture, each pass draws into the next of two render targets, and the
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
use super::stack::{Pass, Stack};

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
    upload: Option<Texture>,
    targets: HashMap<(u32, u32), [Texture; 2]>,
    readback: Option<Buffer>,
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
            upload: None,
            targets: HashMap::new(),
            readback: None,
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

    /// The two render targets of a size (kept; a few sizes at most).
    fn targets(&mut self, width: u32, height: u32) -> [Texture; 2] {
        if !self.targets.contains_key(&(width, height)) {
            if self.targets.len() >= 4 {
                self.targets.clear();
            }
            let pair = [
                self.texture(width, height, true),
                self.texture(width, height, true),
            ];
            self.targets.insert((width, height), pair);
        }
        self.targets[&(width, height)].clone()
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
        enc.set_fragment_texture(0, Some(input));
        enc.set_fragment_sampler_state(0, Some(&self.sampler));
        enc.set_fragment_bytes(
            0,
            std::mem::size_of::<Uniforms>() as u64,
            (uniforms as *const Uniforms).cast(),
        );
        enc.draw_primitives(MTLPrimitiveType::TriangleStrip, 0, 4);
        enc.end_encoding();
    }

    /// Runs `stack` over `frame`: (width, height, BGRA). Passes whose
    /// program doesn't compile are left out.
    pub fn render(&mut self, frame: Image, stack: &Stack) -> (u32, u32, Vec<u8>) {
        objc::rc::autoreleasepool(|| self.render_inner(frame, stack))
    }

    fn render_inner(&mut self, frame: Image, stack: &Stack) -> (u32, u32, Vec<u8>) {
        let (fw, fh) = (frame.width, frame.height);
        let upload = match &self.upload {
            Some(t) if t.width() == fw as u64 && t.height() == fh as u64 => t.clone(),
            _ => {
                let t = self.texture(fw, fh, false);
                self.upload = Some(t.clone());
                t
            }
        };
        upload.replace_region(
            MTLRegion::new_2d(0, 0, fw as u64, fh as u64),
            0,
            frame.bgra.as_ptr().cast(),
            fw as u64 * 4,
        );

        let queue = self.queue.clone();
        let cmd = queue.new_command_buffer();
        let full = [0., 0., 1., 1.];
        let mut current: Texture = upload;
        let mut next = 0;

        let targets = self.targets(fw, fh);
        for pass in &stack.clip {
            let Some(program) = self.pass_program(pass) else {
                continue;
            };
            let target = &targets[next];
            let u = Self::uniforms(pass, fw as f32 / fh as f32);
            self.draw(cmd, &program, &current, target, &u, full, None);
            current = target.clone();
            next = 1 - next;
        }

        let (mut ow, mut oh) = (fw, fh);
        if let Some(canvas) = &stack.canvas {
            let (cw, ch) = (canvas.width.max(2), canvas.height.max(2));
            let targets = self.targets(cw, ch);
            // The clip contain-fitted on the background colour.
            let scale = (cw as f32 / fw as f32).min(ch as f32 / fh as f32);
            let (w, h) = (fw as f32 * scale / cw as f32, fh as f32 * scale / ch as f32);
            let rect = [(1. - w) / 2., (1. - h) / 2., (1. + w) / 2., (1. + h) / 2.];
            let copy = self.program(msl::copy()).expect("the copy shader compiles");
            let u = Uniforms::default();
            self.draw(
                cmd,
                &copy,
                &current,
                &targets[0],
                &u,
                rect,
                Some(canvas.background),
            );
            current = targets[0].clone();
            next = 1;
            for pass in &canvas.passes {
                let Some(program) = self.pass_program(pass) else {
                    continue;
                };
                let target = &targets[next];
                let u = Self::uniforms(pass, cw as f32 / ch as f32);
                self.draw(cmd, &program, &current, target, &u, full, None);
                current = target.clone();
                next = 1 - next;
            }
            (ow, oh) = (cw, ch);
        }

        // Back to the CPU.
        let len = ow as u64 * oh as u64 * 4;
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
            &current,
            0,
            0,
            MTLOrigin { x: 0, y: 0, z: 0 },
            MTLSize {
                width: ow as u64,
                height: oh as u64,
                depth: 1,
            },
            &readback,
            0,
            ow as u64 * 4,
            len,
            MTLBlitOption::empty(),
        );
        blit.end_encoding();
        cmd.commit();
        cmd.wait_until_completed();
        let bytes =
            unsafe { std::slice::from_raw_parts(readback.contents() as *const u8, len as usize) };
        (ow, oh, bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::effects::package::{EffectKind, EffectPackage};
    use crate::effects::stack::{CanvasPasses, EffectInputs};

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
        let (w, h, out) = r.render(frame(), &Stack::default());
        assert_eq!((w, h), (4, 2));
        assert_eq!(out, px);

        // A matrix that zeroes blue and maxes red.
        let mut m = [0f32; 20];
        (m[0], m[4], m[6], m[18]) = (1., 1., 1., 1.);
        let stack = Stack {
            clip: vec![Pass::Matrix(m)],
            canvas: None,
        };
        let (_, _, out) = r.render(frame(), &stack);
        assert_eq!(px_at(&out, 0), [0, 128, 255, 255]);

        // A filter at half strength towards black.
        let stack = Stack {
            clip: vec![Pass::Filter {
                body: "return vec3(0.0);".into(),
                k: 0.5,
            }],
            canvas: None,
        };
        let (_, _, out) = r.render(frame(), &stack);
        assert!(px_at(&out, 0)[0].abs_diff(64) <= 1, "{:?}", px_at(&out, 0));

        // Distortion reading S(), then a plus blend over the canvas: the
        // 4×2 frame fits a 4×4 canvas with red bands above and below.
        let stack = Stack {
            clip: vec![effect(
                EffectKind::Distortion,
                "vec3 c = S(uv);\nreturn vec3(c.r, c.g, MOD(-0.25, 1.0));",
                1.,
            )],
            canvas: Some(CanvasPasses {
                width: 4,
                height: 4,
                background: [1., 0., 0., 1.],
                passes: vec![effect(EffectKind::Plus, "return vec3(0.0, k, 0.0);", 0.25)],
            }),
        };
        let (w, h, out) = r.render(frame(), &stack);
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
