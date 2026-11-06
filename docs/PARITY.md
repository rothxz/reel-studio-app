# Parity checklist: old desktop editor → GPUI

The old editor (web build in Electron) is the spec; its source map is in
`/CLAUDE.md`. Status: `[x]` done and checked in the running app, `[~]`
partial (say what's missing), `[ ]` to do. Add anything you find that
isn't listed.

## Shell and layout

- [x] Four rounded panels: browser | player | inspector over the timeline
  toolbar + timeline, 6 px gutters.
- [x] Drag dividers (browser 260–640, inspector 260–560, timeline ≥180,
  player ≥320 wide); double-click resets; sizes persisted
  (`desktop-editor.json` in the data folder, with the timeline switches).
- [x] Defaults: browser = 24 % of the window within 300–440, inspector
  340, timeline 40 % of height, 40 px header.
- [~] Native window, transparent title bar with traffic lights; double-click
  on the header zooms. *Dragging the window by the header's empty parts
  isn't wired (GPUI's mac backend has no `start_window_move`).*
- [~] Projects home: greeting + drafts count, New video (file dialog or
  drop: a new project with those files), Recent list (thumbnail, length,
  "4m ago"), click opens, right click → Open / Delete with confirm; the
  header's ✕ saves and returns to it. *No rename yet (needs a text field);
  no multi-select delete; the phone-only cards (photo, other tools) are
  left out on purpose, as the old desktop build does.*
- [x] Project persistence: one JSON per project in the data folder's
  `projects/`, written atomically; saved 1.5 s after the last edit, on ✕,
  on window close and on quit; header SAVED · LOCAL / UNSAVED CHANGES.
  ⌘O imports a project JSON (copy), ⌘⇧S writes a copy elsewhere.
- [~] App menu: File, Edit, View, Playback built from the shortcut table
  (same labels and keys). *No Help menu yet.*
- [ ] Settings dialog (language, appearance, storage, updates).
- [~] Toasts (single slot, 1.6 s, as the old app), confirm dialog, popup
  menus. *No blocking progress dialog yet.*
- [ ] Auto-update + installers (mac dmg/zip, Windows, Linux) to replace
  electron-builder / electron-updater.

## Header

- [x] ✕ close project (saves, back to the projects home)
- [x] Saved status (✓ SAVED · LOCAL / • UNSAVED CHANGES)
- [~] Project name ▾. *Shown; a click shows Details in the inspector.
  Rename needs a text field.*
- [x] BETA badge
- [ ] AI agent toggle (opens the agent panel in the inspector slot). *Hidden,
  as in the old app when the agent isn't available.*
- [x] Shortcuts button → Shortcuts dialog (⌘/), with tooltip + key
- [x] Export resolution ▾ (720p / 1080p / 4K, remembered)
- [~] Export button (⌘E). *Says export isn't available yet: no desktop
  encoder (see Export).*

## Browser (left)

- [x] Tab row (icon over label): Media, Audio, Text, Sticker, Effects,
  Transitions, Captions, Filter; sub-nav for tabs with several views
  (Music / Sound / Voiceover, Add text / Text template, …). *Only Media has
  content; the others say they're not in the native editor yet.*
- [x] Media: Import button, item count, tile grid sized to the panel
  (min 104 px, 10 px gaps). Import / drop puts files in the library, not
  on the timeline; the library also lists every file the project uses.
- [x] Import: file dialog (⌘I), drag and drop, command-line paths (a new
  video from them)
- [~] Media tile: thumbnail (decoded off the UI thread), MM:SS, "Added"
  badge, hover border + ⊕, click inserts on the main lane at the playhead
  (before the clip whose middle is past it), drag onto the timeline.
  *No context menu.*
- [ ] Audio: Music (catalog + search), Sound effects, Voiceover (record)
- [ ] Text: Add text (Default, Draw), Text templates
- [ ] Stickers: stickers, GIFs (catalog via the media proxy)
- [ ] Effects: Video effects, Body effects (served catalog, downloaded
  packages)
- [ ] Transitions
- [ ] Captions: Auto generate, Upload file (.srt/.vtt/.ass), Caption styles
- [ ] Filters
- [~] Tab strip overflow: the row scrolls sideways when the panel is
  narrow. *No ‹ › arrow buttons yet.*

## Player

- [x] Frame-accurate preview of the top visible video clip (FFmpeg decode,
  rotation applied)
- [~] The project canvas fitted in the panel on near-black, filled with
  the canvas background colour; the top clip drawn contain-fit in it.
  *No background blur / image yet.*
- [x] "Player" panel title
- [ ] Real compositor: all layers (overlays, images, text, stickers,
  drawings), transforms, keyframes, masks, crop, blend modes, filters,
  adjust, effects, transitions, background, cutout
- [ ] Audio playback in sync (mixer, volume, mute, fades)
- [x] Timecode `HH:MM:SS:FF | HH:MM:SS:FF` (mono)
- [x] Transport: ⏮ go to start, ◀ previous frame, ▶/⏸, ▶ next frame,
  ⏭ go to end, each with tooltip + key
- [x] Ratio badge (16:9, 0.73:1 …) → ratio menu (Original, 9:16, 1:1,
  16:9, 4:3, 3:4, 5.8", 2:1, 2.35:1, 1.85:1; 1080 px short side), undoable
- [x] Fullscreen preview (⌘⇧F, Esc leaves): the player alone
- [ ] On-canvas gizmo: select, move, scale, rotate layers; preview axis
  guides (S)
- [ ] Hardware decode (VideoToolbox / D3D11VA / VA-API) + zero-copy frames

## Inspector (right)

- [x] Nothing selected: Details (Name, Aspect ratio, Resolution, Frame
  rate, Duration, Clips) and Modify → ratio menu.
- [x] Clip selected: the tab row and sub-tab pills per kind from one
  table (`ui/inspector.rs`), the open sub-tab kept across clips that have
  it, else the clip's first. *Panels are placeholders except Speed ›
  Normal.*
- [~] Tabbed inspector per kind (`inspectorTabs.ts`). *Tabs done; panels:*
  - [~] Video: Visual (Basic, Rotate, Remove BG, Mask, Splice, Background
    on main lane) · Audio (Volume) · Speed (Normal ✓, Curve) · Animation ·
    Adjust. *Speed › Normal: log slider 0.1–100× snapping to 1×, read-out,
    length before → after, reset; one undo step per drag.*
  - [ ] Image: Visual · Speed · Animation · Adjust
  - [ ] Text: Text (Basic, Templates, Fonts, Effects, Bubbles) · Animation ·
    Visual (Basic)
  - [ ] Caption: Text (Basic, Fonts, Effects, Bubbles) · Captions (edit
    list) · Animation
  - [ ] Audio: Volume, Beats, Duration
  - [ ] Effect: Effects (replace), Adjust
  - [ ] Sticker: Visual (Basic) · Drawing: Animation
- [~] Shared controls: section heading with reset, label row, slider row
  (any slider: drag anywhere, live value, commit on release), read-out box,
  segment pills. *No editable number field (needs a text field), select,
  colour swatches or keyframe ◇ yet.*
- [ ] AI agent panel (providers, key vault, chat, tools, `look`), shown in
  the inspector slot

## Timeline toolbar

- [x] Icon buttons (28 px) with hover tooltips "Label  key"
- [x] Undo / Redo (⌘Z, ⌘⇧Z / ⌘Y), disabled when there's nothing to do
- [x] Select tool (A) / Split tool (B): the blade cuts a clicked clip at the
  click (frame-snapped); Esc goes back to Select
- [x] Delete left (Q), Delete right (W): selected clip under the playhead,
  else the main clip; trims the source window; magnet closes the gap
- [~] The selected clip's actions. *Split, Delete, Duplicate (per kind, old
  order); nothing selected: Split + disabled Delete. Still to port:
  replace, crop, freeze, reverse, extract audio, rotate…*
- [~] Keyframe ◇. *Shown disabled until keyframes are editable.*
- [x] Toggles: main-lane magnet (P), linkage, preview axis (S), snapping
  (N), with toasts; remembered.
- [x] Zoom to fit (⇧Z), zoom −/log slider/+ (⌘- / ⌘=, ×1.5, 4–400 px/s)

## Timeline

- [x] Ruler: tick lines with "00:02" / "00:01.5" labels beside them (label
  step ≥ 96 px apart, minor ticks between), running past the end; click /
  drag on it or on empty lane space scrubs (and deselects)
- [x] Layout as the old desktop timeline: ruler, the main lane (56 px) under
  it, then a stack of the lanes that hold clips (Overlay video, Effects,
  Audio, Text + stickers; 32 px rows, 6 px gaps). Empty lanes aren't drawn.
- [x] Sub-rows: clips overlapping on a lane stack in rows (`assign_rows`)
- [x] Vertical scroll of the lane stack (wheel) with a scrollbar while it
  overflows; ⇧ + wheel sideways; ⌘ / ctrl + wheel zooms around the pointer;
  ⌘= / ⌘- zoom around the playhead; ⇧Z fits (80 px left after the end)
- [x] Playhead: shield on the ruler + line; timecode readout top left
- [~] Track headers: lane icon, Lock / Hide / Mute (lanes with sound) over
  all the lane's tracks, one undo step each; locking drops the lane's clips
  from the selection; "…" → Select all on track / Delete track. The
  column is 208 px, as the old desktop's.
- [~] Cover tile at the left of the main lane's header: the saved cover
  (`cover.imageUri`), else the main lane's first frame, under a shade with
  ✎ Cover. *A click says the cover editor isn't ported yet.*
- [x] Captions lane: auto captions (text clips with caption data) leave the
  Text lane for their own lane at the top of the stack, one row, caption
  colour. Captions and Text share the text track, so their switches set
  the flags on their own clips (lane targets = tracks + clips, as the old
  app); the CC icon selects the caption under the playhead (else the next,
  else the last).
- [x] Clip visuals: kind colours (old desktop palette), filmstrip tiles on
  video (decoded on a background thread, frames snapped to a time grid so
  zooming reuses them), the picture on images / stickers, the waveform on
  audio (FFmpeg peaks, clip volume), name + length strip on pictures,
  speed badge, lock / mute badges, hover outline, white selection outline,
  deactivated / hidden lanes dimmed. *Filmstrip ignores speed curves; no
  beat dots yet.*
- [x] Keyframe diamonds on the selected clip: one ◆ per keyframe time
  (enabled property tracks, 50 ms merged, thinned to one per 12 px), on
  the clip's middle; the one under the playhead red. Click = playhead onto
  it; drag = move it on every property track (snaps to the clip's edges,
  the playhead and the other diamonds; one undo step; a diamond already
  there gives way). *No Graphs badge, no tracked-path diamonds yet.*
- [x] Transition seams: boxes meet mid-overlap; a button between main
  clips (opens the browser's Transitions tab, which isn't ported yet)
- [x] Drag clips: press-drag moves (selects it); the old slot stays as a
  grey box, the clip follows see-through; snapping to 00:00 and clip edges
  (either edge of a moved clip) with a guide line; Esc cancels
- [x] Main lane + magnet: a dragged main clip goes in at the nearest cut
  (insert line), the lane re-packs; magnet off: a plain move
- [x] Pictures change lanes: a main clip pulled below the main lane lands
  on a free overlay video lane (a new one when none is free) and the gap
  closes; an overlay picture pulled onto the main lane goes in at the
  nearest cut (magnet) or where it was let go
- [x] Trim handles on the selected clip (10 px, white): either edge, live,
  snapped, ≥ 0.6 s, never before 00:00 nor past the media's start / end;
  source window follows; on the main lane the magnet closes up (+ linkage)
- [x] Edge auto-scroll while dragging a clip or scrubbing
- [x] Drop from the media library (drag a tile) or the desktop (files):
  on the ruler / main lane in at the cut nearest the pointer (insert line),
  below it on an overlay lane from that time (ghost); sound to the audio
  lane at that time. One undo step.
- [x] Click a clip selects it (no seek, as the old app); the Split tool
  cuts it; a locked clip acts like empty space
- [x] Multi-select: "Select all on track", ⇧ / ⌘ click adds or removes a
  clip, ⇧ / ⌘ + drag on empty lane space draws a rubber band (a plain drag
  still scrubs, as the old app). Every selected clip gets the outline;
  trim handles only on a lone selection. The last picked is the primary:
  the inspector, Split, Duplicate and Copy follow it; Delete (⌫, toolbar,
  menu) and Deactivate (V) take them all, one undo step. *No group move
  yet.*
- [~] Copy / cut / paste / duplicate (⌘C/X/V/D): in-app clipboard; paste on
  the main lane goes in at the cut after the clip under the playhead.
  *One clip (the primary), as the old app; cut deletes the whole
  selection.*
- [x] Context menus: clip (Copy, Cut, Paste, Delete, Split, Duplicate,
  Deactivate / Activate, with shortcut hints and disabled states), empty
  lane space (Paste, Split, main track magnet, linkage, preview axis,
  snapping, zoom to fit), lane "…" (Delete track)
- [x] Hover line; with the preview axis on the player shows the hovered
  frame while the playhead stays put (any press or leaving ends it)

## Edits + document

- [x] Same JSON document as the app; unknown fields round-trip; lane
  `order` is a float (the app slots lanes at halves)
- [x] Insert media at the playhead, split (reversed / freeze aware), delete,
  speed, delete left/right (trims the source window like `trimmedSource`),
  duplicate, paste, deactivate (clip `hidden`), lane flags, canvas ratio
- [x] Main track magnet + linkage as one document function
  (`model::with_magnet`, the old `withMagnet`): re-packs the main lane by
  out-transition overlap and carries linked clips
- [x] Undo/redo: document snapshots, one step per gesture (a quick run of
  edits to the same thing merges, 400 ms, 200 steps). *Split doesn't yet
  split keyframes / motion paths / speed curves per half.*
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
| Back / forward 1 s | ⇧← / ⇧→ | [x] |
| Prev / next edit point | ↑ / ↓ | [x] |
| Go to start / end | Home / End | [x] |
| Split | ⌘B | [x] |
| Delete left / right | Q / W | [x] |
| Deactivate clip | V | [x] |
| Delete | ⌫ / Delete | [x] |
| Duplicate / copy / cut / paste | ⌘D / ⌘C / ⌘X / ⌘V | [x] |
| Undo / redo | ⌘Z / ⌘⇧Z, ⌘Y | [x] |
| Deselect | Esc | [x] (full screen → split tool → popup → selection) |
| Zoom in / out / fit | ⌘= / ⌘- / ⇧Z | [x] |
| Preview axis / snapping / main magnet | S / N / P | [x] |
| Select tool / split tool | A / B | [x] |
| Fullscreen | ⌘⇧F | [x] |
| Export | ⌘E | [~] toast until export exists |
| Shortcuts dialog | ⌘/ | [x] |

All of them come from one table (`src/shortcuts.rs`): key bindings, menu
items, tooltips and the dialog.

## Still to build next (in order)

1. A native single-line text field (GPUI has none built in): project
   rename, editable number fields, search, text clips.
2. Timeline leftovers: beat dots; the cover editor.
3. Inspector panels on the real document fields: Basic (transform,
   opacity), Volume, Rotate, Background, Adjust.
4. Audio playback, then the compositor (all layers), then export.

## Platform

- [x] macOS (Apple Silicon) builds and runs
- [ ] Windows, Linux builds (GPUI supports both; FFmpeg + engine backends)
- [ ] Khmer / Vietnamese / Chinese / Indonesian UI strings and fonts
- [ ] Bundled FFmpeg (LGPL, trimmed) instead of Homebrew dylibs
