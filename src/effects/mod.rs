//! Video effects and filters in the preview.
//!
//! - `package`: the looks' shaders as downloaded data (+ the store).
//! - `stack`: what to draw at a time, as passes (the document → passes).
//! - `draw`: the composition of a stack on a GPU, over a small trait.
//! - `msl` / `metal`: Metal source and renderer (macOS).
//! - `glsl` / `portable`: GLSL source and the wgpu renderer (Linux,
//!   Windows; on macOS with the `wgpu-effects` feature, for its tests).
//!
//! The editor hands the latest decoded frames (one per visible picture) and
//! their stack to one render thread (latest wins, like the decoders) and
//! shows what comes back. Without a GPU the top picture passes through
//! unchanged.

pub mod draw;
#[cfg(any(not(target_os = "macos"), feature = "wgpu-effects"))]
pub mod glsl;
#[cfg(target_os = "macos")]
pub mod metal;
#[cfg(target_os = "macos")]
pub mod msl;
pub mod package;
#[cfg(any(not(target_os = "macos"), feature = "wgpu-effects"))]
pub mod portable;
pub mod stack;

use std::sync::{Arc, Condvar, Mutex};

use crate::decoder::DecodedFrame;
use stack::Stack;

pub fn image(f: &DecodedFrame) -> draw::Image<'_> {
    draw::Image {
        width: f.width,
        height: f.height,
        bgra: &f.bgra,
    }
}

/// The GPU renderer of this platform: Metal on macOS (wgpu there with the
/// `wgpu-effects` feature and `REEL_GPU=wgpu`), wgpu elsewhere. The
/// preview has one on its render thread; an export makes its own.
pub enum Renderer {
    #[cfg(target_os = "macos")]
    Metal(metal::Renderer),
    #[cfg(any(not(target_os = "macos"), feature = "wgpu-effects"))]
    Portable(portable::Renderer),
}

impl Renderer {
    /// None without a GPU.
    pub fn new() -> Option<Self> {
        #[cfg(all(target_os = "macos", feature = "wgpu-effects"))]
        if std::env::var("REEL_GPU").is_ok_and(|v| v == "wgpu") {
            return portable::Renderer::new().map(Renderer::Portable);
        }
        #[cfg(target_os = "macos")]
        return metal::Renderer::new().map(Renderer::Metal);
        #[cfg(not(target_os = "macos"))]
        return portable::Renderer::new().map(Renderer::Portable);
    }

    /// The pictures (one per `stack.layers`) through the stack: (width,
    /// height, BGRA).
    pub fn render(
        &mut self,
        frames: &[draw::Image],
        backdrop: Option<draw::Image>,
        stack: &Stack,
    ) -> (u32, u32, Vec<u8>) {
        match self {
            #[cfg(target_os = "macos")]
            Renderer::Metal(r) => r.render(frames, backdrop, stack),
            #[cfg(any(not(target_os = "macos"), feature = "wgpu-effects"))]
            Renderer::Portable(r) => r.render(frames, backdrop, stack),
        }
    }
}

/// The pictures to draw through a stack.
pub struct Job {
    /// Grows with every job: a result older than what's shown is dropped.
    pub seq: u64,
    /// One per `stack.layers`, back to front.
    pub frames: Vec<Arc<DecodedFrame>>,
    /// The Background photo, when the stack has one and it's loaded.
    pub backdrop: Option<Arc<DecodedFrame>>,
    pub stack: Stack,
}

/// A drawn frame: tightly packed BGRA.
pub struct Rendered {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// The job waiting (latest wins), and whether the `Worker` was dropped.
#[derive(Default)]
struct Pending {
    job: Option<Job>,
    quit: bool,
}

type Slot = Arc<(Mutex<Pending>, Condvar)>;

/// The render thread.
pub struct Worker {
    slot: Slot,
}

impl Worker {
    pub fn spawn(out: async_channel::Sender<Rendered>) -> Self {
        let slot: Slot = Arc::default();
        let jobs = slot.clone();
        std::thread::Builder::new()
            .name("effects".into())
            .spawn(move || {
                let mut renderer = Renderer::new();
                if renderer.is_none() {
                    eprintln!("no GPU device: effects, filters and layers won't show");
                }
                loop {
                    let job = {
                        let (lock, ready) = &*jobs;
                        let mut slot = lock.lock().unwrap();
                        loop {
                            if slot.quit {
                                return;
                            }
                            if let Some(job) = slot.job.take() {
                                break job;
                            }
                            slot = ready.wait(slot).unwrap();
                        }
                    };
                    let drawn = renderer.as_mut().map(|r| {
                        let frames: Vec<_> = job.frames.iter().map(|f| image(f)).collect();
                        r.render(&frames, job.backdrop.as_deref().map(image), &job.stack)
                    });
                    let Some((width, height, bgra)) = drawn.or_else(|| {
                        job.frames
                            .last()
                            .map(|f| (f.width, f.height, f.bgra.clone()))
                    }) else {
                        continue;
                    };
                    let rendered = Rendered {
                        seq: job.seq,
                        width,
                        height,
                        bgra,
                    };
                    if out.send_blocking(rendered).is_err() {
                        return;
                    }
                }
            })
            .expect("effects thread starts");
        Worker { slot }
    }

    /// Replaces any job not yet started.
    pub fn submit(&self, job: Job) {
        let (lock, ready) = &*self.slot;
        lock.lock().unwrap().job = Some(job);
        ready.notify_one();
    }
}

impl Drop for Worker {
    /// Ends the render thread (and frees its GPU device).
    fn drop(&mut self) {
        let (lock, ready) = &*self.slot;
        lock.lock().unwrap().quit = true;
        ready.notify_one();
    }
}
