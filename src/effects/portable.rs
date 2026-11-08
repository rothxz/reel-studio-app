//! The preview's pictures on the GPU with wgpu (Vulkan on Linux, DX12 or
//! Vulkan on Windows; Metal when tried on macOS): the `Gpu` of `draw.rs`,
//! drawing the GLSL of `glsl.rs` (naga compiles it for the device).
//!
//! Every pipeline shares one bind-group layout (see `glsl.rs`); each draw
//! gets its own small uniform buffer and bind group, which is cheap at the
//! few dozen draws a frame takes. Textures and targets are BGRA8 like the
//! frames, so nothing is swizzled on the way in or out.

use std::collections::HashMap;

use wgpu::util::DeviceExt as _;

use super::draw::{self, Gpu, Image, Program};
use super::glsl;
use super::stack::Stack;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
/// Where the fragment uniforms start in a draw's buffer (after the rect):
/// the alignment every device accepts for a uniform binding's offset.
const UNIFORMS_AT: u64 = 256;

/// A texture and its view.
#[derive(Clone)]
pub struct Tex {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

/// One render's commands.
pub struct Frame {
    encoder: wgpu::CommandEncoder,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    vertex: wgpu::ShaderModule,
    sampler: wgpu::Sampler,
    /// Compiled programs by source (None: it didn't compile; skipped).
    programs: HashMap<String, Option<wgpu::RenderPipeline>>,
    /// One upload texture per picture of a job.
    uploads: Vec<Option<Tex>>,
    /// Render-target pairs by (slot, width, height).
    targets: HashMap<(usize, u32, u32), [Tex; 2]>,
    readback: Option<wgpu::Buffer>,
    /// Bound where a pass reads fewer textures than the layout has.
    dummy: Tex,
}

impl Renderer {
    /// The device's renderer; None without a usable GPU (or a software
    /// one: lavapipe, WARP).
    pub fn new() -> Option<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .or_else(|_| {
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::None,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
        })
        .ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("effects"),
            required_features: wgpu::Features::empty(),
            // Whatever the device has: frames and canvases can be 4K.
            required_limits: adapter.limits(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .ok()?;
        // Errors are caught per program (error scopes); anything else is
        // logged, never a panic of the render thread.
        device.on_uncaptured_error(Box::new(|e| eprintln!("effects GPU error: {e}")));

        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty,
            count: None,
        };
        let uniform = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let texture = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("effects"),
            entries: &[
                entry(0, wgpu::ShaderStages::VERTEX, uniform),
                entry(1, fragment, uniform),
                entry(2, fragment, texture),
                entry(3, fragment, texture),
                entry(
                    4,
                    fragment,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("effects"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let vertex = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fx_vertex"),
            source: wgpu::ShaderSource::Glsl {
                shader: glsl::VERTEX.into(),
                stage: wgpu::naga::ShaderStage::Vertex,
                defines: &[],
            },
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("effects"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let dummy = texture_of(&device, 1, 1, false);
        Some(Renderer {
            device,
            queue,
            layout,
            pipeline_layout,
            vertex,
            sampler,
            programs: HashMap::new(),
            uploads: vec![],
            targets: HashMap::new(),
            readback: None,
            dummy,
        })
    }

    /// Composes the pictures of `stack` (see `draw::render`).
    pub fn render(
        &mut self,
        frames: &[Image],
        backdrop: Option<Image>,
        stack: &Stack,
    ) -> (u32, u32, Vec<u8>) {
        draw::render(self, frames, backdrop, stack)
    }

    /// Compiles one fragment source into a pipeline, or says why not.
    fn compile(&self, source: &str) -> Result<wgpu::RenderPipeline, String> {
        // naga's GLSL front end panics on a few malformed inputs instead of
        // returning an error: a package must never take the thread down.
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let module = self
                .device
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("fx_fragment"),
                    source: wgpu::ShaderSource::Glsl {
                        shader: source.into(),
                        stage: wgpu::naga::ShaderStage::Fragment,
                        defines: &[],
                    },
                });
            let pipeline = self
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("effects"),
                    layout: Some(&self.pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &self.vertex,
                        entry_point: Some("main"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleStrip,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some("main"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: FORMAT,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview: None,
                    cache: None,
                });
            (pipeline, pollster::block_on(self.device.pop_error_scope()))
        }));
        match built {
            Ok((pipeline, None)) => Ok(pipeline),
            // The description carries naga's message (Display doesn't).
            Ok((_, Some(wgpu::Error::Validation { description, .. }))) => Err(description),
            Ok((_, Some(error))) => Err(error.to_string()),
            Err(_) => Err("the shader compiler gave up".into()),
        }
    }

    fn pass<'a>(
        frame: &'a mut Frame,
        target: &'a Tex,
        clear: Option<[f32; 4]>,
    ) -> wgpu::RenderPass<'a> {
        let [r, g, b, a] = clear.unwrap_or([0.; 4]).map(f64::from);
        frame
            .encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Undefined outside the rect unless cleared: a
                        // transparent clear is as cheap as "don't care".
                        load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            })
    }
}

fn texture_of(device: &wgpu::Device, width: u32, height: u32, target: bool) -> Tex {
    let mut usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC;
    usage |= if target {
        wgpu::TextureUsages::RENDER_ATTACHMENT
    } else {
        wgpu::TextureUsages::COPY_DST
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    Tex { texture, view }
}

impl Gpu for Renderer {
    type Texture = Tex;
    type Pipeline = wgpu::RenderPipeline;
    type Frame = Frame;

    fn begin(&mut self) -> Frame {
        Frame {
            encoder: self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None }),
        }
    }

    fn program(&mut self, program: Program) -> Option<wgpu::RenderPipeline> {
        let source = match program {
            Program::Copy => glsl::copy(),
            Program::Matrix => glsl::matrix(),
            Program::Blur => glsl::blur(),
            Program::Compose => glsl::compose(),
            Program::Filter(body) => glsl::filter(body),
            Program::Effect(kind, body) => glsl::effect(kind, body),
            Program::Reveal(body) => glsl::reveal(body),
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

    fn upload(&mut self, slot: usize, image: &Image) -> Tex {
        let (w, h) = (image.width.max(1), image.height.max(1));
        if self.uploads.len() <= slot {
            self.uploads.resize(slot + 1, None);
        }
        let tex = match &self.uploads[slot] {
            Some(t) if t.texture.width() == w && t.texture.height() == h => t.clone(),
            _ => {
                let t = texture_of(&self.device, w, h, false);
                self.uploads[slot] = Some(t.clone());
                t
            }
        };
        if image.bgra.len() >= (w * h * 4) as usize {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &image.bgra[..(w * h * 4) as usize],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(h),
                },
                tex.texture.size(),
            );
        }
        tex
    }

    fn targets(&mut self, slot: usize, width: u32, height: u32) -> [Tex; 2] {
        let key = (slot, width, height);
        if !self.targets.contains_key(&key) {
            if self.targets.len() >= 12 {
                self.targets.clear();
            }
            let pair = [
                texture_of(&self.device, width, height, true),
                texture_of(&self.device, width, height, true),
            ];
            self.targets.insert(key, pair);
        }
        self.targets[&key].clone()
    }

    fn draw(
        &mut self,
        frame: &mut Frame,
        pipeline: &wgpu::RenderPipeline,
        inputs: &[&Tex],
        target: &Tex,
        uniforms: &[u8],
        rect: [f32; 4],
        clear: Option<[f32; 4]>,
    ) {
        // The rect at 0, the fragment uniforms at UNIFORMS_AT (padded to a
        // whole 256 so any block of the shaders fits).
        let mut bytes = vec![0u8; (UNIFORMS_AT * 2) as usize];
        bytes[..16].copy_from_slice(draw::as_bytes(&rect));
        let n = uniforms.len().min(UNIFORMS_AT as usize);
        bytes[UNIFORMS_AT as usize..UNIFORMS_AT as usize + n].copy_from_slice(&uniforms[..n]);
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: &bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let size = std::num::NonZeroU64::new(UNIFORMS_AT);
        let slice = |offset| {
            wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &buffer,
                offset,
                size,
            })
        };
        let input = |i: usize| &inputs.get(i).copied().unwrap_or(&self.dummy).view;
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: slice(0),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: slice(UNIFORMS_AT),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(input(0)),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(input(1)),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut pass = Self::pass(frame, target, clear);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..4, 0..1);
    }

    fn clear(&mut self, frame: &mut Frame, target: &Tex, colour: [f32; 4]) {
        // A pass with nothing drawn: only its clear.
        drop(Self::pass(frame, target, Some(colour)));
    }

    fn finish(&mut self, frame: Frame, tex: &Tex, w: u32, h: u32) -> (u32, u32, Vec<u8>) {
        // Rows of a buffer copy start at multiples of 256 bytes.
        let row = w * 4;
        let stride =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let len = stride as u64 * h as u64;
        let readback = match &self.readback {
            Some(b) if b.size() >= len => b.clone(),
            _ => {
                let b = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("readback"),
                    size: len,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                self.readback = Some(b.clone());
                b
            }
        };
        let mut encoder = frame.encoder;
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &tex.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let slice = readback.slice(..len);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::Wait);
        if !matches!(rx.recv(), Ok(Ok(()))) {
            return (w, h, vec![0; (row * h) as usize]);
        }
        let mut out = Vec::with_capacity((row * h) as usize);
        {
            let mapped = slice.get_mapped_range();
            for y in 0..h as usize {
                let start = y * stride as usize;
                out.extend_from_slice(&mapped[start..start + row as usize]);
            }
        }
        readback.unmap();
        (w, h, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::draw::checks;

    #[test]
    fn pictures_are_placed_on_the_canvas() {
        let Some(mut r) = Renderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        checks::pictures_are_placed_on_the_canvas(&mut r);
    }

    #[test]
    fn passes_change_the_picture() {
        let Some(mut r) = Renderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        checks::passes_change_the_picture(&mut r);
    }

    /// Needs the asset host
    /// (`cargo test --features wgpu-effects -- --ignored every_hosted_package`).
    #[test]
    #[ignore]
    fn every_hosted_package_compiles() {
        checks::every_hosted_package_compiles(&mut Renderer::new().expect("a GPU adapter"));
    }
}
