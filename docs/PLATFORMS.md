# Linux and Windows

The app was built and released for macOS only. This page tracks making
Linux and Windows work as well: what's done, what's still missing, and how
to test it.

Branch: `feat/linux-windows` (off `main`, not yet pushed).

## Status at a glance

| Area | macOS | Linux | Windows |
| --- | --- | --- | --- |
| Builds (type-checks, clippy) | ✅ | ✅ cross-checked from macOS | ✅ cross-checked from macOS |
| Real build on the platform | ✅ | ⏳ first CI run | ⏳ first CI run |
| Preview: layers, effects, filters | ✅ Metal | ✅ wgpu (Vulkan), tested on macOS only | ✅ wgpu (DX12), tested on macOS only |
| Window buttons, title bar | ✅ system | ✅ system, or our own on GNOME Wayland | ✅ our own (system snap and drag) |
| App menu | ✅ menu bar | ✅ ☰ button | ✅ ☰ button |
| AI agent keys kept across launches | ✅ Keychain | ✅ Secret Service | ✅ Credential Manager |
| Offline voice | ✅ `say` | ✅ `espeak-ng` (if installed) | ✅ Windows speech |
| In-app updates | ✅ | ✅ AppImage | ✅ zip / installed copy |
| Downloads | zip, dmg | AppImage, tar.gz | zip, installer |
| Code signing | optional | — | optional (SmartScreen warns without it) |

✅ written and checked as far as possible on macOS · ⏳ needs a run on the
real platform.

## What changed

### Preview renderer (the biggest piece)

Off macOS the preview showed only the top picture: no layering, transforms,
masks, blends, effects or filters, because all of that ran on Metal.

- `src/effects/draw.rs`: the composition (each picture through its looks,
  placed on the canvas, the effect lane over it) written once over a small
  `Gpu` trait, with the renderer tests.
- `src/effects/metal.rs`: Metal behind that trait (macOS, unchanged
  behaviour).
- `src/effects/portable.rs` + `glsl.rs`: wgpu behind it, drawing GLSL
  through naga (Vulkan on Linux, DX12 or Vulkan on Windows). Falls back to
  a software adapter (Mesa lavapipe, WARP) when there's no GPU.
- Checked on macOS (wgpu on Metal): the same composition tests as Metal
  pass, and all 563 hosted effects and filters compile.
  `cargo test --features wgpu-effects effects::`; the app can draw with it
  on macOS with `--features wgpu-effects` and `REEL_GPU=wgpu`.

### Build

- `build.rs`:
  - MSVC flags for the C++ engine (`_USE_MATH_DEFINES`, `/utf-8`).
  - Static FFmpeg on Linux, linked in the order GNU ld needs.
  - Static FFmpeg on Windows (`avcodec.lib`, MSVC-style system libraries).
  - The Windows resources: manifest (per-monitor DPI, which gpui needs for
    sharp text), icon, version info.
- `Cargo.lock`:
  - `libc` pinned to 0.2.189: 0.2.190 dropped `ENOATTR` on Linux, which a
    gpui dependency (`xattr 0.2`) needs. Don't `cargo update` past it until
    that dependency moves on.
  - `gpu-allocator` re-resolved onto `windows` 0.58, as wgpu's DX12 backend
    needs.
- `src/decoder.rs`: the FFmpeg log callback's `va_list` type for each
  platform (the x86_64 macOS / Linux array form, the Windows pointer).
- The C++ engine compiles cleanly with GCC 16 and libstdc++. Not yet with
  MSVC.

### App

- `src/ui/window_chrome.rs`:
  - The window buttons and drag areas where the system draws none.
  - Linux resize edges when the app decorates its own window.
  - The ☰ app menu, built from the same menus as the macOS menu bar.
- `src/main.rs`:
  - Closing the window quits (Linux, Windows).
  - Linux app id `reel-studio` (matches the .desktop file).
  - Windows release builds have no console window.
- `src/agent/keys.rs`: keys in the system credential store (gpui's
  credentials API).
- `src/agent/os_voice.rs`: the system voice on Windows (System.Speech) and
  Linux (`espeak-ng` / `espeak`).
- `src/updater.rs`:
  - Linux replaces the AppImage it runs from (`$APPIMAGE`).
  - Windows replaces `reel-studio.exe`: the running copy is renamed aside
    and removed at the next launch.
- Small fixes:
  - Home folder on Windows (`USERPROFILE`).
  - Clip names from `\` paths.
  - `file:///C:/…` covers.
  - The greeting's local hour (chrono, not the `date` command).
  - "Open skills folder".
  - The undo shortcut in an agent toast.

### Packaging and CI

- `packaging/linux/build-ffmpeg.sh`, `packaging/windows/build-ffmpeg.sh`:
  the same static, LGPL, decode-only FFmpeg 9.0.2 as macOS (Windows: built
  by MSVC in MSYS2, with zlib from source).
- `packaging/linux/bundle.sh`: AppImage (appimagetool 1.9.1, pinned by
  SHA-256), tar.gz, `.deb` (`dpkg-deb`, Depends from `dpkg-shlibdeps`)
  and `.rpm` (`rpmbuild`, requires found from the binaries); fails if the
  binary needs a missing library or a shared FFmpeg. The packages install
  to `/usr/bin` + `/usr/lib/reel-studio` with the menu entry and icon; the
  release installs the `.deb` on Ubuntu 22.04 and the `.rpm` in a Fedora
  container before uploading them. Only the AppImage updates itself
  (`$APPIMAGE`); a package install shows updates as unsupported.
- Face swap's ONNX Runtime on Linux is loaded at run time, not linked
  (`src/face_swap/runtime.rs`): the prebuilt library ort links in needs
  glibc 2.38+, and the release builds on Ubuntu 22.04 (2.35).
  `packaging/linux/fetch-onnxruntime.sh` fetches Microsoft's 1.31.0 CPU
  build (glibc 2.28+, pinned by SHA-256); `bundle.sh` ships it in the
  AppImage (`usr/lib/reel-studio/`) and next to the program in the tar.gz.
  For a dev build: fetch it next to the binary, or set `ORT_DYLIB_PATH`.
  Bumping ort's `api-NN` means bumping that script's version too.
  Intel Macs load it too (ort has no x86_64 macOS build): Microsoft's last
  Intel one, 1.23.2 (`api-23`, macOS 13.4+), from the app's
  `Contents/Frameworks` (`packaging/macos/fetch-onnxruntime.sh`; signed
  with the app). `cfg(ort_dynamic)` (build.rs) marks both targets; CI
  type-checks the Intel build.
- `packaging/windows/bundle.ps1` + `reel-studio.iss`:
  - A zip, and an Inno Setup installer that installs per user into
    `%LOCALAPPDATA%\Programs\Reel Studio`: no admin rights, and a folder
    the updater can write to.
  - Optional signing with `WIN_SIGN_PFX` / `WIN_SIGN_PASSWORD`.
- `packaging/update-manifest.sh`: `linux-x86_64`, `linux-aarch64` and
  `windows-x86_64` entries in `update.json`.
- `.github/workflows/ci.yml`:
  - Linux (Ubuntu 22.04, software Vulkan so the renderer tests really run)
    and Windows (2022) jobs: clippy and tests.
  - The macOS job also runs the wgpu renderer tests.
- `.github/workflows/release.yml`: Linux x64 + arm64 and Windows x64 jobs
  beside macOS; one release with every download and one `update.json`.

## Still missing

### Must do before shipping

1. **First real CI run on Linux and Windows.** Nothing has been built on
   either platform yet; expect fixes. Most likely to need work:
   - the Windows FFmpeg build (MSYS2 + MSVC);
   - the C++ engine under MSVC (only built with clang and GCC so far);
   - Linux link order with the static FFmpeg.
2. **A test release build.** The release workflow runs only on pushes to
   `release` and needs the release secrets. Add a manual run that builds
   the downloads without publishing, to try them in VMs (below).
3. **Try the app on each platform** (checklist below), including GNOME on
   Wayland, where the app draws its own title bar.
4. **Release secrets.** Nothing new is required; `WIN_SIGN_PFX` and
   `WIN_SIGN_PASSWORD` are optional.

### Known gaps

- **Export / "make video" encoding.** The release FFmpeg is built without
  encoders or muxers, on every platform (macOS included), so the agent's
  H.264 movie writer finds no encoder in a release build. Needs encoders
  that fit LGPL: VideoToolbox (macOS), Media Foundation (Windows), and
  VA-API or OpenH264 (Linux).
- **Hardware video decoding.** Software decoding only on Linux and Windows
  (D3D11VA / VA-API not enabled).
- **Linux without a keyring daemon.** API keys last for the session only (a
  log line says so; the agent panel still says "saved").
- **Linux voice** needs `espeak-ng` installed. The local voice (Kokoro) is
  Apple-silicon only.
- **Windows on ARM.** x64 only (it runs under emulation); no native arm64
  build.
- **Unsigned Windows build.** SmartScreen warns until there's a
  code-signing certificate.
- **Distribution packages.** No .deb / .rpm / Flatpak / winget; the
  AppImage and installer only.
- **Shortcut labels.** The Shortcuts dialog and tooltips follow the
  shortcut table (Ctrl on Linux / Windows), but haven't been reviewed on
  those systems.
- **Dev tools.** `tools/shot.sh`, `tools/winid` and the cliclick /
  osascript driving in CLAUDE.md are macOS only.

## How to test

### CI (builds)

Push the branch and open a PR: the Linux and Windows jobs build FFmpeg
(cached after the first run: ~10 min on Linux, ~25 min on Windows), then
clippy and the tests. The renderer tests run on software Vulkan (Linux) and
WARP (Windows).

### Locally on macOS (type-checks only)

Linux and Windows can be type-checked from macOS with zig as the C
compiler. Expect several GB of build output per target in `target/`;
delete `target/x86_64-unknown-linux-gnu` and `target/x86_64-pc-windows-msvc`
afterwards.

```sh
brew install zig
# Linux: zig cc for x86_64-linux-gnu as CC/CXX/AR for the target, a stub
# alsa.pc for pkg-config, zig's glibc headers for bindgen
#   (-D__GLIBC_MINOR__=35 -isystem …/x86-linux-gnu …/generic-glibc …)
cargo check --target x86_64-unknown-linux-gnu --all-targets
# Windows: zig cc -target x86_64-windows-gnu, `zig lib` as AR, the MinGW
# headers for bindgen (-D__IMMINTRIN_H -D__X86INTRIN_H)
cargo check --target x86_64-pc-windows-msvc --all-targets
```

This catches errors in Linux- and Windows-only Rust code (`cfg`-gated
code), not link or runtime problems.

### Virtual machines (running the app)

Use the downloads a release (or test) build makes; building inside a VM is
slow and needs the whole toolchain. Each VM needs 30–60 GB of disk.

| | Tool | Setup |
| --- | --- | --- |
| Linux | UTM or Parallels | Ubuntu 24.04 ARM64, the **arm64 AppImage**. Usually no Vulkan GPU: `sudo apt install mesa-vulkan-drivers espeak-ng` (software Vulkan, and the system voice). |
| Windows | Parallels (best DirectX), or UTM | Windows 11 ARM, the **x64** installer (runs under emulation). Without a GPU, effects fall back to WARP (slow). |

Judge correctness there, not speed.

### Checklist on each platform

- [ ] Starts; home screen; greeting fits the time of day
- [ ] ☰ menu: every item works and shows its shortcut
- [ ] Window: move, double-click maximize, minimize, maximize / restore,
      close (saves the project, then quits); Windows snap layouts; Linux
      resize from the edges on GNOME Wayland
- [ ] Import video, audio, a photo (PNG and JPEG); play, scrub
- [ ] Preview: two layers, move / scale / turn / crop, a mask, a blend,
      an effect, a filter, a Background blur and photo
- [ ] Sound plays in step
- [ ] Shortcuts (Ctrl+Z, Ctrl+S, Space…); the Shortcuts dialog
- [ ] Save, quit, reopen: project and panel sizes kept
- [ ] AI agent: save a key, restart, still there; system voice speaks
- [ ] Updates: install version N, publish N+1, update from Settings,
      restart into N+1 (Windows: the old exe is gone after the next launch)
- [ ] Windows installer: Start menu entry, uninstall leaves nothing
- [ ] Linux: the AppImage runs on a fresh install; tar.gz `INSTALL.txt`
      steps work; the `.deb` installs from GNOME Software / apt and the
      `.rpm` with dnf, the app shows in the menu, and uninstalling removes
      it
- [ ] Face swap: download the models, detect faces, process a clip (on
      Linux from the AppImage and from the installed tar.gz)
