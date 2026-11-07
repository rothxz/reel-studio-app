# Reel Studio desktop on GPUI

The desktop editor, reimplemented as a native Rust app on
[GPUI](https://www.gpui.rs) (the UI framework behind the Zed editor), to
replace the Electron build. Until it ships, the `v…` releases on this repo
(marked Latest) are still the Electron build; the native app publishes
`native-v…` pre-releases next to them (see Releases below).

![Media library, player, inspector and timeline](screenshot.jpg)

## What it does

- Opens on a **projects home** (greeting, New video, Recent list). New video
  takes files from a dialog or a drop; projects are JSON files in the app's
  data folder, saved shortly after each edit, on close and on quit.
- The editor follows the old desktop editor's layout and look: header (✕,
  save state, name, Beta, Shortcuts, resolution, Export), browser | player
  | inspector over the timeline toolbar and timeline, with draggable,
  remembered gutters. Same palette, Nunito / IBM Plex Mono, and the same
  icon glyphs.
- Browser: the eight tabs, with a Media library of thumbnail tiles (Added
  badge, length); a click inserts the file at the playhead.
- Player: the project canvas fitted in the panel, HH:MM:SS:FF timecode,
  go to start / frame back / play / frame forward / go to end, ratio menu,
  full-screen preview. Frames are decoded in-process (ffmpeg-next), one
  thread per source, latest request wins; rotation flags are applied.
- Inspector: project Details, or the selected clip's tabs per kind (Speed ›
  Normal is the first working panel).
- Timeline toolbar and timeline: undo / redo, Select / Split tool, delete
  left / right, split / delete / duplicate, main track magnet + linkage,
  zoom slider and fit; lanes with lock / eye / mute, ruler, playhead.
- Every shortcut of the old editor (one table drives keys, menus, tooltips
  and the Shortcuts dialog, ⌘/), and undo for every edit.
- The document is the **same JSON the app uses**; fields not edited here
  (keyframes, effects, masks…) are kept and written back untouched.
- Runs on the **shared C++ engine** (`--features cpp-engine`) through a
  small C shim (`engine-shim/reel_engine.cpp`): playback clock, which clips
  are visible and their source times (speed curves, reverse, freeze). A
  Rust stand-in keeps the crate buildable from this repo alone.

`docs/PARITY.md` lists what is done and what isn't, item by item.

## Run it

```sh
brew install ffmpeg pkg-config        # FFmpeg 9 (ffmpeg-next 9 matches it)
cargo run --release -- ~/Movies/a.mp4 ~/Movies/b.mov

# With the shared C++ engine. Its source lives in the private app repo and
# is not copied here: point at that checkout's engine folder.
REEL_ENGINE_DIR=/path/to/reel-studio/modules/video-engine/cpp \
  cargo run --release --features cpp-engine

cargo test                            # model + engine tests (either backend)

# In the ~/Project workspace layout (reel-studio next to reel-studio-app),
# .cargo/config.toml already points REEL_ENGINE_DIR there:
cargo dev ~/Movies/a.mp4              # run --release --features cpp-engine
cargo t                               # tests on the C++ engine
```

Working on it: `/CLAUDE.md` is the context (rules, architecture, where each
part of the old editor lives), and `docs/PARITY.md` tracks what's done.

Keys are the old editor's (⌘/ lists them): Space play · ⌘B split · ⌫
delete · Q / W delete left / right · ⌘Z / ⌘⇧Z undo / redo · ←/→ frame ·
↑/↓ edit point · ⌘= / ⌘- / ⇧Z zoom · ⌘I import · ⌘O import a project JSON
· ⌘⇧S save a copy.

## What we measured (M-series Mac, macOS 27, release build)

| | Rust app (GPUI) | Electron app today |
| --- | --- | --- |
| App size | 9.1 MB binary (+ FFmpeg dylibs, ~15–25 MB if bundled trimmed) | 284 MB `.app` |
| Memory, 2 clips open, after playback | ~180–250 MB RSS | (not measured here) |
| Playback, 1080p + 720p H.264 across a cut | 30 fps, ~35–60 % of one core | n/a |

The CPU figure is **software** decode plus a CPU colour conversion. FFmpeg's
VideoToolbox hwaccel, and handing GPUI the hardware frames directly, are the
obvious next steps and should cut it sharply.

## Verdict

**Feasible, and the native engine fits well.** GPUI builds from crates.io
(`gpui 0.2.2`), plays 30 fps footage in real time, and pulls in the C++ core with a plain
`cc` build: no React Native, no web view. The pieces that make this editor
hard (playback clock, timeline math, the project document) moved over with
almost no new code because they were already engine-side or plain data.

**The cost is the UI and the preview, not the engine.** The editor in the main
repo is ~190k lines of TypeScript/React. Nothing of it runs in GPUI; every
panel, sheet and gesture is a rewrite in Rust. The biggest items:

| Area | Today (web/Electron) | In a GPUI app |
| --- | --- | --- |
| Preview compositor (effects, filters, masks, transitions, text) | CanvasKit + SkSL runtime shaders | Reuse the native Metal compositor (`ios/MetalCompositor.mm`, no UIKit) on macOS; Windows/Linux need a GL/wgpu port (the Android GLES shaders are a start). Effects packages already compile to Metal/GLSL. |
| Export | WebCodecs + mediabunny (TS port of the engine) | The C++ `Engine::RunExportJob` with desktop decoder/encoder. On macOS `AVFoundationDecoder`/`VideoToolboxEncoder` are nearly portable (only 2 of 12 iOS `.mm` files touch UIKit); elsewhere FFmpeg. |
| Text, captions, text animations | Skia Paragraph + shared `draw.ts` | GPUI text is fine for UI, but the canvas text needs Skia (`skia-safe`) or a port of `draw.ts`. |
| Audio playback + waveform | Web Audio | `cpal` + the engine's `AudioMixer` |
| AI tools (captions, Remove BG, …) | Transformers.js workers | `ort` / `candle`, or the native models the phone apps already use |
| Widgets (inputs, sliders, menus, dialogs) | React components | `gpui-component` (0.9) covers most; GPUI is still pre-1.0 and its API moves. |
| Languages incl. Khmer | i18n + Kantumruy Pro | Bundle fonts; GPUI shapes text with Core Text / DirectWrite, so Khmer should work but is unverified. |
| Auto-update, installers | electron-builder + electron-updater | Done for macOS: `packaging/macos/bundle.sh` (app, dmg, zip), `src/updater.rs`, `.github/workflows/release.yml`. Windows / Linux still to do. |

**Suggested path if we go ahead:**

1. macOS first. Link the existing Metal compositor and AVFoundation/VideoToolbox
   decode + encode into this app through the C++ engine, so preview and export
   match the phone apps exactly.
2. Build the editor UI panel by panel on `gpui-component`, keeping the JSON
   document so projects move between the web, phone and native apps.
3. Windows/Linux after that, on an FFmpeg + GL/wgpu backend for the engine.

Until step 1 lands, the Electron app stays the one people download.

## Layout

```
src/main.rs        window, fonts, menus
src/editor.rs      editor state, edits + undo, playback, layout, projects
src/shortcuts.rs   the shortcut table
src/model.rs       project document (serde, round-trips unknown fields)
src/projects.rs    projects folder
src/engine.rs      engine binding: C++ core (feature) or Rust stand-in
src/decoder.rs     FFmpeg probe, thumbnails, per-file decode threads
src/ui/            panels: header, home, browser, player, inspector,
                   toolbar, timeline, dialogs, widgets, icons
engine-shim/       C ABI over videoengine::Engine (compiled by build.rs)
assets/fonts/      bundled fonts (OFL / MIT / Apache, see README there)
```

## Releases

Merging into the **`release`** branch (a pull request from `main`) runs
`.github/workflows/release.yml`, the counterpart of the Electron build's
release workflow (which releases on merges into its own branch); it can
also be run by hand from the Actions tab. Day-to-day merges into `main`
only run CI.

1. **build** (arm64, and x64 cross-built, on a macOS runner): the static
   LGPL FFmpeg (`packaging/macos/build-ffmpeg.sh`, cached), then
   `cargo build --release --features cpp-engine` against a sparse checkout
   of the private app repo (engine + filter data only), then
   `packaging/macos/bundle.sh`: `Reel Studio.app` (same bundle id as the
   Electron build, `com.reelstudio.desktop`), signed, notarized and
   stapled when the secrets are set, as a `.zip` (for the updater) and a
   `.dmg`.
2. **release**: `update.json` (`packaging/update-manifest.sh`: version,
   per-platform url + SHA-256 + size), then a GitHub Release
   `native-v<version>` on this repo, `--latest=false` and a pre-release,
   so the Electron app's updater (which follows the release marked Latest)
   is untouched. `update.json` is uploaded last, so the updater never sees
   a half-uploaded release.

Version = Cargo.toml's `major.minor` + the run number (`0.2.0` → `0.2.57`).
CI (`.github/workflows/ci.yml`) runs fmt, clippy and tests on the Rust
stand-in for every pull request.

**Auto-update** (`src/updater.rs`), as the old app: checks 5 s after launch
and every 4 h, never downloads on its own (Settings → App updates →
Download), then asks to restart; Later installs on quit. A download must
match `update.json`'s SHA-256, be our bundle id at the promised version
with a valid signature, and (when this app has a team) be signed by the
same team. Only an installed bundle in a writable folder updates (not
`cargo run`, not a quarantine copy).

**Secrets** live in the **`release` environment** (Settings → Environments
→ New environment `release`, Deployment branches: selected, `release`
only), never as repo secrets: then only the release workflow on the
`release` branch can read them, not a workflow edited on another branch or
in a pull request. `REEL_STUDIO_TOKEN` (fine-grained, Contents: read on
the private app repo, nothing else), `REEL_ASSETS_URL` /
`REEL_CATALOG_URL` / `REEL_CATALOG_KEY` (the app's public client config;
the key must be the publishable one, as it ships inside the app) are
required.
`MAC_CSC_LINK` / `MAC_CSC_KEY_PASSWORD` sign and `APPLE_ID` /
`APPLE_APP_SPECIFIC_PASSWORD` / `APPLE_TEAM_ID` notarize, as for the
Electron build. Without them the app is ad-hoc signed: it runs and
updates, but Gatekeeper asks on first open, and macOS forgets folder
permissions after every update (each ad-hoc build is a new identity).

**Keeping a public repo clean**: no keys, `.env` files, certificates or
private sources are ever committed (the build reads them from the
environment and the private checkout). The release workflow hands each
secret only to the step that uses it (`cargo build`, whose build scripts
are third-party code, gets the public client config and no credential),
caches nothing built from private
sources (pull requests aimed at `release` can restore that branch's
caches), keeps engine compiler output out of the public log, and pins
every action to a commit. CI runs pull requests with no secrets and a
read-only token.

**Try the updater locally**: bundle two versions (`REEL_VERSION=0.2.900
cargo build --release --features cpp-engine`, then `bundle.sh`), serve a
folder with the newer zip, its `update.json` and a `releases.json` in the
GitHub API's shape, and launch the older app's binary with
`REEL_UPDATE_FEED=http://127.0.0.1:<port>/releases.json`.
