//! Where ONNX Runtime comes from. On macOS and Windows it's linked into the
//! program. On Linux it's loaded the first time a face model is: the
//! library ort can link in was built on a newer glibc (2.38+) than the
//! release supports (it builds on Ubuntu 22.04), so the release ships
//! Microsoft's own build (glibc 2.28+) next to the program instead,
//! pinned by `packaging/linux/fetch-onnxruntime.sh`.
//!
//! Looked for, first found wins:
//! 1. `ORT_DYLIB_PATH`, when set, and only it (another build, e.g. a
//!    CUDA one for `--features cuda`);
//! 2. next to the program (the tar.gz; a dev build after
//!    `packaging/linux/fetch-onnxruntime.sh x64 target/release`);
//! 3. `../lib/reel-studio/` from the program (the AppImage's
//!    `usr/lib/reel-studio`, a hand install under `~/.local`);
//! 4. the system's library path (`LD_LIBRARY_PATH`, a distribution's
//!    package).
//!
//! ort refuses a library older than the API it was built for (1.31).

// Only Linux loads the library; the search order is tested everywhere.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The library's file (its soname, as the release ships it).
pub const LIBRARY: &str = "libonnxruntime.so.1";

/// The ONNX Runtime version the release ships (and ort's API needs).
pub const VERSION: &str = "1.31";

/// Where to look, in order (see the module notes). `exe`: this program;
/// `env`: `ORT_DYLIB_PATH`. The last entry is the bare file name, which
/// the system's loader resolves.
pub fn candidates(exe: Option<&Path>, env: Option<&OsStr>) -> Vec<PathBuf> {
    if let Some(path) = env.filter(|p| !p.is_empty()) {
        return vec![PathBuf::from(path)];
    }
    let mut list = vec![];
    if let Some(dir) = exe.and_then(Path::parent) {
        list.push(dir.join(LIBRARY));
        if let Some(prefix) = dir.parent() {
            list.push(prefix.join("lib").join("reel-studio").join(LIBRARY));
        }
    }
    list.push(PathBuf::from(LIBRARY));
    list
}

/// Loads the library and starts an environment builder on it.
#[cfg(target_os = "linux")]
pub fn load() -> Result<ort::environment::EnvironmentBuilder, String> {
    let exe = std::env::current_exe().ok();
    let env = std::env::var_os("ORT_DYLIB_PATH");
    let from_env = env.as_ref().is_some_and(|p| !p.is_empty());
    // Libraries that were there but wouldn't load (a wrong version, a
    // missing dependency): worth saying; a file that isn't there isn't.
    let mut failed = vec![];
    for path in candidates(exe.as_deref(), env.as_deref()) {
        let by_name = path.parent().is_none_or(|p| p.as_os_str().is_empty());
        if !by_name && !path.is_file() {
            if from_env {
                failed.push(format!("ORT_DYLIB_PATH: {} doesn't exist", path.display()));
            }
            continue;
        }
        match ort::init_from(&path) {
            Ok(builder) => return Ok(builder),
            // By name, a failed open just means the system has none.
            Err(ort::LoadDynamicError::Dlopen { .. }) if by_name => {}
            Err(e) => failed.push(e.to_string()),
        }
    }
    let mut message = format!(
        "ONNX Runtime {VERSION} ({LIBRARY}) isn't installed with the app: put it next to \
         the program or set ORT_DYLIB_PATH to it"
    );
    if !failed.is_empty() {
        message.push_str(" (");
        message.push_str(&failed.join("; "));
        message.push(')');
    }
    Err(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_next_to_the_program_then_its_lib_then_the_system() {
        let exe = Path::new("/tmp/.mount_x/usr/bin/reel-studio");
        assert_eq!(
            candidates(Some(exe), None),
            [
                PathBuf::from("/tmp/.mount_x/usr/bin/libonnxruntime.so.1"),
                PathBuf::from("/tmp/.mount_x/usr/lib/reel-studio/libonnxruntime.so.1"),
                PathBuf::from("libonnxruntime.so.1"),
            ]
        );
        assert_eq!(candidates(None, None), [PathBuf::from(LIBRARY)]);
    }

    #[test]
    fn the_environment_variable_wins_alone() {
        let exe = Path::new("/opt/reel/reel-studio");
        let env = OsStr::new("/opt/ort-cuda/lib/libonnxruntime.so.1.31.0");
        assert_eq!(candidates(Some(exe), Some(env)), [PathBuf::from(env)]);
        // Set but empty is unset.
        assert_eq!(candidates(Some(exe), Some(OsStr::new(""))).len(), 3);
    }
}
