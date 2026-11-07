//! The editor's dark palette and type. The editor chrome is always dark,
//! black and white, with monochrome icons (no accent colour in the tools).
//! Values match the old desktop editor's dark palette so panels read the
//! same side by side. The whole palette is kept, used yet or not.
#![allow(dead_code)]

use gpui::{Rgba, rgb, rgba};

// ---- surfaces -------------------------------------------------------------

/// Window background behind the panels (the gutters show it).
pub fn app_bg() -> Rgba {
    rgb(0x111113)
}
/// Panel fill (browser, player, timeline).
pub fn panel() -> Rgba {
    rgb(0x1a1a1c)
}
/// Inspector and dialog fill.
pub fn sheet() -> Rgba {
    rgb(0x1d1d1f)
}
/// The player's letterbox behind the canvas.
pub fn player_bg() -> Rgba {
    rgb(0x0a0a0b)
}
pub fn chip_bg() -> Rgba {
    rgb(0x111113)
}
pub fn button_bg() -> Rgba {
    rgb(0x26262a)
}
pub fn button_hover() -> Rgba {
    rgb(0x313136)
}
pub fn timeline_bg() -> Rgba {
    rgb(0x111113)
}
pub fn lane_bg() -> Rgba {
    rgb(0x111113)
}

// ---- lines and fills --------------------------------------------------------

pub fn card_border() -> Rgba {
    rgba(0xffffff0f)
}
pub fn chip_border() -> Rgba {
    rgba(0xffffff17)
}
pub fn hairline() -> Rgba {
    rgba(0xffffff0d)
}
pub fn fill_subtle() -> Rgba {
    rgba(0xffffff12)
}
pub fn fill_muted() -> Rgba {
    rgba(0xffffff1a)
}
pub fn fill_strong() -> Rgba {
    rgba(0xffffff24)
}

// ---- text -------------------------------------------------------------------

pub fn text() -> Rgba {
    rgb(0xf2f2f2)
}
pub fn text_secondary() -> Rgba {
    rgba(0xffffffa8)
}
pub fn text_tertiary() -> Rgba {
    rgba(0xffffff6b)
}
pub fn text_faint() -> Rgba {
    rgba(0xffffff59)
}
pub fn text_disabled() -> Rgba {
    rgba(0xffffff42)
}

// ---- shell accent (projects home only; the editor's tools stay black and white)

pub fn accent() -> Rgba {
    rgb(0xf5c542)
}
pub fn accent_text() -> Rgba {
    rgb(0x1a1608)
}
pub fn page_bg() -> Rgba {
    rgb(0x141416)
}

// ---- semantic ---------------------------------------------------------------

pub fn danger() -> Rgba {
    rgb(0xff5c5c)
}
pub fn success() -> Rgba {
    rgb(0x2fbf71)
}

// ---- clips --------------------------------------------------------------------

pub fn clip_video() -> Rgba {
    rgb(0x242426)
}
pub fn clip_audio() -> Rgba {
    rgb(0x2c4556)
}
pub fn clip_audio_wave() -> Rgba {
    rgb(0x3d9be9)
}
pub fn clip_text() -> Rgba {
    rgb(0x2d5fd1)
}
pub fn clip_sticker() -> Rgba {
    rgb(0x232327)
}
pub fn clip_effect() -> Rgba {
    rgb(0x3a2f5e)
}
pub fn clip_caption() -> Rgba {
    rgb(0x4a4fd0)
}
pub fn selected() -> Rgba {
    rgb(0xffffff)
}
/// Selected text in a field (white, monochrome chrome).
pub fn selection() -> Rgba {
    rgba(0xffffff40)
}
pub fn playhead() -> Rgba {
    rgb(0xffffff)
}
pub fn toast_bg() -> Rgba {
    rgba(0x1e1e22f5)
}
pub fn backdrop() -> Rgba {
    rgba(0x0000008c)
}

// ---- type ---------------------------------------------------------------------

/// Headings and body (rounded).
pub const FONT_BODY: &str = "Nunito";
/// Small uppercase labels, metadata and timecodes.
pub const FONT_MONO: &str = "IBM Plex Mono";

/// Text sizes in px (xxs 9 · xs 11 · sm 12 · md 14 · lg 16).
pub const XXS: f32 = 9.;
pub const XS: f32 = 11.;
pub const SM: f32 = 12.;
pub const MD: f32 = 14.;
pub const LG: f32 = 16.;

/// The fonts the UI draws with, embedded so the app looks the same on every
/// machine. Licences: `assets/fonts/README.md`.
pub fn embedded_fonts() -> Vec<std::borrow::Cow<'static, [u8]>> {
    macro_rules! font {
        ($file:literal) => {
            std::borrow::Cow::Borrowed(
                include_bytes!(concat!("../assets/fonts/", $file)).as_slice(),
            )
        };
    }
    vec![
        font!("Nunito_400Regular.ttf"),
        font!("Nunito_500Medium.ttf"),
        font!("Nunito_600SemiBold.ttf"),
        font!("Nunito_700Bold.ttf"),
        font!("IBMPlexMono_400Regular.ttf"),
        font!("IBMPlexMono_500Medium.ttf"),
        font!("IBMPlexMono_600SemiBold.ttf"),
    ]
}
