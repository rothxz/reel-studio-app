//! The editor's dark palette (black/white chrome, as in the app's editor).

use gpui::{Rgba, rgb, rgba};

pub fn bg() -> Rgba {
    rgb(0x0b0b0c)
}
pub fn panel() -> Rgba {
    rgb(0x141416)
}
pub fn panel_alt() -> Rgba {
    rgb(0x1b1b1e)
}
pub fn border() -> Rgba {
    rgb(0x26262a)
}
pub fn text() -> Rgba {
    rgb(0xf2f2f3)
}
pub fn text_dim() -> Rgba {
    rgb(0x8c8c94)
}
pub fn hover() -> Rgba {
    rgba(0xffffff14)
}
pub fn clip_video() -> Rgba {
    rgb(0x2f3a4a)
}
pub fn clip_audio() -> Rgba {
    rgb(0x23423a)
}
pub fn clip_other() -> Rgba {
    rgb(0x3d3350)
}
pub fn selected() -> Rgba {
    rgb(0xffffff)
}
pub fn playhead() -> Rgba {
    rgb(0xffffff)
}
