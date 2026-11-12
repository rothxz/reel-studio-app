# Reel Studio desktop: context

This repo is the **desktop app**: a native Rust reimplementation of the
Reel Studio desktop editor on GPUI (the UI framework behind the Zed
editor), replacing the Electron build. The Cargo crate sits at the repo
root next to the public `README.md` / `docs/images/` and the GitHub
Releases (the Electron build publishes there until the Rust app ships).
Its write-up is `docs/GPUI.md`.

**Goal:** match the current desktop editor's UI and behaviour (the web
editor that Electron wraps) in native Rust, panel by panel. Track progress
in `docs/PARITY.md`: tick items off there, and add any gaps you find.

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
- The app lives on `main`. Work on branches off `main`; commit or push
  only when the user asks.
- The old editor is the spec. Before building a panel, read its source
  (map below) and match its layout, labels, shortcuts and edge cases, not
  a guess of them.

## Build, run, verify

```sh
cargo dev [files…]     # = run --release --features cpp-engine (see .cargo/config.toml)
cargo t                # tests on the C++ engine backend
cargo test             # tests on the Rust stand-in (what CI without the engine can run)
cargo fmt && cargo clippy --features cpp-engine
tools/shot.sh out.png  # screenshot the app window without focusing it
```

- `.cargo/config.toml` points `REEL_ENGINE_DIR` at
  `../reel-studio/modules/video-engine/cpp` (an exported variable wins).
  `build.rs` compiles every `cpp/src/*.cpp` + the shim with `cc`.
- It also points `REEL_APP_DIR` at `../reel-studio`: `build.rs` reads the
  app's public client config from its `.env` (asset host, catalog database
  URL + publishable key → `REEL_ASSETS_URL` / `REEL_CATALOG_URL` /
  `REEL_CATALOG_KEY`, also overridable at run time) and the filter catalog
  + preview manifest into `OUT_DIR`. Compiled in, never committed; without
  a checkout the app builds with no asset host, effects or filters.
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
src/i18n.rs        UI language: the app's dictionaries (compiled in by
                   build.rs), `t("English")` / `tf` / `ts` look up by
                   English text, `tk` / `plural` by key; Settings →
                   Language, "Same as device"
src/shortcuts.rs   the shortcut table → key bindings, menu, tooltips, dialog
src/pinch.rs       trackpad pinch → timeline zoom (AppKit magnify
                   monitor; gpui drops the event)
src/analytics.rs   anonymous usage events → PostHog (the phone app's
                   project and event names): batching thread, privacy
                   filter, events derived from each edit; release
                   builds only, Settings switch (docs/ANALYTICS.md)
src/history.rs     undo / redo (document snapshots, coalesced per gesture)
src/model.rs       Project document (serde): same JSON as the app; unknown fields
                   kept in `extra`; clip edits + `with_magnet`
src/clip_edits.rs the toolbar's one-shot clip edits on the document:
                   freeze frame, extract audio, replace media, crop,
                   arrange layers
src/crop.rs        crop math (box, handles, ratio fit, straighten cover)
src/projects.rs    projects folder: list / load / save (atomic) / delete
src/prefs.rs       panel sizes + timeline switches, data folder
src/storage.rs     what the data folder holds by category (Settings → Data
                   and Storage): measure, clear the cache, launch trim
src/data_usage.rs  bytes the app's features downloaded, per kind
src/engine.rs      Engine: C++ core via FFI (feature cpp-engine) or Rust stand-in
src/decoder.rs     FFmpeg probe, thumbnails, one decode thread per source,
                   filmstrip tile thread, waveform peaks
src/assets.rs      the asset host (R2): URLs, disk cache, the tile picture
                   workers (download + decode, first frame / animation)
src/catalog.rs     served video effects (catalog database REST, paged per
                   tab, disk copy) + the compiled-in filter list
src/library.rs     online libraries: stickers / GIFs + music through the
                   media proxy, sound effects (table + asset host),
                   downloads kept in <data>/library/
src/stickers.rs    sticker / GIF frames for the player (decode, loop)
src/text_render.rs text clips drawn as pictures (cosmic-text + tiny-skia)
src/audio.rs       sound out (cpal): timeline mixer in step with the
                   engine (`TimelineMix`, also run offline by export),
                   library previews
src/beats/         an audio clip's beat map (`clip.beats`, the app's JSON:
                   source-time markers, cached analysis, modes, edits),
                   analysis (FFmpeg decode → the engine's analyzer via the
                   shim; `standin.rs` without the engine)
src/encode.rs      MP4 writer: H.264 (VideoToolbox / Media Foundation /
                   x264 / openh264, MPEG-4 last) + AAC (export, agent)
src/export.rs      export thread: frame-exact layers → GPU compositor →
                   encoder, timeline sound; `.part` renamed when whole
src/agent/         the AI agent: providers, keys (keychain), llm (Ollama /
                   OpenAI-style / Anthropic dialects, vision), snapshot
                   (the project as the model reads it, short ids), skills
                   (a job's know-how + extra tools per request: routed
                   by its words or `use_skill`; <data>/skills/*.md), tools
                   (edit, motion: titles + animate, sound: find / add /
                   sound_design, produce: voiceover, captions, music,
                   stickers, drawn scenes, make_video; board: a video
                   written scene by scene, start_video / write_scene /
                   build_video, any length; check: check_video, the
                   timeline's faults each with its fix; look), sfx (app
                   library + Openverse, onset / peak analysis), voice
                   (text to speech: OpenAI / Gemini key, else `say`),
                   captions (timed to the voice's pauses), graphics
                   (motion-graphic scenes drawn with tiny-skia; canvas:
                   a scene composed from placed, timed, animated
                   elements), movie
                   (frames → H.264 MP4), eval (real-model scoring)
src/face_swap/     AI face swap on this computer: model registry (pinned
                   URLs, SHA-256, licences, download), ONNX Runtime
                   sessions (`ort`; Core ML / DirectML / CUDA, CPU
                   fallback), runtime (ONNX Runtime linked in on Apple
                   silicon / Windows, loaded on Linux and Intel Macs from
                   the library the release ships: `cfg(ort_dynamic)`),
                   detector (YuNet), five-point align,
                   embedder (ArcFace), swapper trait (inswapper 128),
                   restorer (GFPGAN), mask / colour match / paste,
                   tracker (identity checks, One-Euro landmarks), the
                   clip job (reader thread → bounded queue), the result
                   cache (per-frame crop + transform) pasted by the
                   player (`Editor::face_swap_frames`) and export;
                   models in <data>/models/face-swap (`REEL_FACE_MODELS`
                   overrides), end-to-end tests in `tests.rs` (ignored)
src/effects/       video effects + filters in the preview: shader packages
                   (download, hash check, cache), the passes at a time
                   (stack), Metal source (msl: the effect dialect →
                   MSL, our fx_/fl_ helpers), the Metal renderer and its
                   render thread
src/theme.rs       palette, fonts (bundled Nunito + IBM Plex Mono; Kantumruy
                   Pro for all UI text in Khmer: `font_body()` /
                   `font_mono()`, `line_height`)
src/ui/            one file per panel, each `impl Editor { fn render_… }`:
                   header, home, browser, fx (effect / filter grids, the
                   effect clip's settings, Adjust sliders, their edits),
                   library (Audio / Text / Sticker views), text_input (the
                   text field), text_panel, audio_panel, beats (Audio ›
                   Beats), audio_duration (Audio › Duration),
                   player, inspector, toolbar, clip_tools (the
                   toolbar's Replace / Freeze / Extract audio / Overlay),
                   crop (the Crop dialog), layers (Arrange layers),
                   timeline (lane layout + drawing), timeline_edit (clip
                   drag / trim / snapping, drops, wheel, right-click menus),
                   dialogs (popup, confirm, shortcuts), face_swap
                   (Visual › Face Swap), settings +
                   storage (its Data and Storage pages), export (save
                   dialog + the export screen), agent (the AI
                   agent panel + its turn loop), widgets,
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
| Strings | `src/i18n/locales/en.ts` (labels: reuse the exact English, which is also how `src/i18n.rs` finds the translation) |
| Colours, spacing, type | `src/theme/palettes.ts` (dark), `tokens.ts` (spacing 4/8/12/16/20/24; Nunito + IBM Plex Mono) |
| Electron shell behaviour (title bar, updates, LLM proxy, key vault) | `desktop/main.mjs`, `desktop/README.md` |

## Design notes

- **The browser picks, the inspector sets.** Looks and assets (effects,
  filters, stickers, text templates, caption styles…) are picked in the
  left browser only. The right inspector holds the selected clip's
  settings (sliders, toggles, values), never a second catalog grid; a
  "Replace" there sends you back to the browser tab.
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
- Export doesn't use the engine's `StartExport` (it has no desktop
  backends): `src/export.rs` asks the engine for layers at each frame time
  and draws them with our own decoder, GPU compositor and FFmpeg encoder.
  Anything the player learns to draw (transitions, drawings…) must be
  drawn there too: both build on `effects::stack` and
  `engine::picture_layers`.
- Effects and filters are data: a package's `body` is GLSL-flavoured
  (`vecN`, `S(uv)`, `MOD(`, `ATAN2(`); `effects/msl.rs` rewrites it to
  Metal. Write the `fx_` / `fl_` helpers ourselves (never copy the app's
  preludes); `every_hosted_package_compiles` (ignored, needs the asset
  host) proves each hosted package still compiles.
- Old web projects live in IndexedDB with `blob:` media; here projects are
  JSON files with absolute media paths. Opening a web-exported project
  will need a media relink step.
