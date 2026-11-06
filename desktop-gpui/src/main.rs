mod decoder;
mod editor;
mod engine;
mod model;
mod theme;

use gpui::{
    App, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions, point, prelude::*, px, size,
};

use editor::*;

fn main() {
    decoder::init();
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("space", TogglePlay, Some("Editor")),
            KeyBinding::new("s", SplitClip, Some("Editor")),
            KeyBinding::new("backspace", DeleteClip, Some("Editor")),
            KeyBinding::new("delete", DeleteClip, Some("Editor")),
            KeyBinding::new("left", StepBack, Some("Editor")),
            KeyBinding::new("right", StepForward, Some("Editor")),
            KeyBinding::new("home", GoToStart, Some("Editor")),
            KeyBinding::new("cmd-=", ZoomIn, Some("Editor")),
            KeyBinding::new("cmd--", ZoomOut, Some("Editor")),
            KeyBinding::new("cmd-i", ImportMedia, None),
            KeyBinding::new("cmd-o", OpenProject, None),
            KeyBinding::new("cmd-s", SaveProject, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus(vec![
            Menu {
                name: "Reel Studio".into(),
                items: vec![MenuItem::action("Quit", Quit)],
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("Import Media…", ImportMedia),
                    MenuItem::action("Open Project…", OpenProject),
                    MenuItem::action("Save Project…", SaveProject),
                ],
            },
        ]);

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
                ..Default::default()
            },
            |window, cx| {
                let editor = cx.new(|cx| Editor::new(window, cx));
                // Files given on the command line land in the library.
                let args: Vec<String> = std::env::args().skip(1).collect();
                if !args.is_empty() {
                    editor.update(cx, |ed, cx| ed.open_paths(args, cx));
                }
                editor
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
