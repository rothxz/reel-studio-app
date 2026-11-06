# Parity checklist: old desktop editor → GPUI

The old editor (web build in Electron) is the spec; its source map is in
`/CLAUDE.md`. Status: `[x]` done and checked in the running app, `[~]`
partial (say what's missing), `[ ]` to do. Add anything you find that
isn't listed.

## Shell and layout

- [~] Four panels: browser | player | inspector over the timeline toolbar +
  timeline. *Fixed sizes: no gutters, splitters or remembered sizes yet.*
- [ ] Drag dividers (browser 260–640, inspector 260–560, timeline ≥180,
  player ≥320 wide); double-click resets; sizes persisted.
- [ ] Defaults: browser = share of the window within 300–440, inspector
  340, timeline 40 % of height, 6 px gutters, 40 px header.
- [~] Native window, transparent title bar with traffic lights. *Title is
  set; header doesn't double as a drag region yet.*
- [ ] Projects home (list, new project, open, delete, rename) before the
  editor; the header's ✕ returns to it.
- [ ] Project persistence on disk + autosave; header "SAVED · LOCAL" /
  "Unsaved" state.
- [ ] App menu (File, Edit, View, Help) mirroring the shortcuts.
- [ ] Settings dialog (language, appearance, storage, updates).
- [ ] Toasts, blocking progress dialog (long jobs), confirm dialogs.
- [ ] Auto-update + installers (mac dmg/zip, Windows, Linux) to replace
  electron-builder / electron-updater.

## Header

- [ ] ✕ close project
- [ ] Saved status (dot + SAVED · LOCAL)
- [~] Project name. *Shown; no ▾ menu (rename, details) yet.*
- [ ] BETA badge
- [ ] AI agent toggle (opens the agent panel in the inspector slot)
- [ ] Shortcuts button → Shortcuts dialog (⌘/)
- [ ] Export resolution ▾ (e.g. 1080p)
- [ ] Export button (⌘E) → export dialog with progress + preview

## Browser (left)

- [~] Media: Import button, item count, tiles. *List rows, no thumbnail
  tiles, "Added" badge or duration overlay; no folder/stock tabs.*
- [x] Import: file dialog (⌘I), drag and drop, command-line paths
- [ ] Media tile: thumbnail, duration, "Added" badge, drag to timeline,
  + to append, context menu (delete, reveal)
- [ ] Audio: Music (catalog + search), Sound effects, Voiceover (record)
- [ ] Text: Add text (Default, Draw), Text templates
- [ ] Stickers: stickers, GIFs (catalog via the media proxy)
- [ ] Effects: Video effects, Body effects (served catalog, downloaded
  packages)
- [ ] Transitions
- [ ] Captions: Auto generate, Upload file (.srt/.vtt/.ass), Caption styles
- [ ] Filters
- [ ] Tab strip overflow chevron (›) when the panel is narrow

## Player

- [x] Frame-accurate preview of the top visible video clip (FFmpeg decode,
  rotation applied)
- [~] Canvas fitted in the panel (letterboxed). *Uses the frame's own
  aspect; should be the project canvas with background colour/blur.*
- [ ] "Player" panel title
- [ ] Real compositor: all layers (overlays, images, text, stickers,
  drawings), transforms, keyframes, masks, crop, blend modes, filters,
  adjust, effects, transitions, background, cutout
- [ ] Audio playback in sync (mixer, volume, mute, fades)
- [~] Timecode `current | total`. *Shows MM:SS:FF; old app is HH:MM:SS:FF.*
- [~] Transport. *Play/Pause only; add ⏮ go to start, ◀ previous frame,
  ▶ next frame, ⏭ go to end buttons (keys exist for ←/→/Home).*
- [ ] Ratio button (16:9 …) → canvas ratio picker
- [ ] Fullscreen preview (⌘⇧F)
- [ ] On-canvas gizmo: select, move, scale, rotate layers; preview axis
  guides (S)
- [ ] Hardware decode (VideoToolbox / D3D11VA / VA-API) + zero-copy frames

## Inspector (right)

- [~] Nothing selected: project details. *Basic rows; match
  `DesktopProjectDetails`.*
- [~] Clip selected: details + speed buttons + split/delete. *Placeholder;
  replace with the tabbed inspector below.*
- [ ] Tabbed inspector per kind (`inspectorTabs.ts`):
  - [ ] Video: Visual (Basic, Rotate, Remove BG, Mask, Blend, Background on
    main lane) · Audio (Volume) · Speed (Normal, Curve) · Animation · Adjust
  - [ ] Image: Visual · Speed · Animation · Adjust
  - [ ] Text: Text (Basic, Templates, Fonts, Effects, Bubbles) · Animation ·
    Visual (Basic)
  - [ ] Caption: Text (Basic, Fonts, Effects, Bubbles) · Captions (edit
    list) · Animation
  - [ ] Audio: Volume, Beats, Duration
  - [ ] Effect: Effects (replace), Adjust
  - [ ] Sticker: Visual (Basic) · Drawing: Animation
- [ ] Shared controls: number field with drag-scrub, slider row, select,
  colour swatches, keyframe ◇ per property
- [ ] AI agent panel (providers, key vault, chat, tools, `look`), shown in
  the inspector slot

## Timeline toolbar

- [~] Split, Delete. *As text buttons; old app uses icon buttons with
  tooltips + shortcut hints.*
- [ ] Undo / Redo (⌘Z, ⌘⇧Z / ⌘Y)
- [ ] Select tool (A) / Split tool (B)
- [ ] Delete left (Q), Delete right (W)
- [ ] The selected clip's actions (`useClipTools` minus inspector tools:
  replace, crop, duplicate, freeze, extract audio, reverse, …)
- [ ] Keyframe ◇ (add/remove at playhead)
- [ ] Toggles: main-lane magnet (P), linkage, preview axis (S), snapping (N)
- [ ] Zoom to fit (⇧Z), zoom −/slider/+ (⌘- / ⌘=)

## Timeline

- [x] Ruler with timecode ticks; scrub by click/drag; playhead line
- [x] Lanes with clips, selection, wheel scroll, ⌘/ctrl-wheel zoom
- [ ] Timecode readout at the top-left (`00:00 / 02:25`)
- [ ] Lane headers: Cover button on the main lane; lock / eye / speaker /
  "…" per lane (`SetLaneFlagCommand`)
- [ ] Lane types and stacking: main video, overlay, text, captions, sticker,
  effect, audio; add/remove lanes; vertical scroll
- [ ] Clip visuals: filmstrip thumbnails, audio waveform, name, speed
  badge, mute icon, keyframe diamonds, beat dots
- [ ] Transition seam button between main-lane clips (+ seam geometry)
- [ ] Drag clips (within/between lanes), trim handles, ripple on main lane,
  snapping guides, drop from browser, edge auto-scroll
- [ ] Multi-select, copy / cut / paste / duplicate (⌘C/X/V/D)
- [ ] Context menus (clip, lane, empty area)
- [ ] Skim (hover preview) if the old app has it (`timelineSkim.ts`)

## Edits + document

- [x] Same JSON document as the app; unknown fields round-trip
- [x] Append media, split, delete (main lane closes gap), speed (ripple)
- [ ] Undo/redo history with one step per gesture (port the command list
  in `src/stores/commands/`)
- [ ] Keyframes, transforms, masks, crop, adjust, filters, effects,
  transitions, text, stickers, drawings, captions, audio volume/fades,
  speed curves, freeze, reverse (baked copy), extract audio
- [ ] Media relink for projects from the web app

## Export

- [ ] Engine export on desktop: decoder + compositor + encoder backends
  (macOS: AVFoundation/Metal/VideoToolbox from the iOS engine; others:
  FFmpeg + GL/wgpu)
- [ ] Export dialog: resolution, fps, quality, progress, cancel, live
  preview, reveal file

## Shortcuts (from `desktopShortcuts.ts`)

| Action | Keys | Status |
| --- | --- | --- |
| Play/pause | Space | [x] |
| Prev / next frame | ← / → | [x] |
| Back / forward 1 s | ⇧← / ⇧→ | [ ] |
| Prev / next edit point | ↑ / ↓ | [ ] |
| Go to start / end | Home / End | [~] Home only |
| Split | ⌘B | [~] bound to S (fix: S is preview axis) |
| Delete left / right | Q / W | [ ] |
| Deactivate clip | V | [ ] |
| Delete | ⌫ / Delete | [x] |
| Duplicate / copy / cut / paste | ⌘D / ⌘C / ⌘X / ⌘V | [ ] |
| Undo / redo | ⌘Z / ⌘⇧Z, ⌘Y | [ ] |
| Deselect | Esc | [ ] |
| Zoom in / out / fit | ⌘= / ⌘- / ⇧Z | [~] fit missing |
| Preview axis / snapping / main magnet | S / N / P | [ ] |
| Select tool / split tool | A / B | [ ] |
| Fullscreen | ⌘⇧F | [ ] |
| Export | ⌘E | [ ] |
| Shortcuts dialog | ⌘/ | [ ] |

## Platform

- [x] macOS (Apple Silicon) builds and runs
- [ ] Windows, Linux builds (GPUI supports both; FFmpeg + engine backends)
- [ ] Khmer / Vietnamese / Chinese / Indonesian UI strings and fonts
- [ ] Bundled FFmpeg (LGPL, trimmed) instead of Homebrew dylibs
