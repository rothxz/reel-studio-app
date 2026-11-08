//! Adjust: seven colour sliders on a video or photo, stored as one effect
//! of type "Adjust" on the clip (`parameters: { brightness: 20, … }`, each
//! −100…100, hue in degrees −180…180). The engine turns them into a colour
//! matrix (applied under the clip's filter); keyframed sliders are the
//! clip's `adjust.<key>` property tracks.

use serde_json::Value;

use crate::model::{Clip, EffectInstance};

pub const EFFECT_TYPE: &str = "Adjust";
/// Keyframe property ids are `adjust.<key>`.
pub const PROPERTY_PREFIX: &str = "adjust.";

#[derive(Clone, Copy, Debug)]
pub struct Control {
    pub key: &'static str,
    pub label: &'static str,
    /// The inspector slider's id (its live value while dragged).
    pub slider: &'static str,
    /// Its keyframe property id.
    pub property: &'static str,
    pub min: f64,
    pub max: f64,
}

const fn c(
    key: &'static str,
    label: &'static str,
    slider: &'static str,
    property: &'static str,
) -> Control {
    Control {
        key,
        label,
        slider,
        property,
        min: -100.,
        max: 100.,
    }
}

/// The sliders in the engine's parameter order (`adjust_color_matrix`).
pub const CONTROLS: [Control; 7] = [
    c(
        "brightness",
        "Brightness",
        "adjust:brightness",
        "adjust.brightness",
    ),
    c("contrast", "Contrast", "adjust:contrast", "adjust.contrast"),
    c(
        "saturation",
        "Saturation",
        "adjust:saturation",
        "adjust.saturation",
    ),
    Control {
        min: -180.,
        max: 180.,
        ..c("hue", "Hue", "adjust:hue", "adjust.hue")
    },
    c("exposure", "Exposure", "adjust:exposure", "adjust.exposure"),
    c(
        "temperature",
        "Temperature",
        "adjust:temperature",
        "adjust.temperature",
    ),
    c("tint", "Tint", "adjust:tint", "adjust.tint"),
];

/// The inspector's groups: what changes the colour, then the light.
pub const GROUPS: [(&str, &[&str]); 2] = [
    ("Color", &["temperature", "tint", "saturation", "hue"]),
    ("Lightness", &["exposure", "brightness", "contrast"]),
];

pub fn control(key: &str) -> Option<&'static Control> {
    CONTROLS.iter().find(|c| c.key == key)
}

/// The values in `CONTROLS` order, from a lookup per key.
pub fn values(mut f: impl FnMut(&str) -> f64) -> [f64; 7] {
    CONTROLS.map(|c| f(c.key))
}

/// A video / photo's Adjust sliders as a colour matrix, each from
/// `value` (None when they're all 0, or on other clips).
pub fn matrix(clip: &Clip, value: impl Fn(&Control) -> f64) -> Option<[f32; 20]> {
    if clip.kind != "video" && clip.kind != "image" {
        return None;
    }
    let values = values(|key| value(control(key).expect("a control")));
    crate::engine::adjust_color_matrix(&values)
}

impl Clip {
    /// The clip's Adjust effect (the first one switched on).
    pub fn adjust_effect(&self) -> Option<EffectInstance> {
        self.effects()
            .into_iter()
            .find(|e| e.enabled && e.kind == EFFECT_TYPE)
    }

    /// A slider's own (un-keyed) value: the Adjust effect's, 0 without one.
    pub fn adjust_value(&self, key: &str) -> f64 {
        self.adjust_effect()
            .and_then(|e| e.param(key))
            .unwrap_or(0.)
    }

    /// Sets a slider's own value: on the Adjust effect, or a new one
    /// holding just that slider. Rounded to whole steps, as shown.
    pub fn set_adjust_value(&mut self, key: &str, v: f64) {
        let Some(control) = control(key) else {
            return;
        };
        let v = v.round().clamp(control.min, control.max);
        let mut effects = self.effects();
        match effects
            .iter_mut()
            .find(|e| e.enabled && e.kind == EFFECT_TYPE)
        {
            Some(e) => {
                e.parameters.insert(key.into(), Value::from(v));
            }
            None => effects.push(EffectInstance::new(
                EFFECT_TYPE.into(),
                &[(key.to_string(), v)],
            )),
        }
        self.set_effects(&effects);
    }

    /// Whether Reset has anything to undo: an Adjust effect or keyframed
    /// sliders.
    pub fn has_adjust(&self) -> bool {
        self.effects().iter().any(|e| e.kind == EFFECT_TYPE)
            || self
                .animated_properties()
                .iter()
                .any(|p| p.starts_with(PROPERTY_PREFIX))
    }

    /// Reset: drops every Adjust effect and the sliders' keyframes.
    pub fn reset_adjust(&mut self) -> bool {
        if !self.has_adjust() {
            return false;
        }
        let effects: Vec<_> = self
            .effects()
            .into_iter()
            .filter(|e| e.kind != EFFECT_TYPE)
            .collect();
        self.set_effects(&effects);
        let ids: Vec<String> = CONTROLS
            .iter()
            .map(|c| format!("{PROPERTY_PREFIX}{}", c.key))
            .collect();
        self.clear_tracks(&ids);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn clip(extra: Value) -> Clip {
        let mut v = json!({
            "id": "c", "kind": "video", "timelineStart": 0, "timelineDuration": 1_000_000,
        });
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_first_slider_adds_one_adjust_effect() {
        let mut c = clip(
            json!({ "effects": [{ "id": "f", "type": "Filters:VHS", "parameters": { "intensity": 80 } }] }),
        );
        assert!(!c.has_adjust());
        c.set_adjust_value("saturation", 30.4);
        c.set_adjust_value("hue", -500.);
        let fx = c.effects();
        assert_eq!(fx.len(), 2);
        assert_eq!(fx[1].kind, EFFECT_TYPE);
        assert_eq!(c.adjust_value("saturation"), 30.);
        assert_eq!(c.adjust_value("hue"), -180.);
        assert_eq!(c.adjust_value("tint"), 0.);
        assert!(c.reset_adjust());
        assert_eq!(c.effects().len(), 1);
        assert!(!c.reset_adjust());
    }

    #[test]
    fn keyed_sliders_go_through_the_property_tracks() {
        let mut c = clip(json!({ "keyframes": [
            { "property": "transform.x", "enabled": true, "keyframes": [
                { "time": 0, "value": 0 }, { "time": 500000, "value": 0.2 } ] }
        ] }));
        c.set_properties_at(&[("adjust.contrast", 40.)], 500_000, true, 1_000);
        assert!(
            c.animated_properties()
                .contains(&"adjust.contrast".to_string())
        );
        assert!(c.has_adjust());
        assert!(c.reset_adjust());
        assert!(
            !c.animated_properties()
                .iter()
                .any(|p| p.starts_with("adjust."))
        );
        assert!(c.animated_properties().contains(&"transform.x".to_string()));
    }
}
