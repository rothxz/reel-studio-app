//! AI face swap, on this computer: a chosen person in a video clip gets
//! the face from a photo. Nothing is uploaded; the models run through ONNX
//! Runtime (`inference`), on the GPU / Neural Engine when there is one.
//!
//! ```text
//! models     the model registry: pinned URLs, SHA-256, licences, download
//! inference  ONNX Runtime sessions (providers, CPU fallback, shape checks)
//! runtime    where ONNX Runtime's library comes from (loaded on Linux)
//! detector   faces + five landmarks (YuNet)
//! align      five-point similarity alignment, warps
//! embedder   identity vectors (ArcFace)
//! swapper    the swap model behind a trait (inswapper 128)
//! restorer   optional face restoration (GFPGAN)
//! blend      the stable crop mask, colour match, paste-back
//! tracker    following one person: identity checks, landmark smoothing
//! pipeline   shared models; source identity; people in a clip; a frame
//! job        processing a clip (reader thread → bounded queue → swap)
//! cache      the processed result on disk, and lookups by frame time
//! config     the clip's settings (document) and processing config
//! ```
//!
//! A swap is non-destructive: the clip keeps its media and stores its
//! settings (`faceSwap`); processing writes a result to the cache (the
//! face crop and its place, per frame), and the player and export paste
//! it over the frames they decode. Off, removed or cleared, the original
//! shows again.

pub mod align;
pub mod blend;
pub mod cache;
pub mod config;
pub mod detector;
pub mod embedder;
pub mod error;
pub mod inference;
pub mod job;
pub mod models;
pub mod onnx;
pub mod pipeline;
pub mod restorer;
pub mod runtime;
pub mod swapper;
pub mod tracker;
pub mod types;

/// The tag an exported video with a swapped face carries.
pub const MANIPULATED_NOTE: &str =
    "AI-edited: a face in this video was replaced with AI face swap (Reel Studio).";

pub use config::{FaceSwapSettings, Quality, TargetAnchor};
pub use error::FaceSwapError;
pub use job::{FaceSwapJob, FaceSwapProgress, FaceSwapResult};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod probe {
    /// Prints each downloaded model's inputs and outputs:
    /// `REEL_FACE_MODELS=<dir> cargo test face_swap::probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn print_model_ports() {
        let dir = std::path::PathBuf::from(std::env::var("REEL_FACE_MODELS").unwrap());
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "onnx") {
                continue;
            }
            let t = std::time::Instant::now();
            let m = super::inference::Model::load(&path, false, None).unwrap();
            println!("{} ({:?}, {:?})", path.display(), m.provider, t.elapsed());
            for p in &m.inputs {
                println!("  in  {} {:?} {:?}", p.name, p.ty, p.shape);
            }
            for p in &m.outputs {
                println!("  out {} {:?} {:?}", p.name, p.ty, p.shape);
            }
        }
    }
}
