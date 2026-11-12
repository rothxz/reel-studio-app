//! What can go wrong in a face swap, worded for the panel.

use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum FaceSwapError {
    /// A model file isn't downloaded yet (its id).
    ModelMissing(&'static str),
    /// A model file is there but isn't the one we pinned (its id).
    ModelMismatch(&'static str),
    /// A model's inputs / outputs aren't what the pipeline was written for.
    ModelShape(String),
    /// ONNX Runtime's library isn't there or won't load (Linux and Intel
    /// Macs, where it isn't linked in): what was looked for.
    #[cfg_attr(not(ort_dynamic), allow(dead_code))]
    RuntimeMissing(String),
    /// ONNX Runtime failed (load or run).
    Inference(String),
    /// The source photo has no face the detector is sure of.
    NoSourceFace,
    /// The chosen person isn't at the anchor frame any more.
    TargetNotFound,
    Decode(String),
    Io(String),
    InvalidConfig(String),
    Cancelled,
}

impl fmt::Display for FaceSwapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ModelMissing(id) => write!(f, "The {id} model isn't downloaded"),
            Self::ModelMismatch(id) => write!(
                f,
                "The {id} model file doesn't match its checksum; download it again"
            ),
            Self::ModelShape(m) => write!(f, "Unexpected model: {m}"),
            Self::RuntimeMissing(m) => write!(f, "{m}"),
            Self::Inference(m) => write!(f, "Face model error: {m}"),
            Self::NoSourceFace => write!(f, "No face found in the source photo"),
            Self::TargetNotFound => write!(
                f,
                "The chosen person can't be found in the clip; detect faces again"
            ),
            Self::Decode(m) => write!(f, "Can't read the video: {m}"),
            Self::Io(m) => write!(f, "{m}"),
            Self::InvalidConfig(m) => write!(f, "{m}"),
            Self::Cancelled => write!(f, "Cancelled"),
        }
    }
}

impl std::error::Error for FaceSwapError {}

impl From<ort::Error> for FaceSwapError {
    fn from(e: ort::Error) -> Self {
        Self::Inference(e.to_string())
    }
}

impl From<std::io::Error> for FaceSwapError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

pub type Result<T, E = FaceSwapError> = std::result::Result<T, E>;
