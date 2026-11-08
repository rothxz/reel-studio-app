//! Editor preferences that outlive a project: panel sizes and the timeline
//! switches. Stored as JSON in the app's data folder, read once at launch
//! and written when they change.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Prefs {
    /// Left browser width; None = a share of the window.
    pub browser_width: Option<f32>,
    pub inspector_width: f32,
    /// Timeline panel height; None = a share of the window.
    pub timeline_height: Option<f32>,
    pub main_track_magnet: bool,
    pub linkage: bool,
    pub preview_axis: bool,
    pub snapping: bool,
    /// Export resolution: index into `RESOLUTIONS`.
    pub export_resolution: usize,
    /// The folder the last export went to.
    pub export_dir: Option<String>,
    /// Settings → "Share anonymous usage data" (src/analytics.rs).
    pub share_usage_data: bool,
    /// Settings → Language: a language code (src/i18n.rs), or None for
    /// "Same as device".
    pub language: Option<String>,
    /// Captions › Style: the look new captions take (src/caption_styles.rs).
    pub caption_style: String,
    /// Captions › Auto captions: the spoken language picked last.
    pub caption_language: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            browser_width: None,
            inspector_width: 340.,
            timeline_height: None,
            main_track_magnet: true,
            linkage: true,
            preview_axis: true,
            snapping: true,
            export_resolution: 1,
            export_dir: None,
            share_usage_data: true,
            language: None,
            caption_style: crate::caption_styles::DEFAULT.into(),
            caption_language: "en".into(),
        }
    }
}

/// Export resolutions offered in the header (label, short side scale).
pub const RESOLUTIONS: [(&str, f32); 3] = [("720p", 720. / 1080.), ("1080p", 1.), ("4K", 2.)];

/// The user's home folder (`USERPROFILE` on Windows, which has no HOME).
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// The app's data folder (created on first use).
pub fn data_dir() -> PathBuf {
    let home = home_dir();
    let dir = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Reel Studio")
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or(home)
            .join("Reel Studio")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("reel-studio")
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn path() -> PathBuf {
    data_dir().join("desktop-editor.json")
}

impl Prefs {
    pub fn load() -> Prefs {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path(), json);
        }
    }
}
