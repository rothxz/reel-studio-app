//! Where ONNX Runtime comes from. On Apple silicon and Windows it's linked
//! into the program. Elsewhere (`cfg(ort_dynamic)`, set by build.rs) it's
//! loaded the first time a face model is, from the copy the release ships:
//!
//! - Linux: the library ort can link in was built on a newer glibc (2.38+)
//!   than the release supports (it builds on Ubuntu 22.04), so the release
//!   ships Microsoft's own 1.31 (glibc 2.28+), pinned by
//!   `packaging/linux/fetch-onnxruntime.sh`.
//! - Intel Macs: ort has no prebuilt library for them; Microsoft's last
//!   Intel build is 1.23.2 (macOS 13.4+), which the app bundle carries in
//!   `Contents/Frameworks`, pinned by `packaging/macos/fetch-onnxruntime.sh`.
//!
//! Looked for, first found wins:
//! 1. `ORT_DYLIB_PATH`, when set, and only it (another build, e.g. a
//!    CUDA one for `--features cuda`);
//! 2. where the release puts it (`Layout::dirs`, from the program's
//!    folder): next to the program (the tar.gz, a dev build after
//!    fetching it into `target/release`), `../lib/reel-studio` (the
//!    AppImage, the .deb / .rpm, a hand install under `~/.local`),
//!    `../Frameworks` (the macOS app);
//! 3. the system's library path, by name.
//!
//! ort refuses a library older than the API it was built for.

// Only `ort_dynamic` targets load the library; the search order is tested
// everywhere.
#![cfg_attr(not(ort_dynamic), allow(dead_code))]

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

/// Where one platform's release keeps the library.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    /// The file (on Linux its soname).
    pub library: &'static str,
    /// The ONNX Runtime the release ships (and the oldest ort accepts).
    pub version: &'static str,
    /// Folders to look in, relative to the program's, in order.
    pub dirs: &'static [&'static str],
}

pub const LINUX: Layout = Layout {
    library: "libonnxruntime.so.1",
    version: "1.31",
    dirs: &["", "../lib/reel-studio"],
};

pub const INTEL_MAC: Layout = Layout {
    library: "libonnxruntime.dylib",
    version: "1.23",
    dirs: &["../Frameworks", ""],
};

/// This build's.
pub const HERE: Layout = if cfg!(target_os = "macos") {
    INTEL_MAC
} else {
    LINUX
};

/// Where to look, in order (see the module notes). `exe`: this program;
/// `env`: `ORT_DYLIB_PATH`. The last entry is the bare file name, which
/// the system's loader resolves.
pub fn candidates(layout: Layout, exe: Option<&Path>, env: Option<&OsStr>) -> Vec<PathBuf> {
    if let Some(path) = env.filter(|p| !p.is_empty()) {
        return vec![PathBuf::from(path)];
    }
    let mut list = vec![];
    if let Some(dir) = exe.and_then(Path::parent) {
        for rel in layout.dirs {
            let mut path = dir.to_path_buf();
            for part in Path::new(rel).components() {
                match part {
                    Component::ParentDir => {
                        path.pop();
                    }
                    Component::Normal(name) => path.push(name),
                    _ => {}
                }
            }
            list.push(path.join(layout.library));
        }
    }
    list.push(PathBuf::from(layout.library));
    list
}

/// Loads the library and starts an environment builder on it.
#[cfg(ort_dynamic)]
pub fn load() -> Result<ort::environment::EnvironmentBuilder, String> {
    let exe = std::env::current_exe().ok();
    let env = std::env::var_os("ORT_DYLIB_PATH");
    let from_env = env.as_ref().is_some_and(|p| !p.is_empty());
    // Libraries that were there but wouldn't load (a wrong version, a
    // missing dependency): worth saying; a file that isn't there isn't.
    let mut failed = vec![];
    for path in candidates(HERE, exe.as_deref(), env.as_deref()) {
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
    let Layout {
        library, version, ..
    } = HERE;
    let mut message = format!(
        "ONNX Runtime {version} ({library}) isn't installed with the app: reinstall the app, \
         or set ORT_DYLIB_PATH to the library"
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
    fn linux_looks_next_to_the_program_then_its_lib_then_the_system() {
        let exe = Path::new("/tmp/.mount_x/usr/bin/reel-studio");
        assert_eq!(
            candidates(LINUX, Some(exe), None),
            [
                PathBuf::from("/tmp/.mount_x/usr/bin/libonnxruntime.so.1"),
                PathBuf::from("/tmp/.mount_x/usr/lib/reel-studio/libonnxruntime.so.1"),
                PathBuf::from("libonnxruntime.so.1"),
            ]
        );
        assert_eq!(
            candidates(LINUX, None, None),
            [PathBuf::from(LINUX.library)]
        );
    }

    #[test]
    fn a_mac_looks_in_the_bundles_frameworks_first() {
        let exe = Path::new("/Applications/Reel Studio.app/Contents/MacOS/reel-studio");
        assert_eq!(
            candidates(INTEL_MAC, Some(exe), None),
            [
                PathBuf::from(
                    "/Applications/Reel Studio.app/Contents/Frameworks/libonnxruntime.dylib"
                ),
                PathBuf::from("/Applications/Reel Studio.app/Contents/MacOS/libonnxruntime.dylib"),
                PathBuf::from("libonnxruntime.dylib"),
            ]
        );
    }

    #[test]
    fn the_environment_variable_wins_alone() {
        let exe = Path::new("/opt/reel/reel-studio");
        let env = OsStr::new("/opt/ort-cuda/lib/libonnxruntime.so.1.31.0");
        assert_eq!(
            candidates(LINUX, Some(exe), Some(env)),
            [PathBuf::from(env)]
        );
        // Set but empty is unset.
        assert_eq!(candidates(LINUX, Some(exe), Some(OsStr::new(""))).len(), 3);
    }
}
