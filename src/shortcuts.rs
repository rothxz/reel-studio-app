//! The editor's keyboard shortcuts as one table: the key bindings, the menu
//! key equivalents, the tooltips ("Split  ⌘B") and the Shortcuts dialog all
//! read it. Same keys and labels as the old desktop editor.

use gpui::{Action, KeyBinding, actions};

/// Where a shortcut is listed in the Shortcuts dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Playback,
    Edit,
    Timeline,
    App,
}

impl Group {
    pub const ALL: [Group; 4] = [Group::Playback, Group::Edit, Group::Timeline, Group::App];

    pub fn label(self) -> &'static str {
        match self {
            Group::Playback => "Playback",
            Group::Edit => "Edit",
            Group::Timeline => "Timeline",
            Group::App => "General",
        }
    }
}

pub struct ShortcutDef {
    pub id: Shortcut,
    pub label: &'static str,
    /// GPUI keystrokes; `secondary` is ⌘ on a Mac, Ctrl elsewhere. The first
    /// one is what the UI shows.
    pub combos: &'static [&'static str],
    pub group: Group,
}

macro_rules! shortcuts {
    ($($id:ident),* $(,)?) => {
        actions!(shortcut, [$($id),*]);

        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Shortcut { $($id),* }

        impl Shortcut {
            pub fn action(self) -> Box<dyn Action> {
                match self { $(Shortcut::$id => Box::new($id)),* }
            }
        }
    };
}

shortcuts!(
    PlayPause,
    PrevFrame,
    NextFrame,
    Back1s,
    Forward1s,
    PrevEdit,
    NextEdit,
    GoStart,
    GoEnd,
    SplitClip,
    DeleteLeft,
    DeleteRight,
    Deactivate,
    DeleteClip,
    Duplicate,
    CopyClip,
    CutClip,
    PasteClip,
    Undo,
    Redo,
    Deselect,
    ZoomIn,
    ZoomOut,
    ZoomFit,
    PreviewAxis,
    Snapping,
    MainMagnet,
    SelectTool,
    SplitTool,
    Fullscreen,
    Export,
    ShowShortcuts,
);

use Group::*;

pub const SHORTCUTS: &[ShortcutDef] = &[
    def(Shortcut::PlayPause, "Play / pause", &["space"], Playback),
    def(Shortcut::PrevFrame, "Previous frame", &["left"], Playback),
    def(Shortcut::NextFrame, "Next frame", &["right"], Playback),
    def(Shortcut::Back1s, "Back 1 second", &["shift-left"], Playback),
    def(
        Shortcut::Forward1s,
        "Forward 1 second",
        &["shift-right"],
        Playback,
    ),
    def(Shortcut::PrevEdit, "Previous edit point", &["up"], Playback),
    def(Shortcut::NextEdit, "Next edit point", &["down"], Playback),
    def(Shortcut::GoStart, "Go to start", &["home"], Playback),
    def(Shortcut::GoEnd, "Go to end", &["end"], Playback),
    def(Shortcut::SplitClip, "Split", &["secondary-b"], Edit),
    def(Shortcut::DeleteLeft, "Delete left", &["q"], Edit),
    def(Shortcut::DeleteRight, "Delete right", &["w"], Edit),
    def(Shortcut::Deactivate, "Deactivate clip", &["v"], Edit),
    def(
        Shortcut::DeleteClip,
        "Delete",
        &["delete", "backspace"],
        Edit,
    ),
    def(Shortcut::Duplicate, "Duplicate", &["secondary-d"], Edit),
    def(Shortcut::CopyClip, "Copy", &["secondary-c"], Edit),
    def(Shortcut::CutClip, "Cut", &["secondary-x"], Edit),
    def(Shortcut::PasteClip, "Paste", &["secondary-v"], Edit),
    def(Shortcut::Undo, "Undo", &["secondary-z"], Edit),
    def(
        Shortcut::Redo,
        "Redo",
        &["secondary-shift-z", "secondary-y"],
        Edit,
    ),
    def(Shortcut::Deselect, "Deselect", &["escape"], Edit),
    def(Shortcut::ZoomIn, "Zoom in", &["secondary-="], Timeline),
    def(Shortcut::ZoomOut, "Zoom out", &["secondary--"], Timeline),
    def(Shortcut::ZoomFit, "Zoom to fit", &["shift-z"], Timeline),
    def(
        Shortcut::PreviewAxis,
        "Preview axis on/off",
        &["s"],
        Timeline,
    ),
    def(Shortcut::Snapping, "Auto snapping on/off", &["n"], Timeline),
    def(
        Shortcut::MainMagnet,
        "Main track magnet on/off",
        &["p"],
        Timeline,
    ),
    def(Shortcut::SelectTool, "Select", &["a"], Timeline),
    def(Shortcut::SplitTool, "Split tool", &["b"], Timeline),
    def(
        Shortcut::Fullscreen,
        "Full screen preview",
        &["secondary-shift-f"],
        App,
    ),
    def(Shortcut::Export, "Export", &["secondary-e"], App),
    def(
        Shortcut::ShowShortcuts,
        "Show shortcuts",
        &["secondary-/"],
        App,
    ),
];

const fn def(
    id: Shortcut,
    label: &'static str,
    combos: &'static [&'static str],
    group: Group,
) -> ShortcutDef {
    ShortcutDef {
        id,
        label,
        combos,
        group,
    }
}

/// The editor's key bindings, generated from the table.
pub fn key_bindings() -> Vec<KeyBinding> {
    SHORTCUTS
        .iter()
        .flat_map(|s| {
            s.combos.iter().map(|combo| {
                // The dialog toggle works with the dialog open, everything
                // else only while the editor itself has the keyboard.
                let context = if s.id == Shortcut::ShowShortcuts {
                    None
                } else {
                    Some("Editor")
                };
                KeyBinding::load(
                    combo,
                    s.id.action(),
                    context.map(|c| {
                        gpui::KeyBindingContextPredicate::parse(c)
                            .expect("context parses")
                            .into()
                    }),
                    false,
                    None,
                    &gpui::DummyKeyboardMapper,
                )
                .expect("shortcut table keystrokes parse")
            })
        })
        .collect()
}

pub fn find(id: Shortcut) -> &'static ShortcutDef {
    SHORTCUTS
        .iter()
        .find(|s| s.id == id)
        .expect("every shortcut is in the table")
}

const MAC: bool = cfg!(target_os = "macos");

/// One key of a combo as a key cap: "secondary" → "⌘", "left" → "←".
fn key_glyph(key: &str) -> String {
    match key {
        "secondary" => if MAC { "⌘" } else { "Ctrl" }.into(),
        "shift" => if MAC { "⇧" } else { "Shift" }.into(),
        "alt" => if MAC { "⌥" } else { "Alt" }.into(),
        "left" => "←".into(),
        "right" => "→".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "escape" => "Esc".into(),
        "backspace" => "⌫".into(),
        "delete" => if MAC { "⌦" } else { "Del" }.into(),
        "space" => "Space".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        k => k.to_uppercase(),
    }
}

/// A combo as key caps for display: "secondary-shift-z" → ["⌘", "⇧", "Z"].
pub fn combo_keys(combo: &str) -> Vec<String> {
    // "secondary--" is secondary + "-": split on the separators, not the key.
    let mut parts = vec![];
    let mut rest = combo;
    while let Some(i) = rest.find('-') {
        if i == 0 {
            break;
        }
        parts.push(&rest[..i]);
        rest = &rest[i + 1..];
    }
    parts.push(rest);
    parts.into_iter().map(key_glyph).collect()
}

/// A shortcut's first combo as one string for a tooltip: "⌘B".
pub fn hint(id: Shortcut) -> String {
    combo_keys(find(id).combos[0]).join(if MAC { "" } else { "+" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_render_as_key_caps() {
        assert_eq!(
            combo_keys("secondary--"),
            vec![key_glyph("secondary"), "-".into()]
        );
        assert_eq!(
            combo_keys("shift-left"),
            vec![key_glyph("shift"), "←".to_string()]
        );
        assert_eq!(combo_keys("secondary-/").last().unwrap(), "/");
        assert_eq!(combo_keys("q"), vec!["Q".to_string()]);
    }

    #[test]
    fn every_combo_is_bound_once() {
        let mut seen = std::collections::HashSet::new();
        for s in SHORTCUTS {
            for c in s.combos {
                assert!(seen.insert(*c), "{c} bound twice");
            }
        }
        assert_eq!(key_bindings().len(), seen.len());
    }
}
