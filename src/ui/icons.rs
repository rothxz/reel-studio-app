//! Monochrome icons from two bundled icon fonts (Ionicons and Material
//! Design Icons). GPUI won't load a font without an "m" glyph, so instead
//! of drawing them as text each glyph's outline is served as an SVG asset
//! (`icon/<font>/<codepoint>.svg`) and drawn with `svg()`, which tints it
//! with the text colour.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::sync::OnceLock;

use gpui::{
    AssetSource, Div, Hsla, ParentElement, SharedString, Styled, Svg, Transformation, div, px, svg,
};

const ION: &str = "ion";
const MDI: &str = "mdi";

static ION_TTF: &[u8] = include_bytes!("../../assets/fonts/Ionicons.ttf");
static MDI_TTF: &[u8] = include_bytes!("../../assets/fonts/MaterialCommunityIcons.ttf");

macro_rules! icons {
    ($($name:ident => ($font:ident, $code:literal),)*) => {
        /// The icon set (more than the panels use yet).
        #[allow(dead_code)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Icon { $($name,)* }
        impl Icon {
            fn glyph(self) -> (&'static str, u32) {
                match self { $(Icon::$name => ($font, $code),)* }
            }
        }
    };
}

icons! {
    Close => (ION, 0xf24a),
    Ellipse => (ION, 0xf2cb),
    CheckmarkCircle => (ION, 0xf21e),
    Checkmark => (ION, 0xf21d),
    ChevronDown => (ION, 0xf232),
    ChevronUp => (ION, 0xf241),
    ChevronForward => (ION, 0xf23b),
    ChevronBack => (ION, 0xf229),
    Sparkles => (ION, 0xf58d),
    Share => (ION, 0xf572),
    SkipBack => (ION, 0xf4d8),
    CaretBack => (ION, 0xf1ea),
    Play => (ION, 0xf4c6),
    Pause => (ION, 0xf493),
    CaretForward => (ION, 0xf1f6),
    SkipForward => (ION, 0xf4de),
    Expand => (ION, 0xf2e0),
    Contract => (ION, 0xf289),
    Trash => (ION, 0xf5f6),
    Copy => (ION, 0xf290),
    Crop => (ION, 0xf296),
    Swap => (ION, 0xf5b1),
    ZoomOut => (ION, 0xf522),
    ZoomIn => (ION, 0xf105),
    Add => (ION, 0xf103),
    Cut => (ION, 0xf29c),
    Options => (ION, 0xf48b),
    VolumeHigh => (ION, 0xf614),
    VolumeMute => (ION, 0xf61d),
    Eye => (ION, 0xf2ea),
    EyeOff => (ION, 0xf2e8),
    LockClosed => (ION, 0xf3c7),
    LockOpen => (ION, 0xf3cb),
    Layers => (ION, 0xf3b0),
    Text => (ION, 0xf5cc),
    Palette => (ION, 0xf27e),
    MusicalNotes => (ION, 0xf46a),
    MusicalNote => (ION, 0xf467),
    Timer => (ION, 0xf5e1),
    Film => (ION, 0xf2ff),
    Image => (ION, 0xf38f),
    Images => (ION, 0xf392),
    FolderOpen => (ION, 0xf327),
    CloudUpload => (ION, 0xf260),
    Search => (ION, 0xf55f),
    Ellipsis => (ION, 0xf2ce),
    Pencil => (ION, 0xf49c),
    Checkbox => (ION, 0xf21b),
    Body => (ION, 0xf1a0),
    ColorFilter => (ION, 0xf27b),
    Happy => (ION, 0xf362),
    Star => (ION, 0xf599),
    Compare => (ION, 0xf341),
    Chatbox => (ION, 0xf20d),
    Mic => (ION, 0xf45e),
    Document => (ION, 0xf2b4),
    Warning => (ION, 0xf629),
    Info => (ION, 0xf399),
    Settings => (ION, 0xf56c),
    Download => (ION, 0xf2ba),
    Reload => (ION, 0xf51a),
    Videocam => (ION, 0xf60d),
    Shuffle => (ION, 0xf583),
    Keyboard => (MDI, 0xf097b),
    Undo => (MDI, 0xf17b3),
    Redo => (MDI, 0xf17b7),
    Split => (MDI, 0xf00fb),
    Blade => (MDI, 0xf00d5),
    Cursor => (MDI, 0xf01bf),
    DeleteLeft => (MDI, 0xf0325),
    DeleteRight => (MDI, 0xf0312),
    Magnet => (MDI, 0xf0347),
    Link => (MDI, 0xf0339),
    LinkOff => (MDI, 0xf033a),
    PreviewAxis => (MDI, 0xf061e),
    Snapping => (MDI, 0xf084c),
    ZoomFit => (MDI, 0xf084e),
    Snowflake => (MDI, 0xf0717),
    ExtractAudio => (MDI, 0xf0dde),
    VectorSquare => (MDI, 0xf0001),
    VectorCircle => (MDI, 0xf0556),
    Speedometer => (MDI, 0xf04c5),
    MotionPlay => (MDI, 0xf1591),
    Rhombus => (MDI, 0xf070b),
    RhombusOutline => (MDI, 0xf070c),
    Plus => (MDI, 0xf0415),
    Curve => (MDI, 0xf0fa7),
    Voice => (MDI, 0xf05cb),
    Rotate => (MDI, 0xf0885),
    MdiClose => (MDI, 0xf0156),
    MdiPencil => (MDI, 0xf03eb),
    MdiRotateRight => (MDI, 0xf0467),
    Waveform => (MDI, 0xf147d),
    Title => (MDI, 0xf05f4),
    Combine => (MDI, 0xf0558),
    Crosshairs => (MDI, 0xf01a4),
    FocusCenter => (MDI, 0xf02f1),
    Opacity => (MDI, 0xf1853),
    Sparkle4 => (MDI, 0xf0ae3),
    ClosedCaption => (MDI, 0xf0dbd),
    TextEdit => (MDI, 0xf1a7d),
    Sticker => (MDI, 0xf0785),
    Transition => (MDI, 0xf0915),
    FormatText => (MDI, 0xf0284),
    Filmstrip => (MDI, 0xf0230),
    Ban => (MDI, 0xf073a),
    RotateLeft => (ION, 0xf143),
    RotateRight => (ION, 0xf13d),
    SwapHorizontal => (ION, 0xf5b1),
    SwapVertical => (ION, 0xf5b4),
    BanOutline => (ION, 0xf16a),
    MaskHorizontal => (MDI, 0xf0374),
    MaskMirror => (MDI, 0xf11d8),
    CircleOutline => (MDI, 0xf0766),
    RectangleOutline => (MDI, 0xf0e5f),
    FormatBold => (MDI, 0xf0264),
    FormatItalic => (MDI, 0xf0277),
    FormatUnderline => (MDI, 0xf0287),
    AlignLeft => (MDI, 0xf0262),
    AlignCenter => (MDI, 0xf0260),
    AlignRight => (MDI, 0xf0263),
    Brush => (MDI, 0xf00e3),
    HeartOutline => (MDI, 0xf02d5),
    StarOutline => (MDI, 0xf04d2),
    MdiLayers => (MDI, 0xf09fe),
    SkipBackOutline => (ION, 0xf4dc),
    SkipForwardOutline => (ION, 0xf4e2),
    Refresh => (ION, 0xf514),
    ExpandOutline => (ION, 0xf2e1),
}

impl Icon {
    /// The icon as a centred box `size` px square.
    pub fn el(self, size: f32, color: impl Into<Hsla>) -> Div {
        let (font, code) = self.glyph();
        div()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .size(px(size))
            .child(
                svg()
                    .path(SharedString::from(format!("icon/{font}/{code:x}.svg")))
                    .size(px(size))
                    .text_color(color.into()),
            )
    }

    /// The icon `size` px square, turned clockwise by `radians`.
    pub fn turned(self, size: f32, color: impl Into<Hsla>, radians: f32) -> Svg {
        let (font, code) = self.glyph();
        svg()
            .path(SharedString::from(format!("icon/{font}/{code:x}.svg")))
            .size(px(size))
            .flex_none()
            .text_color(color.into())
            .with_transformation(Transformation::rotate(gpui::radians(radians)))
    }
}

fn face(font: &str) -> Option<&'static ttf_parser::Face<'static>> {
    static ION_FACE: OnceLock<Option<ttf_parser::Face<'static>>> = OnceLock::new();
    static MDI_FACE: OnceLock<Option<ttf_parser::Face<'static>>> = OnceLock::new();
    match font {
        ION => ION_FACE
            .get_or_init(|| ttf_parser::Face::parse(ION_TTF, 0).ok())
            .as_ref(),
        MDI => MDI_FACE
            .get_or_init(|| ttf_parser::Face::parse(MDI_TTF, 0).ok())
            .as_ref(),
        _ => None,
    }
}

/// Writes a glyph outline as SVG path data, flipped to y-down.
struct PathData {
    d: String,
    top: f32,
}

impl ttf_parser::OutlineBuilder for PathData {
    fn move_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.d, "M{x} {}", self.top - y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.d, "L{x} {}", self.top - y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let _ = write!(self.d, "Q{x1} {} {x} {}", self.top - y1, self.top - y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let t = self.top;
        let _ = write!(self.d, "C{x1} {} {x2} {} {x} {}", t - y1, t - y2, t - y);
    }
    fn close(&mut self) {
        self.d.push('Z');
    }
}

/// One icon glyph as an SVG document: an em square centred on the glyph's
/// advance and the font's line box, as the glyph would sit in text.
pub fn glyph_svg(font: &str, code: u32) -> Option<String> {
    let face = face(font)?;
    let glyph = face.glyph_index(char::from_u32(code)?)?;
    let em = face.units_per_em() as f32;
    let (asc, desc) = (face.ascender() as f32, face.descender() as f32);
    let advance = face.glyph_hor_advance(glyph).map_or(em, f32::from);
    let mut path = PathData {
        d: String::new(),
        top: asc,
    };
    face.outline_glyph(glyph, &mut path)?;
    let x = advance / 2. - em / 2.;
    let y = (asc - desc) / 2. - em / 2.;
    Some(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{x} {y} {em} {em}"><path d="{}"/></svg>"#,
        path.d
    ))
}

/// Serves `icon/<font>/<hex codepoint>.svg` from the bundled icon fonts.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let Some(rest) = path.strip_prefix("icon/") else {
            return Ok(None);
        };
        let Some((font, file)) = rest.split_once('/') else {
            return Ok(None);
        };
        let code = file
            .strip_suffix(".svg")
            .and_then(|h| u32::from_str_radix(h, 16).ok());
        Ok(code
            .and_then(|c| glyph_svg(font, c))
            .map(|s| Cow::Owned(s.into_bytes())))
    }

    fn list(&self, _: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_has_an_outline() {
        // A sample from each font; a wrong codepoint gives no glyph.
        for i in [
            Icon::Close,
            Icon::Play,
            Icon::Trash,
            Icon::Undo,
            Icon::Magnet,
            Icon::Filmstrip,
        ] {
            let (font, code) = i.glyph();
            let svg = glyph_svg(font, code).unwrap_or_else(|| panic!("{i:?}"));
            assert!(svg.contains("<path d=\"M"), "{i:?}");
        }
    }
}
