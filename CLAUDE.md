# Reel Studio desktop (GPUI): context

This repo has two jobs:

1. **Public home of the desktop app**: `README.md`, `docs/images/`, and the
   GitHub Releases the Electron build publishes to (`main` branch).
2. **The native rewrite**: `desktop-gpui/`, a Rust app on GPUI (the UI
   framework behind the Zed editor) that will replace the Electron shell.
   Work happens on the `gpui-experiment` branch.

**Goal:** match the current desktop editor's UI and behaviour (the web
editor that Electron wraps) in native Rust, panel by panel. Track progress
in `desktop-gpui/docs/PARITY.md`: tick items off there, and add any gaps
you find.

Workspace: open `~/Project/reel-studio-desktop.code-workspace`. It holds this
repo and **`~/Project/reel-studio`**, the private app repo, which is the
reference for every screen and the home of the shared C++ engine. That repo's
own `CLAUDE.md` is its codebase map.

## Rules

- **This repo is PUBLIC.** Never copy source from `reel-studio` in here:
  no TypeScript, C++, shaders, catalogs or secrets. Read it, then write the
  Rust from scratch. The C++ engine is compiled *from the reference
  checkout* (`REEL_ENGINE_DIR`, see Build) and never vendored.
  `engine-shim/` (our own C ABI) is the only C++ that lives here.
- **Never name the apps we study** (the phone/desktop editor and the
  document scanner we model features on) in code, comments, docs, commits
  or UI strings. Describe the feature in our own words.
- `main` is the release/README branch. Don't commit app code there; work on
  `gpui-experiment` (or branches off it). Push only when the user asks.
- The old editor is the spec. Before building a panel, read its source
  (map below) and match its layout, labels, shortcuts and edge cases, not
  a guess of them.

## Build, run, verify

```sh
cd desktop-gpui
cargo dev [files…]     # = run --release --features cpp-engine (see .cargo/config.toml)
cargo t                # tests on the C++ engine backend
cargo test             # tests on the Rust stand-in (what CI without the engine can run)
cargo fmt && cargo clippy --features cpp-engine
tools/shot.sh out.png  # screenshot the app window without focusing it
```

- `.cargo/config.toml` points `REEL_ENGINE_DIR` at
  `../../reel-studio/modules/video-engine/cpp` (an exported variable wins).
  `build.rs` compiles every `cpp/src/*.cpp` + the shim with `cc`.
- Needs Homebrew `ffmpeg` (9.x, matched by `ffmpeg-next = 9`) and `pkg-config`.
- A clean build takes minutes (GPUI is big); incremental is ~10 s.
- Unlike the mobile app, building and running this app yourself is fine and
  expected: verify UI changes by launching it and taking a screenshot.
  To drive it: `cliclick c:x,y` clicks at screen points (window origin from
  `tools/winid`, image px ÷ 2 on Retina). For keys use
  `osascript -e 'tell application "System Events" to key code 49'` (Space)
  or `keystroke "s"`; cliclick's `kp:space` does **not** reach GPUI.

## Architecture (current)

```
src/main.rs        window, fonts, app menu (built from the shortcut table)
src/editor.rs      Editor state: document, edits (+ magnet), undo, playback,
                   preview, drags, panel layout, projects session, autosave
src/shortcuts.rs   the shortcut table → key bindings, menu, tooltips, dialog
src/history.rs     undo / redo (document snapshots, coalesced per gesture)
src/model.rs       Project document (serde): same JSON as the app; unknown fields
                   kept in `extra`; clip edits + `with_magnet`
src/projects.rs    projects folder: list / load / save (atomic) / delete
src/prefs.rs       panel sizes + timeline switches, data folder
src/engine.rs      Engine: C++ core via FFI (feature cpp-engine) or Rust stand-in
src/decoder.rs     FFmpeg probe, thumbnails, one decode thread per source
src/theme.rs       palette, fonts (bundled Nunito + IBM Plex Mono)
src/ui/            one file per panel, each `impl Editor { fn render_… }`:
                   header, home, browser, player, inspector, toolbar,
                   timeline, dialogs (popup, confirm, shortcuts), widgets,
                   icons (icon fonts served as SVG)
engine-shim/       C ABI over videoengine::Engine (+ FindActiveClips etc.)
assets/fonts/      bundled fonts + licences
```

Data flow mirrors the app's: **the `Project` document is the source of
truth** (Rust state in the Editor, later its own model/store module). Every
edit replaces the document and pushes it to the engine (`Editor::commit` →
`engine.load`), like the app's store → `VideoEngine.updateProject`. The
engine owns playback state (clock, play/pause, seek) and answers "what is
visible at t" with source times resolved (speed, curves, reverse, freeze),
never project data. Preview frames come from `decoder.rs` threads, requested
latest-wins, delivered over an async channel to the UI.

Every edit goes through `Editor::edit(label, magnet, coalesce, f)`: it
snapshots the document for undo, applies `f`, runs the main-track magnet
when asked, marks the project dirty (autosave in 1.5 s) and commits. Next
split as it grows: `src/model/` (document + edits), `src/media/` (decode,
thumbnails, waveform, audio out), and the editor state out of `editor.rs`.

## Porting map: old editor → source to read in `~/Project/reel-studio`

| Part | Read first |
| --- | --- |
| Whole layout, panel sizes, gutters | `src/features/editor/layout/EditorDesktopLayout.tsx`, `desktop/desktopPanelStore.ts` (browser 260–640, inspector 260–560 default 340, timeline ≥180 default 40% of height, gutter 6, header 40, player ≥320 wide) |
| Header (close, Saved·Local, project name ▾, BETA, AI agent, Shortcuts, resolution ▾, Export) | `desktop/DesktopHeader.tsx` |
| Left browser (Media, Audio, Text, Sticker, Effects, Transitions, Captions, Filters) | `desktop/DesktopBrowser.tsx`, `browserViews.tsx`, `BrowserTiles.tsx`, `DesktopMediaLibrary.tsx` |
| Player + transport (HH:MM:SS:FF, ⏮ ◀ ▶ ▶ ⏭, ratio, fullscreen) | `desktop/DesktopTransport.tsx`, `src/features/preview/` |
| Inspector tabs per clip kind | `desktop/inspectorTabs.ts` (the table), `DesktopInspector.tsx`, `Desktop*Panel.tsx`, `DesktopProjectDetails.tsx` (nothing selected) |
| Timeline toolbar | `desktop/DesktopTimelineToolbar.tsx` (clip actions = `useClipTools` minus `inspectorToolKeys`) |
| Timeline (ruler, lanes, filmstrip, waveform, seams, magnet, snapping, drag/trim) | `src/features/timeline/` (`Timeline.tsx`, `TimelineLanes.tsx`, `TimelineClip.tsx`, `desktopTimelineEdits.ts`, `useDesktopTimelineMouse.ts`, `DesktopLaneControls.tsx`, `transitionSeams.ts`, `clipTrim.ts`, `timelineZoom.ts`) |
| Context menus | `desktop/DesktopContextMenu.tsx`, `DesktopTimelineContextMenu.tsx`, `timeline/timelineContextMenu.ts` |
| Shortcuts | `desktop/desktopShortcuts.ts` (data), `useDesktopShortcuts.ts`, `DesktopShortcutsDialog.tsx` |
| Edits + undo | `src/stores/commands/*` (one command class per edit), `historyStore` |
| Document types | `src/types/project.ts` (+ `keyframes`, `cutout`, `media` types) |
| Clip time math | engine `Timeline.h` (use it through the shim, don't re-implement) |
| Export | engine `Engine::StartExport`; web reference `modules/video-engine/src/web/exportJob.ts`; native `ios/VideoToolboxEncoder.mm`, `MetalCompositor.mm` |
| AI agent panel | `src/features/agent/` (tools, providers, snapshot) |
| Strings | `src/i18n/locales/en.ts` (labels: reuse the exact English; 5 locales later) |
| Colours, spacing, type | `src/theme/palettes.ts` (dark), `tokens.ts` (spacing 4/8/12/16/20/24; Nunito + IBM Plex Mono) |
| Electron shell behaviour (title bar, updates, LLM proxy, key vault) | `desktop/main.mjs`, `desktop/README.md` |

## Design notes

- Editor chrome is always dark, black/white, monochrome icons; no yellow
  accent in the editor (the app's accent is for the shell, not tools).
- Old desktop palette: card `#1d1d1f`, card border `rgba(255,255,255,.06)`,
  text `#f2f2f2` / secondary 66 % / tertiary 42 % white, danger `#ff5c5c`,
  success `#2fbf71`. Labels/metadata/timecodes are small uppercase mono.
- Shortcuts are data in the old app; keep them as one table here too
  (`KeyBinding`s generated from it, and the Shortcuts dialog reads it).

## GPUI notes (gpui 0.2.2, learned the hard way)

- `window.request_animation_frame()` may only run **during render**: it
  panics (`current_view()` unwrap) from an action/click handler. Start
  playback by setting state + `cx.notify()`; `render` keeps requesting
  frames while the engine plays.
- An `img()` must be `.absolute().inset_0()` inside a `.relative()
  .overflow_hidden()` box, or its intrinsic size drives the layout.
- Video frames: `RenderImage` wants BGRA bytes inside an `image::Frame`.
  Each new frame is a new atlas entry: `window.drop_image(old)` the
  replaced one on the next render (see `stale_frames`).
- Mouse positions are window coordinates. To map x → time, record the
  element's bounds with a `canvas(prepaint…)` child into an
  `Rc<Cell<Bounds>>`.
- Key bindings with a context (`Some("Editor")`) need the focused element
  inside `.key_context("Editor").track_focus(&focus)`. The root re-focuses
  on mouse down so shortcuts survive clicks.
- GPUI's macOS text system **skips any font without an `m` glyph**, so
  icon fonts never load as fonts. `ui/icons.rs` reads the glyph outlines
  (ttf-parser) and serves them as SVG assets for `svg()`, which tints them
  with `text_color`. Add an icon: its codepoint from the font's glyph map.
- Mouse drags (splitters, sliders, scrub) are `Editor::drag`: the element
  starts it on mouse down, the window root handles moves and the release,
  so a drag keeps working off the element. Elements that need their bounds
  (sliders, lanes, player area) record them with `Editor::measure(key)`.
- Driving the app: cliclick `m:` sends plain moves (no button held), which
  correctly ends a drag; use `dd:` / `dm:` / `du:` for drags, `rc:` for a
  right click. `tools/shot.sh` captures the widest window of the process.
- No crate copy of `gpui-component` yet (none offline, and the `gpui-kit`
  crates in the registry target another gpui); it (0.9) is the planned widget set
  (inputs, sliders, select, tabs, dialogs, scroll areas). Check its API
  matches gpui 0.2.2 before adding it.

## Engine notes

- `Engine::Seek` always lands **paused** and quantizes to the frame;
  `LoadProject` resets to 0, paused. The editor re-seeks after every load.
- Clip times: `ClipOffsetToSourceTime` (speed, speed curve, reverse,
  freeze). A clip plays `[sourceStart, sourceStart + timelineDuration ×
  speed]`; `sourceDuration` is the media's length, not the segment.
- The engine has no desktop decoder/compositor/encoder yet; export needs
  them (macOS: the iOS AVFoundation/VideoToolbox/Metal files are nearly
  portable; only `AVFoundationDecoder.mm` and `VideoEngineBridge.mm` use
  UIKit).
- Old web projects live in IndexedDB with `blob:` media; here projects are
  JSON files with absolute media paths. Opening a web-exported project
  will need a media relink step.
