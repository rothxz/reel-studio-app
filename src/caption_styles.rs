//! Captions › Style: one-click looks for every caption. A style owns the
//! whole look (font, fill, outline, shadow, glow, box, spacing) and how
//! many lines a caption shows (Monoline: one, Multiline: two); where the
//! captions sit and their size stay as the user left them. A pick restyles
//! every caption and is the look the next ones take.

use serde_json::{Map, Value, json};

pub const DEFAULT: &str = "classic";

/// The side nav's tabs (key, label).
pub const TABS: &[(&str, &str)] = &[
    ("all", "All"),
    ("monoline", "Monoline"),
    ("multiline", "Multiline"),
];

#[derive(Clone, Copy, Debug)]
pub struct CaptionStyle {
    pub id: &'static str,
    pub label: &'static str,
    /// Lines a caption shows: 1 or 2.
    pub lines: u8,
    /// A bundled face (`Family_400Regular`…).
    pub font: &'static str,
    pub bold: bool,
    pub italic: bool,
    pub color: &'static str,
    /// Outline colour and width.
    pub outline: Option<(&'static str, f64)>,
    /// Drop shadow colour and opacity.
    pub shadow: Option<(&'static str, f64)>,
    /// Glow colour and intensity.
    pub glow: Option<(&'static str, f64)>,
    /// Box colour, opacity, corner radius.
    pub boxed: Option<(&'static str, f64, f64)>,
    pub letter_spacing: Option<f64>,
}

const BASE: CaptionStyle = CaptionStyle {
    id: "",
    label: "",
    lines: 1,
    font: "Nunito_700Bold",
    bold: true,
    italic: false,
    color: "#ffffff",
    outline: None,
    shadow: None,
    glow: None,
    boxed: None,
    letter_spacing: None,
};

pub const STYLES: &[CaptionStyle] = &[
    // Monoline
    CaptionStyle {
        id: "classic",
        label: "Classic",
        outline: Some(("#000000", 0.45)),
        ..BASE
    },
    CaptionStyle {
        id: "clean",
        label: "Clean",
        font: "Nunito_500Medium",
        bold: false,
        shadow: Some(("#000000", 0.7)),
        ..BASE
    },
    CaptionStyle {
        id: "yellow",
        label: "Yellow",
        italic: true,
        color: "#ffe14d",
        outline: Some(("#000000", 0.5)),
        ..BASE
    },
    CaptionStyle {
        id: "box-black",
        label: "Black box",
        boxed: Some(("#000000", 0.85, 0.2)),
        ..BASE
    },
    CaptionStyle {
        id: "box-white",
        label: "White box",
        color: "#111111",
        boxed: Some(("#ffffff", 0.95, 0.2)),
        ..BASE
    },
    CaptionStyle {
        id: "glow",
        label: "Glow",
        glow: Some(("#5ad7ff", 0.8)),
        ..BASE
    },
    CaptionStyle {
        id: "neon",
        label: "Neon",
        color: "#ffd6f5",
        glow: Some(("#ff3fb4", 1.0)),
        ..BASE
    },
    CaptionStyle {
        id: "type",
        label: "Typewriter",
        font: "IBMPlexMono_600SemiBold",
        bold: false,
        boxed: Some(("#000000", 0.7, 0.05)),
        ..BASE
    },
    // Multiline
    CaptionStyle {
        id: "subtitle",
        label: "Subtitle",
        lines: 2,
        font: "Nunito_600SemiBold",
        bold: false,
        shadow: Some(("#000000", 0.8)),
        ..BASE
    },
    CaptionStyle {
        id: "soft-box",
        label: "Soft box",
        lines: 2,
        font: "Nunito_600SemiBold",
        bold: false,
        boxed: Some(("#000000", 0.55, 0.35)),
        ..BASE
    },
    CaptionStyle {
        id: "bold-outline",
        label: "Bold outline",
        lines: 2,
        outline: Some(("#000000", 0.6)),
        letter_spacing: Some(0.02),
        ..BASE
    },
    CaptionStyle {
        id: "news",
        label: "News",
        lines: 2,
        color: "#111111",
        boxed: Some(("#f4f4f4", 1.0, 0.0)),
        ..BASE
    },
    CaptionStyle {
        id: "mono-lines",
        label: "Mono",
        lines: 2,
        font: "IBMPlexMono_500Medium",
        bold: false,
        outline: Some(("#000000", 0.35)),
        ..BASE
    },
];

/// The style an id names, else the default.
pub fn get(id: &str) -> &'static CaptionStyle {
    STYLES.iter().find(|s| s.id == id).unwrap_or(&STYLES[0])
}

pub fn in_tab(key: &str) -> Vec<&'static CaptionStyle> {
    STYLES
        .iter()
        .filter(|s| match key {
            "monoline" => s.lines == 1,
            "multiline" => s.lines == 2,
            _ => true,
        })
        .collect()
}

/// Every look field a style owns: cleared before a style is put on.
const OWNED: &[&str] = &[
    "templateId",
    "fontFamily",
    "textOpacity",
    "bold",
    "italic",
    "color",
    "outlineColor",
    "outlineWidth",
    "shadowColor",
    "shadowOpacity",
    "shadowBlur",
    "shadowDistance",
    "shadowAngle",
    "glowColor",
    "glowIntensity",
    "glowRange",
    "backgroundColor",
    "backgroundStyle",
    "backgroundOpacity",
    "backgroundRadius",
    "backgroundHeight",
    "backgroundWidth",
    "letterSpacing",
    "textAnimations",
];

impl CaptionStyle {
    /// Puts this look on a caption's fields (its text, place, size and
    /// language stay).
    pub fn apply(&self, extra: &mut Map<String, Value>) {
        for k in OWNED {
            extra.remove(*k);
        }
        extra.insert("fontFamily".into(), self.font.into());
        extra.insert("bold".into(), self.bold.into());
        extra.insert("italic".into(), self.italic.into());
        extra.insert("color".into(), self.color.into());
        extra.insert("entrance".into(), "none".into());
        extra.insert("exit".into(), "none".into());
        if let Some((c, w)) = self.outline {
            extra.insert("outlineColor".into(), c.into());
            extra.insert("outlineWidth".into(), json!(w));
        }
        if let Some((c, o)) = self.shadow {
            extra.insert("shadowColor".into(), c.into());
            extra.insert("shadowOpacity".into(), json!(o));
        }
        if let Some((c, k)) = self.glow {
            extra.insert("glowColor".into(), c.into());
            extra.insert("glowIntensity".into(), json!(k));
        }
        if let Some((c, o, r)) = self.boxed {
            extra.insert("backgroundColor".into(), c.into());
            extra.insert("backgroundOpacity".into(), json!(o));
            extra.insert("backgroundRadius".into(), json!(r));
        }
        if let Some(s) = self.letter_spacing {
            extra.insert("letterSpacing".into(), json!(s));
        }
        let mut caption = extra
            .get("caption")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        caption.insert("lines".into(), json!(self.lines));
        extra.insert("caption".into(), Value::Object(caption));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_style_replaces_the_whole_look_and_keeps_the_rest() {
        let mut extra = Map::new();
        extra.insert("glowColor".into(), "#ff0000".into());
        extra.insert("fontSize".into(), json!(30));
        extra.insert("caption".into(), json!({ "language": "km", "lines": 1 }));
        get("soft-box").apply(&mut extra);
        assert!(!extra.contains_key("glowColor"));
        assert_eq!(extra["fontSize"], json!(30));
        assert_eq!(extra["backgroundColor"], json!("#000000"));
        assert_eq!(extra["caption"], json!({ "language": "km", "lines": 2 }));
        assert_eq!(get("nope").id, DEFAULT);
        assert_eq!(in_tab("all").len(), STYLES.len());
        assert_eq!(
            in_tab("monoline").len() + in_tab("multiline").len(),
            STYLES.len()
        );
    }
}
