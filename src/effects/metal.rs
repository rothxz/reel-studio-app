//! The preview's pictures on the GPU with Metal (macOS): the `Gpu` of
//! `draw.rs`. Programs are compiled once per source (so once per effect /
//! filter) and kept; render targets are kept per size.

use std::collections::HashMap;

use metal::{
    Buffer, CommandBuffer, CompileOptions, Device, MTLBlitOption, MTLClearColor, MTLLoadAction,
    MTLOrigin, MTLPixelFormat, MTLPrimitiveType, MTLRegion, MTLResourceOptions,
    MTLSamplerAddressMode, MTLSamplerMinMagFilter, MTLSize, MTLStorageMode, MTLStoreAction,
    MTLTextureUsage, RenderPassDescriptor, RenderPassDescriptorRef, RenderPipelineDescriptor,
    RenderPipelineState, SamplerDescriptor, SamplerState, Texture, TextureDescriptor,
};

use super::draw::{self, Gpu, Image, Program};
use super::msl;
use super::stack::Stack;

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
        })
    }

    /// Composes the pictures of `stack` (see `draw::render`).
    pub fn render(
        &mut self,
        frames: &[Image],
        backdrop: Option<Image>,
        stack: &Stack,
    ) -> (u32, u32, Vec<u8>) {
        objc::rc::autoreleasepool(|| draw::render(self, frames, backdrop, stack))
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

    /// A render pass into `target`: cleared to `clear`, or left undefined.
    fn pass(target: &Texture, clear: Option<[f32; 4]>) -> &'static RenderPassDescriptorRef {
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
        desc
    }
}

impl Gpu for Renderer {
    type Texture = Texture;
    type Pipeline = RenderPipelineState;
    type Frame = CommandBuffer;

    fn begin(&mut self) -> CommandBuffer {
        self.queue.new_command_buffer().to_owned()
    }

    fn program(&mut self, program: Program) -> Option<RenderPipelineState> {
        let source = match program {
            Program::Copy => msl::copy(),
            Program::Matrix => msl::matrix(),
            Program::Blur => msl::blur(),
            Program::Compose => msl::compose(),
            Program::Filter(body) => msl::filter(body),
            Program::Effect(kind, body) => msl::effect(kind, body),
        };
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

    fn draw(
        &mut self,
        cmd: &mut CommandBuffer,
        program: &RenderPipelineState,
        inputs: &[&Texture],
        target: &Texture,
        uniforms: &[u8],
        rect: [f32; 4],
        clear: Option<[f32; 4]>,
    ) {
        let enc = cmd.new_render_command_encoder(Self::pass(target, clear));
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

    fn clear(&mut self, cmd: &mut CommandBuffer, target: &Texture, colour: [f32; 4]) {
        // A pass with nothing drawn: only its clear.
        cmd.new_render_command_encoder(Self::pass(target, Some(colour)))
            .end_encoding();
    }

    fn finish(
        &mut self,
        cmd: CommandBuffer,
        texture: &Texture,
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
        // SAFETY: a shared buffer of at least `len` bytes, the GPU done with it.
        let bytes =
            unsafe { std::slice::from_raw_parts(readback.contents() as *const u8, len as usize) };
        (w, h, bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::draw::checks;

    #[test]
    fn pictures_are_placed_on_the_canvas() {
        let Some(mut r) = Renderer::new() else {
            eprintln!("no Metal device: skipped");
            return;
        };
        checks::pictures_are_placed_on_the_canvas(&mut r);
    }

    #[test]
    fn passes_change_the_picture() {
        let Some(mut r) = Renderer::new() else {
            eprintln!("no Metal device: skipped");
            return;
        };
        checks::passes_change_the_picture(&mut r);
    }

    /// Needs the asset host (`cargo t -- --ignored every_hosted_package`).
    #[test]
    #[ignore]
    fn every_hosted_package_compiles() {
        checks::every_hosted_package_compiles(&mut Renderer::new().expect("Metal"));
    }
}
