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
  (Music / Sound / Voiceover, Add text / Text template, …); a view with
  categories (Video effects, Filter, Music, Sound, Stickers) folds them under
  its row in the side nav (chevron); a search hides the category highlight.
  *Transitions, Captions, Voiceover and Text template still say they're
  not in the native editor yet.*
- [x] Media: Import button, item count, tile grid sized to the panel
  (min 104 px, 10 px gaps). Import / drop puts files in the library, not
  on the timeline; the library also lists every file the project uses.
- [x] Import: file dialog (⌘I), drag and drop, command-line paths (a new
  video from them)
- [~] Media tile: thumbnail (decoded off the UI thread), MM:SS, "Added"
  badge, hover border + ⊕, click inserts on the main lane at the playhead
  (before the clip whose middle is past it), drag onto the timeline.
  *No context menu.*
- [~] Audio › Music: the moods in the side nav (Chill first) or a search
  (settles after 400 ms, Enter at once), from the music API through the
  app's media proxy (the catalog database's edge function; empty answers
  asked again twice). Rows: cover with ▶ / ⏸ to listen (streamed, on its
  own decoder thread), title, artist, M:SS, ⊕ on hover. A click downloads
  the track (kept in `<data>/library/music/`, so projects play offline),
  measures it and adds it on the audio lane as the old editor fits it
  (from the playhead when ≥ 0.5 s of picture is left, else 00:00; no
  longer than the picture), titled "Song – Artist", selected. *No Import
  tab (the Media tab imports sound).*
- [x] Audio › Sound: My sounds (downloaded before, from the metadata kept
  beside each file), then the library's categories (the first opens), or a
  search; the same rows; files from the asset host's `sound-effects/`,
  kept in `<data>/library/sound-effects/`.
- [ ] Audio › Voiceover (record)
- [~] Text › Add text: Default text adds a plain caption (Nunito 24,
  white, centred, no entrance) at the playhead on the text lane, selected,
  with its words selected in the inspector so typing replaces them. *Draw
  and Text template aren't ported.*
- [x] Sticker › Stickers / GIFs: the quick picks (Arrows first … Trending)
  then the API's own categories, or a search; GIFs: trending or a search.
  Square tiles sized to the panel (≥ 88 px), the small preview's first
  frame, playing on hover, ⊕; 30 a page, more as the grid nears its end;
  Retry when offline. A click adds a 3 s sticker at the playhead on the
  sticker lane (`<project>-overlay-1`, made on first use above the picture
  lanes), selected. Tile pictures and sticker files are cached by URL
  (`cache/remote/`).
- [~] Effects › Video effects: the served catalog (the catalog database's
  effects table over REST), one category at a time (All, Trending,
  Classic, the mood tabs), 30 rows per request, more as the grid scrolls
  near its end; rows kept on disk (`effect-catalog.json`) so an offline
  tab lists what was seen (with Retry); stale lists (10 min) start over
  when the tab opens. None leads All. A pick adds a 3 s clip (≥ 1 s) on
  the Effects lane at the playhead and selects it, so the next pick
  replaces it; None deletes it; toasts "<name> on" / "Effect removed".
  The picked tile is outlined; its sliders (Reset, a 0–100 slider per
  served parameter, one undo step per drag; the preview follows a held
  slider) are in the inspector, since its clip is selected. The effect's shader package is downloaded on first use
  and renders in the player (see Player › effects). *No search field
  yet (needs the text field). Body effects are left out, as the old
  desktop build does (no on-device person matte).*
- [x] Effect / filter tiles: the hosted previews from the asset host
  (R2 bucket, `effect-previews/<key>`), downloaded once to
  `<data>/cache/` (keys are content-hashed) on 4 worker threads, newest
  request first; the first frame shown, the animated WebP / GIF plays on
  hover (a handful of animations kept). Square tiles sized to the panel
  (≥ 92 px), white ring + Adjust on the picked one, ⊕ on hover, plain
  swatch until the picture arrives.
- [ ] Transitions
- [ ] Captions: Auto generate, Upload file (.srt/.vtt/.ass), Caption styles
- [~] Filters: the categories (All, Featured, Portrait, Landscape, Food,
  Film, Cinematic, Retro, Night, Style, B&W, Classic) and looks compiled
  in at build time from the app checkout (never committed), tiles from
  the asset host. A pick sets `Filters:<name>` on the selected video /
  photo (else the main clip under the playhead), one per clip, keeping
  the intensity already dialled in (new: 80); None removes it; the picked
  tile's Adjust card has Intensity. Looks render in the player: Classic
  ones through the engine's colour matrices, the rest from their
  downloaded packages. *No search. On the Rust stand-in engine (no
  `cpp-engine`) the Classic looks show unfiltered.*
- [~] Tab strip overflow: the row scrolls sideways when the panel is
  narrow. *No ‹ › arrow buttons yet.*

## Player

- [x] Frame-accurate preview of every visible video / photo layer (FFmpeg
  decode, rotation applied; one decoder per layer, so a clip and its copy
  on an overlay lane don't share one)
- [~] The project canvas fitted in the panel on near-black, filled with
  the canvas background colour; the top clip drawn contain-fit in it.
  *No background blur / image yet.*
- [x] "Player" panel title
- [x] Effects and filters in the preview (`src/effects/`, Metal): the
  shown clip's own video effect (distortion before its filter, blend
  after) and filter (Classic: the engine's 4×5 matrix via the shim;
  packaged: `fl_filter` mixed by intensity), then every effect-lane clip
  under the playhead over the canvas (the clip fitted on the background
  colour), lane order. Shader packages are fetched from the asset host
  (`effect-packages/<key>`: an effect's key from its catalog row, a
  filter's from the app's package manifest, compiled in), checked
  against the hash in their key and cached on disk; the dialect is
  rewritten to Metal with this app's own `fx_` / `fl_` helpers, compiled
  once per look. Inputs as the old preview: time since the effect began
  × Speed, intensity, progress, clip length, p1–p4. Runs on its own
  thread (latest frame wins), re-run when the time, the document or a
  held slider changes. Every hosted package compiles (`cargo t --
  --ignored every_hosted_package`). *macOS only for now (other
  platforms show frames unprocessed); GPU readback per frame, not yet
  zero-copy; the background-blur effect and body effects aren't ported.*
- [~] Compositor (`effects/metal.rs`, Metal): the video / photo layers
  back to front, each through its own looks, then placed on the canvas —
  crop, mirror / flip, transform (contain-fit × scale, clockwise turn,
  move), keyframes (the engine evaluates them at the preview time), shape
  mask (six shapes, feather, invert), opacity, Splice blend (W3C
  formulas) — over the canvas colour or the main clip's Background
  (colour, photo, or its own picture blurred at the four strengths). A
  held Visual slider / field shows live before release. One picture with
  nothing to place still goes straight to the screen. Layers paint by
  stack order (the clip's `stackOrder`, else its lane's), and the
  effect-lane clips run right after the last video / photo, so text and
  stickers above stay unfiltered (as the app and its export do).
  - Text clips (`text_render.rs`): laid out with cosmic-text (shaping,
    fallback fonts, wrap at 86 % of the canvas less padding), drawn with
    tiny-skia: fill + opacity, stroke, glow (two passes past half), drop
    shadow, background box or per line (merged), underline, letter / line
    spacing, bold (heavier face), italic (the face, else slanted), curve
    (an arc, on one line), colour emoji as bitmaps. Font size is in text
    units of the canvas's short side / 390, so a caption is the same share
    of the picture at any resolution (the app measured it in preview px).
    Drawn at the preview's size × the clip's zoom (quarter octaves) and
    cached by look; a held slider redraws live.
  - Stickers (`stickers.rs`): the GIF / WebP downloaded, every frame decoded
    with its delay (≤ 150 frames, ≤ 480 px), premultiplied, looping over
    the clip; fitted in 40 % of the canvas's short side at zoom 100 %.
  *Not yet: drawings, transitions, Adjust, cutout, pattern backgrounds,
  crop straighten, text animations / templates.*
- [~] Audio playback in sync (`audio.rs`): a cpal output fed from a ring
  the mixer thread keeps ~120 ms ahead; every audible video / audio clip
  (clip and lane not muted, not deactivated) decoded by FFmpeg from its
  source time, resampled linearly to the device rate (speed shifts pitch,
  as the engine's export does), × volume (linear, to 1000 %) × fades;
  decoders opened a second before a clip starts. Started at the engine's
  time on play, restarted if the two drift > 120 ms, stopped on pause.
  *Not yet: volume keyframes, reversed clips (silent; the app plays a baked
  reversed copy), freeze frames are silent, pitch-preserving speed.*
- [x] Timecode `HH:MM:SS:FF | HH:MM:SS:FF` (mono)
- [x] Transport: ⏮ go to start, ◀ previous frame, ▶/⏸, ▶ next frame,
  ⏭ go to end, each with tooltip + key
- [x] Ratio badge (16:9, 0.73:1 …) → ratio menu (Original, 9:16, 1:1,
  16:9, 4:3, 3:4, 5.8", 2:1, 2.35:1, 1.85:1; 1080 px short side), undoable
- [x] Fullscreen preview (⌘⇧F, Esc leaves): the player alone
- [~] On-canvas gizmo (`gizmo.rs` math, `ui/gizmo.rs`): every text,
  sticker and picture-in-picture layer on screen is picked topmost first
  (12 px slop) and dragged to move; the main video only once selected
  (pan, no corner handle). The selected layer gets a white box (dark
  underlay; 2 px out on text / stickers, text boxed on its words, not its
  glow) with ✕ delete (text, stickers), ✎ edit the words (text: Text ›
  Basic with the field focused; so does a second click on selected text)
  and scale + turn about the centre (bottom right, 0.1–100×), the handles
  turning with it. Moves snap within 10 px to the canvas centre lines and
  the other layers' centres, turns to every 45° within 4°, with a guide
  line / level cross while on one. Shown live, one "Change Transform" undo
  step on release (rounded as the old editor; keyed at the playhead on a
  keyframed clip); a locked clip or lane selects but doesn't move; a click
  on empty canvas clears the selection, ⇧ / ⌘ click toggles; a move pauses
  playback. Cursor: open / closed hand. *Not yet: the Mask gizmo (Mask
  tab), drawings, trackpad pinch / twist, the inspector's numbers
  following a drag before release.*
- [ ] Hardware decode (VideoToolbox / D3D11VA / VA-API) + zero-copy frames

## Inspector (right)

- [x] Nothing selected: Details (Name, Aspect ratio, Resolution, Frame
  rate, Duration, Clips) and Modify → ratio menu.
- [x] Clip selected: the tab row and sub-tab pills per kind from one
  table (`ui/inspector.rs`), the open sub-tab kept across clips that have
  it, else the clip's first. *Panels are placeholders except Speed ›
  Normal.*
- [~] Tabbed inspector per kind (`inspectorTabs.ts`). *Tabs done; panels:*
  - [~] Video: Visual ✓ · Audio (Volume) · Speed (Normal ✓, Curve) ·
    Animation · Adjust. *Speed › Normal: log slider 0.1–100× snapping to
    1×, read-out, length before → after, reset; one undo step per drag.*
    Visual (`ui/visual.rs`, document side `visual.rs`), each control one
    undo step:
    - [x] Basic: Transform (Zoom slider to 400 % / field to 10 000 %,
      Position X / Y in canvas px with Y up, Rotate) with reset and ‹ ◇ ›
      (previous / next diamond, add or delete one at the playhead);
      Opacity on a video / photo. On a keyframed clip the values shown
      are the engine's at the playhead (`reel_engine_clip_property`) and
      edits key the playhead (the diamond under it, else a new one — auto
      keyframe, no switch yet).
    - [x] Rotate: Left / Right quarter turn keeping the fit (the canvas
      turns too when it's the main lane's only clip and had its shape),
      Mirror, Flip (lit when on). *Crop shown disabled (no crop editor).*
    - [~] Remove BG: Auto removal switch, Keep (Person / Auto), Stroke
      switch, colour, Size, Spacing. *No background removal (needs the
      on-device model): a cutout already in the project can be turned off
      (a body effect keeps the matte) and its stroke edited; the edge
      sliders and stroke preset tiles aren't ported.*
    - [x] Mask: None + six shape tiles; the shape's Invert, Position,
      Size (W / H per shape), Rotate (centred slider), Feather, reset
      (also drops its keyframes) and ‹ ◇ › (◇ keys the mask too). A new
      shape keeps the place and size dialled in.
    - [x] Splice (overlay video lanes only): the 11 blend modes, Opacity.
      *Pills, not the sample tiles.*
    - [~] Background (main lane only): Type Color / Image / Blur, colour
      swatches (+ none), Upload photo, four blur strengths over the clip's
      thumbnail, reset, Apply to all (one step). *No pattern tiles, no
      eyedropper or custom colour.*
    - *All of it shows in the player (see Compositor) except Remove BG.
      Number fields are drag-sideways scrubbers until there's a text
      field.*
  - [ ] Image: Visual · Speed · Animation · Adjust
  - [~] Text: Text › Basic (`ui/text_panel.rs`): the words (multi-line
    field, one undo step per run of typing), Font (the app's eight, in a
    menu), Size 12–46, Pattern B / I / U, Color swatches, Opacity,
    Alignment; Stroke, Glow, Background (Box / Per line, colour, opacity,
    roundness, height, width), Shadow (colour, opacity, blur, distance,
    angle) each switched at its title; Spacing (letter, line); Curve.
    Visual › Basic as on pictures. *Not yet: the "Aa" presets row,
    Templates, Fonts, Effects, Bubbles, Animation; custom colours.*
  - [ ] Caption: Text (Basic, Fonts, Effects, Bubbles) · Captions (edit
    list) · Animation
  - [~] Audio (and a video's Audio tab): Volume › Volume — slider with
    100 % at its middle (to 1000 %), read-out, mute, reset (setting a
    level unmutes) — and Fade in / Fade out (0.1 s steps, each side at most
    10 s and never over the other). *Beats and Duration aren't ported;
    no keyframed volume.*
  - [x] Effect: Effects › Adjust only, no second grid (looks are picked
    in the browser): the effect's preview + name with Replace (opens the
    browser's Video effects, where a pick replaces it), then Reset + its
    sliders. An effect the catalog hasn't seen is fetched by name for its
    sliders.
  - [x] Sticker: Visual (Basic).
  - [ ] Drawing: Animation
- [~] Shared controls: section heading with reset, label row, slider row
  (any slider: drag anywhere, live value, commit on release), read-out box,
  segment pills, colour swatches, keyframe ◇. A native text field
  (`ui/text_input.rs`: one line or soft-wrapped lines, caret, selection,
  mouse / word / line moves, copy / cut / paste, IME through the platform
  input handler, Esc gives the keyboard back); the editor's shortcuts are
  bound in `Editor && !TextInput`, so typing never triggers them. *Number
  fields don't take typing yet; no select widget (the Font menu is a
  popup).*
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
  deactivated / hidden lanes dimmed. *Filmstrip ignores speed curves.*
- [x] Beat dots on audio clips: the clip's beat markers (source time →
  timeline through its window and speed) along the bottom edge, 1.6 /
  2.3 / 3 px for weak / medium / strong. White, not the old app's yellow
  accent (the editor stays monochrome). Moved clip edges and keyframe
  diamonds snap to the beats (a clip not to its own). *Editing beats is
  the Audio › Beats panel, not ported.*
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
- [x] Effects on the document: `effects` (`EffectInstance`: id, type,
  enabled, parameters) on clips; an effect-lane clip (`<project>-effect-1`,
  "Effects 1", made on first use, with a default `transform`); one
  `Overlays:` / `Filters:` per clip; slider edits coalesce per effect
- [x] Main track magnet + linkage as one document function
  (`model::with_magnet`, the old `withMagnet`): re-packs the main lane by
  out-transition overlap and carries linked clips
- [x] Undo/redo: document snapshots, one step per gesture (a quick run of
  edits to the same thing merges, 400 ms, 200 steps). *Split doesn't yet
  split keyframes / motion paths / speed curves per half.*
- [x] Text clips, stickers, library sound on the document: the text lane
  (`<project>-text-1`) and sticker lane (`<project>-overlay-1`) made on
  first use above the picture lanes (`layerStack` set); text style fields
  as the app's (unset = its default); `sourceWidth/Height` on stickers;
  `title`, `volume`, `muted`, `audioFade` on audio.
- [ ] Keyframes, transforms, masks, crop, adjust, transitions, drawings,
  captions, speed curves, freeze, reverse (baked copy), extract audio
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

1. Use the new text field for project rename and typed number fields.
2. Timeline leftovers: the cover editor; group move of a multi-selection;
   Graphs badge and tracked-path diamonds.
3. Inspector panels on the real document fields: Adjust; Text's
   Templates / Fonts / Effects / Bubbles / Animation; Audio's Beats and
   Duration.
4. Browser: Transitions (picker + the compositor's transition passes),
   Captions (upload .srt/.vtt, caption styles; auto captions need a
   speech model), Voiceover, Text templates.
5. Export.

## Platform

- [x] macOS (Apple Silicon) builds and runs
- [ ] Windows, Linux builds (GPUI supports both; FFmpeg + engine backends)
- [ ] Khmer / Vietnamese / Chinese / Indonesian UI strings and fonts
- [ ] Bundled FFmpeg (LGPL, trimmed) instead of Homebrew dylibs
