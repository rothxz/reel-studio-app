// A release build on Windows is a GUI program: no console window beside it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod adjust;
mod agent;
mod analytics;
mod assets;
mod audio;
mod caption_styles;
mod captions;
mod catalog;
mod clip_edits;
mod crop;
mod data_usage;
mod decoder;
mod editor;
mod effects;
mod encode;
mod engine;
mod export;
mod gizmo;
mod history;
mod i18n;
mod library;
mod model;
mod pinch;
mod prefs;
mod projects;
mod selection;
mod shortcuts;
mod speech;
mod speed_curve;
mod stickers;
mod storage;
mod text_render;
mod theme;
mod transitions;
mod ui;
mod updater;
mod visual;

use gpui::{
    App, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions, point, prelude::*, px, size,
};

use editor::*;
use shortcuts::{Shortcut, find};

/// A menu item for a shortcut: its label and action from the table, so
/// the menu shows the same key as the Shortcuts dialog.
fn item(id: Shortcut) -> MenuItem {
    MenuItem::Action {
        name: i18n::t(find(id).label).into(),
        action: id.action(),
        os_action: None,
    }
}

/// The app menu (the macOS menu bar; the ☰ menu elsewhere), in the current
/// language: built again when the language changes.
pub fn set_app_menus(cx: &mut App) {
    use Shortcut::*;
    use i18n::t;
    cx.set_menus(vec![
        Menu {
            name: t("Reel Studio").into(),
            items: vec![
                MenuItem::action(t("Settings…"), OpenSettings),
                MenuItem::action(t("Check for Updates…"), CheckForUpdates),
                MenuItem::separator(),
                MenuItem::action(t("Quit Reel Studio"), Quit),
            ],
        },
        Menu {
            name: t("File").into(),
            items: vec![
                MenuItem::action(t("Import Media…"), ImportMedia),
                MenuItem::separator(),
                MenuItem::action(t("Open Project…"), OpenProject),
                MenuItem::action(t("Save Project"), SaveProject),
                MenuItem::action(t("Save Project As…"), SaveProjectAs),
                MenuItem::separator(),
                item(Export),
            ],
        },
        Menu {
            name: t("Edit").into(),
            items: vec![
                item(Undo),
                item(Redo),
                MenuItem::separator(),
                item(CutClip),
                item(CopyClip),
                item(PasteClip),
                item(Duplicate),
                item(DeleteClip),
                MenuItem::separator(),
                item(SplitClip),
                item(DeleteLeft),
                item(DeleteRight),
                item(Deactivate),
                MenuItem::separator(),
                item(Deselect),
            ],
        },
        Menu {
            name: t("View").into(),
            items: vec![
                item(ZoomIn),
                item(ZoomOut),
                item(ZoomFit),
                MenuItem::separator(),
                item(MainMagnet),
                item(Snapping),
                item(PreviewAxis),
                MenuItem::separator(),
                item(SelectTool),
                item(SplitTool),
                MenuItem::separator(),
                item(Fullscreen),
                item(ShowShortcuts),
            ],
        },
        Menu {
            name: t("Playback").into(),
            items: vec![
                item(PlayPause),
                MenuItem::separator(),
                item(PrevFrame),
                item(NextFrame),
                item(Back1s),
                item(Forward1s),
                item(PrevEdit),
                item(NextEdit),
                MenuItem::separator(),
                item(GoStart),
                item(GoEnd),
            ],
        },
    ]);
}

fn main() {
    // The UI language before anything shows a string (Settings → Language).
    let prefs = prefs::Prefs::load();
    i18n::set_locale(i18n::resolve(prefs.language.as_deref()));
    // Settings → Storage Usage's limits, before anything reads the cache.
    storage::trim(prefs.keep_cache, prefs.cache_limit_bytes());
    decoder::init();
    text_render::warm_up();
    let app = Application::new().with_assets(ui::icons::Assets);
    // macOS keeps the app running with its window closed: the Dock icon
    // (or opening the app again) brings a new one.
    app.on_reopen(|cx| {
        if cx.windows().is_empty() {
            open_main_window(cx, false);
        }
        cx.activate(true);
    });
    app.run(|cx: &mut App| {
        if let Err(e) = cx.text_system().add_fonts(theme::embedded_fonts()) {
            eprintln!("bundled fonts didn't load: {e}");
        }
        agent::keys::load(cx);
        cx.bind_keys(shortcuts::key_bindings());
        cx.bind_keys(ui::text_input::bindings());
        cx.bind_keys([
            KeyBinding::new("secondary-i", ImportMedia, None),
            KeyBinding::new("secondary-o", OpenProject, None),
            KeyBinding::new("secondary-s", SaveProject, None),
            KeyBinding::new("secondary-shift-s", SaveProjectAs, None),
            KeyBinding::new("secondary-q", Quit, None),
            KeyBinding::new("secondary-,", OpenSettings, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());

        set_app_menus(cx);

        open_main_window(cx, true);
        // Off macOS an app without windows has nothing left to show
        // (no menu bar, no dock): closing the window quits.
        if !cfg!(target_os = "macos") {
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
        }
        cx.activate(true);
    });
}

/// The app's window: the home screen (or, at launch, a new video from the
/// files on the command line).
fn open_main_window(cx: &mut App, first: bool) {
    let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Reel Studio".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(12.), px(12.))),
            }),
            window_min_size: Some(size(px(960.), px(600.))),
            // Linux: the window's class, matching reel-studio.desktop.
            app_id: Some("reel-studio".into()),
            ..Default::default()
        },
        |window, cx| {
            let editor = cx.new(|cx| Editor::new(window, cx));
            // Closing the window saves the open project first.
            let for_close = editor.clone();
            window.on_window_should_close(cx, move |_, cx| {
                for_close.update(cx, |ed, cx| ed.save_now(cx));
                true
            });
            editor.update(cx, |ed, cx| ed.refresh_home(cx));
            // Files given on the command line: a new video from them.
            let args: Vec<String> = std::env::args().skip(1).collect();
            if first && !args.is_empty() {
                editor.update(cx, |ed, cx| ed.open_from_command_line(args, cx));
            }
            editor
        },
    )
    .unwrap();
}
