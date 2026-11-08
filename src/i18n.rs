//! The UI language: English, Khmer, Vietnamese, Simplified Chinese and
//! Indonesian, as in the old editor (Settings → Language, "Same as device"
//! until the user picks one).
//!
//! The dictionaries are the app checkout's (build.rs flattens them into
//! `OUT_DIR/locales.json`; none without a checkout, and the app is then
//! English only). Our UI strings are written in English, the old editor's
//! exact English, so `t("Settings")` finds the key whose English text that
//! is and returns the current language's text for it: a string with no
//! entry stays English. `tf` fills `{name}` placeholders the same way;
//! `tk` looks a key up directly where the English alone is ambiguous.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{LazyLock, Mutex};

use gpui::SharedString;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Locale {
    En,
    Km,
    Vi,
    Zh,
    Id,
}

impl Locale {
    const ALL: [Locale; 5] = [Locale::En, Locale::Km, Locale::Vi, Locale::Zh, Locale::Id];

    /// The language subtag it's stored and looked up under.
    pub fn code(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Km => "km",
            Locale::Vi => "vi",
            Locale::Zh => "zh",
            Locale::Id => "id",
        }
    }

    pub fn from_code(code: &str) -> Option<Locale> {
        Locale::ALL.into_iter().find(|l| l.code() == code)
    }

    /// The language named in its own language (the Settings list).
    pub fn native_name(self) -> &'static str {
        match self {
            Locale::En => "English",
            Locale::Km => "ភាសាខ្មែរ",
            Locale::Vi => "Tiếng Việt",
            Locale::Zh => "简体中文",
            Locale::Id => "Bahasa Indonesia",
        }
    }
}

/// The languages offered in Settings, in the old editor's order.
pub const LANGUAGES: [Locale; 5] = [Locale::Km, Locale::En, Locale::Vi, Locale::Zh, Locale::Id];

/// The languages this build has strings for: English, plus every language
/// the dictionaries were compiled in for.
pub fn available() -> Vec<Locale> {
    LANGUAGES
        .into_iter()
        .filter(|l| *l == Locale::En || DICTS.text.contains_key(l))
        .collect()
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// The language shown now.
pub fn locale() -> Locale {
    Locale::ALL[CURRENT.load(Ordering::Relaxed) as usize]
}

/// Switches the language every later `t` call answers in.
pub fn set_locale(locale: Locale) {
    let index = Locale::ALL.iter().position(|l| *l == locale).unwrap_or(0);
    CURRENT.store(index as u8, Ordering::Relaxed);
}

/// What a saved preference (`None` = "Same as device") resolves to.
pub fn resolve(preference: Option<&str>) -> Locale {
    preference
        .and_then(Locale::from_code)
        .unwrap_or_else(system_locale)
}

/// The system's preferred languages mapped to one we ship: the first one
/// we have wins, so ["fr-FR", "km-KH"] still gets Khmer; else English.
pub fn system_locale() -> Locale {
    pick_supported(sys_locale::get_locales())
}

fn pick_supported(tags: impl IntoIterator<Item = String>) -> Locale {
    for tag in tags {
        let code = tag
            .split(['-', '_'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        // Every Chinese tag gets Simplified Chinese, the only one we ship;
        // "in" is the old ISO code for Indonesian.
        let code = if code == "in" { "id".into() } else { code };
        if let Some(locale) = Locale::from_code(&code).filter(|l| available().contains(l)) {
            return locale;
        }
    }
    Locale::En
}

struct Dicts {
    /// Each language's key → text.
    text: HashMap<Locale, HashMap<&'static str, &'static str>>,
    /// English text → the key it's found under (exact, and lowercased).
    by_english: HashMap<&'static str, &'static str>,
    by_english_lower: HashMap<String, &'static str>,
}

/// Namespaces whose strings win when one English text has several keys:
/// the desktop editor's own first.
const PREFERRED: [&str; 8] = [
    "desktop.",
    "shortcuts.",
    "editor.",
    "timeline.",
    "tools.",
    "clip.",
    "settings.",
    "common.",
];

static DICTS: LazyLock<Dicts> =
    LazyLock::new(|| load(include_str!(concat!(env!("OUT_DIR"), "/locales.json"))));

fn load(json: &str) -> Dicts {
    let raw: HashMap<String, HashMap<String, String>> =
        serde_json::from_str(json).unwrap_or_default();
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let mut text: HashMap<Locale, HashMap<&'static str, &'static str>> = HashMap::new();
    for (code, entries) in raw {
        let Some(locale) = Locale::from_code(&code) else {
            continue;
        };
        let entries = entries
            .into_iter()
            .map(|(k, v)| (leak(k), leak(v)))
            .collect();
        text.insert(locale, entries);
    }
    let rank = |key: &str| {
        let ns = PREFERRED.iter().position(|p| key.starts_with(p));
        (ns.unwrap_or(PREFERRED.len()), key.to_string())
    };
    let mut by_english: HashMap<&'static str, &'static str> = HashMap::new();
    let mut by_english_lower: HashMap<String, &'static str> = HashMap::new();
    if let Some(en) = text.get(&Locale::En) {
        for (&key, &english) in en {
            let better = |old: &&'static str| rank(key) < rank(old);
            if by_english.get(english).is_none_or(better) {
                by_english.insert(english, key);
            }
            let lower = english.to_lowercase();
            if by_english_lower.get(&lower).is_none_or(better) {
                by_english_lower.insert(lower, key);
            }
        }
    }
    Dicts {
        text,
        by_english,
        by_english_lower,
    }
}

/// Uppercased translations, made once each (a label shown in capitals,
/// like "UPDATES", whose dictionary text isn't).
static UPPER: LazyLock<Mutex<HashMap<&'static str, &'static str>>> =
    LazyLock::new(Default::default);

/// `english` in the current language: the text of the dictionary key whose
/// English it is (the same words in other capitals count, and an all-caps
/// label stays all caps), else `english` itself.
pub fn t(english: &'static str) -> &'static str {
    translate(locale(), english).unwrap_or(english)
}

fn translate(locale: Locale, english: &str) -> Option<&'static str> {
    if locale == Locale::En || english.is_empty() {
        return None;
    }
    let dicts = &*DICTS;
    let text = dicts.text.get(&locale)?;
    if let Some(key) = dicts.by_english.get(english) {
        return text.get(key).copied();
    }
    let key = dicts.by_english_lower.get(&english.to_lowercase())?;
    let found = *text.get(key)?;
    let all_caps =
        english.chars().any(char::is_alphabetic) && !english.chars().any(char::is_lowercase);
    if !all_caps {
        return Some(found);
    }
    let mut upper = UPPER.lock().unwrap();
    Some(
        upper
            .entry(found)
            .or_insert_with(|| Box::leak(found.to_uppercase().into_boxed_str())),
    )
}

/// `t` for any string, owned or not (the shared widgets' labels, toasts,
/// popup rows): its translation, else the string itself.
pub fn ts(text: impl Into<SharedString>) -> SharedString {
    let text = text.into();
    match translate(locale(), &text) {
        Some(found) => found.into(),
        None => text,
    }
}

/// A dictionary key's text in the current language (English when the
/// language lacks it), `fallback` when no dictionary has it (a build
/// without the app checkout), then its placeholders filled.
pub fn tk(key: &str, fallback: &str, params: &[(&str, &dyn Display)]) -> String {
    let dicts = &*DICTS;
    let get = |l: Locale| dicts.text.get(&l).and_then(|t| t.get(key)).copied();
    let template = get(locale())
        .or_else(|| get(Locale::En))
        .unwrap_or(fallback);
    interpolate(template, params)
}

/// A count's string from a plural pair of keys (`<base>.one` for 1, else
/// `<base>.other`), `{count}` filled; `one` / `other` are the English for
/// a build without dictionaries.
pub fn plural(base: &str, count: usize, one: &str, other: &str) -> String {
    let (suffix, fallback) = if count == 1 {
        ("one", one)
    } else {
        ("other", other)
    };
    tk(&format!("{base}.{suffix}"), fallback, &[("count", &count)])
}

/// `t` of an English template, then its `{name}` placeholders filled:
/// `tf("Version {version} is available", &[("version", &v)])`.
pub fn tf(template: &'static str, params: &[(&str, &dyn Display)]) -> String {
    interpolate(t(template), params)
}

/// Fills `{name}` placeholders; an unknown one is left as written, so a
/// typo shows instead of going blank.
pub fn interpolate(template: &str, params: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after.find('}');
        let value = close.and_then(|c| {
            let name = &after[..c];
            params.iter().find(|(n, _)| *n == name).map(|(_, v)| v)
        });
        match (close, value) {
            (Some(c), Some(v)) => {
                out.push_str(&v.to_string());
                rest = &after[c + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_placeholders() {
        let s = interpolate(
            "Version {v} of {name}, {missing}",
            &[("v", &3), ("name", &"x")],
        );
        assert_eq!(s, "Version 3 of x, {missing}");
        assert_eq!(interpolate("a { b", &[]), "a { b");
    }

    #[test]
    fn picks_the_first_supported_system_language() {
        let tags = |t: &[&str]| t.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(pick_supported(tags(&["fr-FR"])), Locale::En);
        if available().contains(&Locale::Km) {
            assert_eq!(pick_supported(tags(&["fr-FR", "km-KH"])), Locale::Km);
            assert_eq!(pick_supported(tags(&["zh-Hant-TW"])), Locale::Zh);
            assert_eq!(pick_supported(tags(&["in_ID"])), Locale::Id);
        }
    }

    #[test]
    fn looks_up_by_english() {
        let d = load(
            r#"{"en":{"settings.title":"Settings","settings.groups.about":"About",
                      "foo.title":"Settings"},
                "vi":{"settings.title":"Cài đặt","settings.groups.about":"Giới thiệu",
                      "foo.title":"Khác"}}"#,
        );
        assert_eq!(d.by_english["Settings"], "settings.title");
        assert_eq!(d.by_english_lower["about"], "settings.groups.about");
    }

    /// With the app checkout: every language came through build.rs whole
    /// (same keys as English), and a lookup by English text finds it,
    /// all caps kept.
    #[test]
    fn compiled_dictionaries_are_whole() {
        let Some(en) = DICTS.text.get(&Locale::En) else {
            return; // built without the app checkout: English only
        };
        assert!(en.len() > 1000, "only {} English strings", en.len());
        for locale in available() {
            let text = &DICTS.text[&locale];
            let missing: Vec<_> = en
                .keys()
                .filter(|k| !text.contains_key(*k))
                .take(5)
                .collect();
            assert!(missing.is_empty(), "{locale:?} lacks {missing:?}");
        }
        let settings = DICTS.text[&Locale::Vi]["settings.title"];
        assert_eq!(translate(Locale::Vi, "Settings"), Some(settings));
        assert_eq!(
            translate(Locale::Vi, "SETTINGS"),
            Some(&*settings.to_uppercase())
        );
    }

    #[test]
    fn english_is_untouched() {
        assert_eq!(translate(Locale::En, "Settings"), None);
        assert_eq!(t("No such string anywhere"), "No such string anywhere");
    }
}
