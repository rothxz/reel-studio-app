//! Video effects and filters in the preview.
//!
//! - `package`: the looks' shaders as downloaded data (+ the store).
//! - `stack`: what to draw at a time, as passes (the document → passes).
//! - `msl`: Metal source for each pass (the dialect rewritten, helpers).
//! - `metal`: the passes on the GPU (macOS).
//!
//! The editor hands the latest decoded frame and its stack to one render
//! thread (latest wins, like the decoders) and shows what comes back.
//! Without a GPU renderer (other platforms for now) frames pass through
//! unchanged.

#[cfg(target_os = "macos")]
pub mod metal;
pub mod msl;
pub mod package;
pub mod stack;

use std::sync::{Arc, Condvar, Mutex};

use crate::decoder::DecodedFrame;
use stack::Stack;

/// One frame to draw through a stack.
pub struct Job {
    /// Grows with every job: a result older than what's shown is dropped.
    pub seq: u64,
    pub frame: Arc<DecodedFrame>,
    pub stack: Stack,
}

/// A drawn frame: tightly packed BGRA.
pub struct Rendered {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

type Slot = Arc<(Mutex<Option<Job>>, Condvar)>;

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
                #[cfg(target_os = "macos")]
                let mut renderer = metal::Renderer::new();
                #[cfg(target_os = "macos")]
                if renderer.is_none() {
                    eprintln!("no Metal device: effects and filters won't show");
                }
                loop {
                    let job = {
                        let (lock, ready) = &*jobs;
                        let mut slot = lock.lock().unwrap();
                        loop {
                            if let Some(job) = slot.take() {
                                break job;
                            }
                            slot = ready.wait(slot).unwrap();
                        }
                    };
                    let f = &job.frame;
                    #[cfg(target_os = "macos")]
                    let drawn = renderer.as_mut().map(|r| {
                        let image = metal::Image {
                            width: f.width,
                            height: f.height,
                            bgra: &f.bgra,
                        };
                        r.render(image, &job.stack)
                    });
                    #[cfg(not(target_os = "macos"))]
                    let drawn: Option<(u32, u32, Vec<u8>)> = None;
                    let (width, height, bgra) =
                        drawn.unwrap_or_else(|| (f.width, f.height, f.bgra.clone()));
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
        *lock.lock().unwrap() = Some(job);
        ready.notify_one();
    }
}
