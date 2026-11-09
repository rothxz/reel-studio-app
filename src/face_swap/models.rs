//! The face models: what each is, where it's from, its licence, and its
//! file in `<data>/models/face-swap/`. None is bundled with the app: each
//! is downloaded once the user agrees to (after seeing its licence), only
//! from the URL pinned here, and only kept when its SHA-256 matches.
//!
//! Licences differ per model and are shown before downloading:
//! - YuNet (detector): MIT.
//! - ArcFace w600k R50 (identity) and inswapper 128 (swap): InsightFace's
//!   pretrained models, released for non-commercial research use only.
//! - GFPGAN 1.4 (restoration, optional): Apache-2.0.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

use super::error::{FaceSwapError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum License {
    Mit,
    Apache2,
    /// Research and personal use only, no commercial use.
    NonCommercial,
}

impl License {
    pub fn label(self) -> &'static str {
        match self {
            License::Mit => "MIT",
            License::Apache2 => "Apache-2.0",
            License::NonCommercial => "Non-commercial use only",
        }
    }

    pub fn commercial(self) -> bool {
        !matches!(self, License::NonCommercial)
    }
}

#[derive(Debug)]
pub struct ModelSpec {
    pub id: &'static str,
    pub name: &'static str,
    /// Changes when the file does (part of every result's cache key).
    pub version: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
    pub license: License,
    /// Who made it, and where (the licence is there), for the licence line.
    pub author: &'static str,
    pub home: &'static str,
}

pub const DETECTOR: ModelSpec = ModelSpec {
    id: "yunet",
    name: "Face detector (YuNet)",
    version: "2023mar",
    file: "face_detection_yunet_2023mar.onnx",
    url: "https://huggingface.co/opencv/face_detection_yunet/resolve/3cc26e7f1014a5ee5d74a42acee58bafc9d0a310/face_detection_yunet_2023mar.onnx",
    sha256: "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
    bytes: 232_589,
    license: License::Mit,
    author: "OpenCV Zoo (Shiqi Yu et al.)",
    home: "https://github.com/opencv/opencv_zoo/tree/main/models/face_detection_yunet",
};

pub const EMBEDDER: ModelSpec = ModelSpec {
    id: "arcface",
    name: "Face identity (ArcFace R50)",
    version: "w600k_r50",
    file: "arcface_w600k_r50.onnx",
    url: "https://huggingface.co/facefusion/models-3.0.0/resolve/728b9659bd9691bf32cbf7f61af478d94b7ba81e/arcface_w600k_r50.onnx",
    sha256: "f1f79dc3b0b79a69f94799af1fffebff09fbd78fd96a275fd8f0cbbea23270d1",
    bytes: 174_388_474,
    license: License::NonCommercial,
    author: "InsightFace",
    home: "https://github.com/deepinsight/insightface",
};

/// The fp32 build. An fp16 one is half the download (278 MB) and as fast
/// on Core ML, but ONNX Runtime's CPU kernels run it about 12× slower
/// (1.1 s against 0.1 s a frame), and the CPU is every machine's fallback.
pub const SWAPPER: ModelSpec = ModelSpec {
    id: "inswapper",
    name: "Face swap (inswapper 128)",
    version: "128",
    file: "inswapper_128.onnx",
    url: "https://huggingface.co/facefusion/models-3.0.0/resolve/728b9659bd9691bf32cbf7f61af478d94b7ba81e/inswapper_128.onnx",
    sha256: "a290273ed497312095dac48cdef20feec9d5208298223dd01288ab202b54bea7",
    bytes: 555_303_150,
    license: License::NonCommercial,
    author: "InsightFace",
    home: "https://github.com/deepinsight/insightface",
};

pub const RESTORER: ModelSpec = ModelSpec {
    id: "gfpgan",
    name: "Face restoration (GFPGAN 1.4)",
    version: "1.4",
    file: "gfpgan_1.4.onnx",
    url: "https://huggingface.co/facefusion/models-3.0.0/resolve/728b9659bd9691bf32cbf7f61af478d94b7ba81e/gfpgan_1.4.onnx",
    sha256: "accc4757b26bdb89b32b4d3500d4f79c9dff97c1dd7c7104bf9dcb95e3311385",
    bytes: 340_299_087,
    license: License::Apache2,
    author: "Tencent ARC",
    home: "https://github.com/TencentARC/GFPGAN",
};

/// What a face swap needs; the restorer is only for its option.
pub const REQUIRED: [&ModelSpec; 3] = [&DETECTOR, &EMBEDDER, &SWAPPER];
pub const ALL: [&ModelSpec; 4] = [&DETECTOR, &EMBEDDER, &SWAPPER, &RESTORER];

/// The registry's version: every model's, one string (a cache key part).
pub fn versions() -> String {
    ALL.iter()
        .map(|m| format!("{}@{}", m.id, m.version))
        .collect::<Vec<_>>()
        .join(",")
}

/// `<data>/models/face-swap`, or `REEL_FACE_MODELS` (a folder holding the
/// same files: development and tests).
pub fn dir() -> PathBuf {
    match std::env::var_os("REEL_FACE_MODELS") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => crate::prefs::data_dir().join("models").join("face-swap"),
    }
}

/// Compiled Core ML programs, kept so a model isn't compiled each launch.
pub fn compile_cache() -> PathBuf {
    dir().join("compiled")
}

impl ModelSpec {
    pub fn path(&self) -> PathBuf {
        dir().join(self.file)
    }

    /// The file's there with the size it should have. Its hash was checked
    /// when it was downloaded (`verified` to check again).
    pub fn present(&self) -> bool {
        std::fs::metadata(self.path()).is_ok_and(|m| m.len() == self.bytes)
    }

    /// Hashes the file and compares (slow: hundreds of MB).
    #[cfg(test)]
    pub fn verified(&self) -> Result<()> {
        let path = self.path();
        if !path.is_file() {
            return Err(FaceSwapError::ModelMissing(self.id));
        }
        if sha256_file(&path)? == self.sha256 {
            Ok(())
        } else {
            Err(FaceSwapError::ModelMismatch(self.id))
        }
    }
}

/// Whether every model in `specs` is downloaded.
pub fn all_present(specs: &[&ModelSpec]) -> bool {
    specs.iter().all(|m| m.present())
}

/// Bytes still to download for `specs`.
pub fn missing_bytes(specs: &[&ModelSpec]) -> u64 {
    specs.iter().filter(|m| !m.present()).map(|m| m.bytes).sum()
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Downloads `spec` from its pinned URL into a `.part` file, hashing as it
/// goes; renamed into place only when the size and SHA-256 match.
/// `progress(bytes so far)`.
pub fn download(spec: &ModelSpec, cancel: &AtomicBool, progress: &dyn Fn(u64)) -> Result<()> {
    if spec.present() {
        return Ok(());
    }
    let path = spec.path();
    std::fs::create_dir_all(dir())?;
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_connect(Some(std::time::Duration::from_secs(15)))
            .build(),
    );
    let mut res = agent
        .get(spec.url)
        .call()
        .map_err(|e| FaceSwapError::Io(format!("Couldn't download {}: {e}", spec.name)))?;
    let part = path.with_extension("part");
    let mut file = std::fs::File::create(&part)?;
    let mut reader = res.body_mut().with_config().limit(spec.bytes + 1).reader();
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut got = 0u64;
    let result = loop {
        if cancel.load(Ordering::Relaxed) {
            break Err(FaceSwapError::Cancelled);
        }
        let n = match reader.read(&mut buf) {
            Ok(n) => n,
            Err(e) => break Err(FaceSwapError::Io(format!("{} download: {e}", spec.name))),
        };
        if n == 0 {
            break Ok(());
        }
        file.write_all(&buf[..n])?;
        hash.update(&buf[..n]);
        got += n as u64;
        progress(got);
    };
    crate::data_usage::record(crate::data_usage::Kind::AiModels, got);
    let checked = result.and_then(|()| {
        file.sync_all()?;
        if got != spec.bytes || hex(&hash.finalize()) != spec.sha256 {
            Err(FaceSwapError::ModelMismatch(spec.id))
        } else {
            Ok(())
        }
    });
    drop(file);
    match checked {
        Ok(()) => {
            std::fs::rename(&part, &path)?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_pinned() {
        for m in ALL {
            assert_eq!(m.sha256.len(), 64, "{}", m.id);
            assert!(m.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            // Pinned to a commit, never a moving branch.
            assert!(!m.url.contains("/resolve/main/"), "{}", m.id);
            assert!(m.url.starts_with("https://"));
            assert!(m.bytes > 0);
        }
        let ids: Vec<_> = ALL.iter().map(|m| m.id).collect();
        let mut unique = ids.clone();
        unique.dedup();
        assert_eq!(ids, unique);
        assert!(versions().contains("inswapper@128,"));
    }

    #[test]
    fn hashes_files() {
        let path = std::env::temp_dir().join(format!("sha-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_file(path);
    }

    /// Downloads every model and checks it (network, ~1.1 GB):
    /// `cargo test face_swap::models -- --ignored`.
    #[test]
    #[ignore]
    fn every_pinned_model_downloads() {
        for m in ALL {
            download(m, &AtomicBool::new(false), &|_| {}).unwrap();
            m.verified().unwrap();
        }
    }
}
