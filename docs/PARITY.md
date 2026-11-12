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
- [~] Projects home: left sidebar (Edit, Projects, Settings dialog);
  Edit = greeting + drafts count, the New video card (a blank project in
  the editor, written only once edited; files dropped on the window start
  one with them, with a dashed "drop to start" overlay) pinned over the
  list and shrinking to a slim bar as it scrolls, Recent (newest 5, "See
  all" → Projects) over a Videos pill with the count; Projects = every
  draft. Rows: thumbnail, length, "4m ago"; click opens, right click →
  Open / Select / Delete; select mode ticks drafts for one Delete. The
  header's ✕ saves and returns to it. *No rename yet (needs a text field);
  the phone-only cards (photo, scans, other tools) are left out on
  purpose, as the old desktop build does.*
- [x] Project persistence: one JSON per project in the data folder's
  `projects/`, written atomically; saved 1.5 s after the last edit, on ✕,
  on window close and on quit; header SAVED · LOCAL / UNSAVED CHANGES.
  ⌘O imports a project JSON (copy), ⌘⇧S writes a copy elsewhere.
- [~] App menu: File, Edit, View, Playback built from the shortcut table
  (same labels and keys). *No Help menu yet.*
- [~] Settings dialog: General (Language, Data and Storage), Updates,
  Privacy (share anonymous usage data), Help (Report a bug: a mail
  draft with app version, OS and language), Follow us (Instagram,
  Facebook) and About (Privacy Policy, Terms of Use, Data deletion,
  version), the links opened in the browser (src/links.rs). *No
  appearance yet; no replay-onboarding or tips rows (the desktop has
  neither).*
- [x] Data and Storage (Settings pages with a back arrow): Storage Usage
  (donut, cache categories ticked over one Clear button, refused while an
  export / captions / agent job runs; the user's own data; Auto-remove
  cache and Maximum cache size, applied at launch), Data Usage (download
  counts per kind since a date, reset) and AI models (speech models and
  the natural voice: delete one or all). *No Wi-Fi / mobile split (a
  desktop has no cellular); fonts and thumbnails have no disk cache here;
  the older desktop build's own files in the data folder aren't counted.
  Desktop-only strings (natural voice, AI models notes) stay English.*
- [~] Languages: English, Khmer, Vietnamese, Simplified Chinese,
  Indonesian (Settings → Language, "Same as device" by default; the UI
  and the app menu switch at once). The app's dictionaries are compiled
  in from the checkout; strings are looked up by their English text, so
  only strings worded exactly as the old editor's translate. *Desktop-only
  strings with no dictionary entry stay English: Privacy row, update
  dialog, export screen messages, AI agent panel, File menu items,
  Create New Project, undo labels.* In Khmer every UI text is set in the
  bundled Kantumruy Pro (mono labels and timecodes too), with line
  heights of at least 1.5 × the size, as the old editor does.
- [~] Toasts (single slot, 1.6 s, as the old app), confirm dialog, popup
  menus. *No blocking progress dialog yet.*
- [~] Auto-update + installers to replace electron-builder /
  electron-updater: macOS done (dmg + zip, signed / notarized when the
  secrets are set, `v…` releases from `release.yml`; check on
  launch + every 4 h, Download, Restart now / Later installs on quit).
  *Windows, Linux to do; moving Electron users onto the native app is
  still to decide.*

## Header

- [x] ✕ close project (saves, back to the projects home)
- [x] Saved status (✓ SAVED · LOCAL / • UNSAVED CHANGES)
- [~] Project name ▾. *Shown; a click shows Details in the inspector.
  Rename needs a text field.*
- [x] BETA badge
- [x] AI agent toggle (opens the agent panel in the inspector slot),
  remembered across launches
- [x] Shortcuts button → Shortcuts dialog (⌘/), with tooltip + key
- [x] Export resolution ▾ (720p / 1080p / 4K, remembered) and frame rate
  (24 / 30 / 60; the project's own, snapped, until one is picked)
- [x] Export button (⌘E): save dialog (last folder, else Movies), then the
  export screen (see Export).

## Browser (left)

- [x] Tab row (icon over label): Media, Audio, Text, Sticker, Effects,
  Transitions, Captions, Filter; sub-nav for tabs with several views
  (Music / Sound / Voiceover, Add text / Text template, …); a view with
  categories (Video effects, Filter, Music, Sound, Stickers) folds them under
  its row in the side nav (chevron); a search hides the category highlight.
  *Voiceover and Text template still say they're
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
- [x] Transitions (`transitions.rs`, `ui/transitions.rs`): the picker aims
  at the main lane's cut nearest the playhead ("Applies to the cut at …");
  categories (Trending, Basic, Slide, Zoom, Spin, Light, Glitch, Mask, 3D)
  folded under the view in the side nav; None first on Trending, the
  picked tile ticked (again = off). A pick stores `outTransition` on the
  outgoing clip, overlaps the two clips by its length (later main clips
  move left), and replays the cut. DURATION slider (0.1 s up to half of
  either clip, one undo step per drag) and Apply to all. Posed presets
  (Mix, Black Fade, Pull, Zoom, Flip, Slide, Cover, Spin, Rotate, Swing:
  written here, same ids) move both clips; the shader reveals (the clip
  In reveals in Light / Glitch / Mask / 3D) come from the app checkout at
  build time, their packages downloaded on first use, tile animations
  from the asset host. Unknown stored types play as Mix. *No search box
  yet; posed tiles show an icon, not a live preview.*
- [~] Captions (`captions.rs`, `caption_styles.rs`, `speech.rs`,
  `ui/captions.rs`): Auto captions (speech language, Style field opening
  the Style view, what's captioned: selected clips with sound, else every
  video clip; Generate; progress / Cancel in place while it runs, tabs
  can change) on this computer with whisper.cpp's `whisper-cli` (PATH or
  Homebrew, `REEL_WHISPER` overrides) and a multilingual model downloaded
  once to `<data>/models/`. Khmer runs its own model (the community
  Khmer fine-tune of Whisper Base, 60 MB, pinned, credited "sengtha /
  iAny" in the Khmer note): its speech found by loudness, cut into
  ≤ 6 s pieces at quiet points, beam search, an empty piece split and
  retried, syllables cut between segments joined, lines broken only
  between syllables. Every language drops a looping transcript ("ng ng
  ng…"). Upload file (.srt / .vtt / .ass, kept at the
  file's times, replacing overlapped captions, cut at the video's end).
  Style: All / Monoline / Multiline, our own looks; a pick restyles every
  caption (one undo step) and is remembered for the next ones. *Not yet:
  whisper bundled with the app (users without `whisper-cli` get a toast
  saying how to install it), Accurate / Fast quality, caption pages
  (one clip per line-sized segment instead).*
- [x] Edit captions (a caption selected → inspector Edit captions,
  `ui/caption_edit.rs`): every caption with its mm:ss, the one under the
  playhead lit, the selected one in the text field; Merge up / Merge
  down, Split at the text cursor (time by the words' share, snapped to a
  Khmer syllable), Delete, Find & replace (any case, match count,
  Replace all). Each one undo step. The timeline's Split on a caption
  cuts its words too (at the nearest word / syllable, never under
  0.3 s a side).
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
  crop (with its straighten: the source turned about the crop's centre,
  grown to cover it), mirror / flip, transform (contain-fit × scale, clockwise turn,
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
  Transitions: each clip's pose inside a transition window (opacity,
  move by its own size, scale, turn, stretch) on its placement, and a
  reveal shader pass on the incoming clip; the incoming clip draws over
  the outgoing one. Shared by the player and export (`effects::stack`).
  *Not yet: drawings, Adjust, cutout, pattern backgrounds,
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
  it, else the clip's first. *Placeholders left: Animation (clip,
  text, drawing), Text's Templates / Fonts / Effects / Bubbles.*
- [~] Tabbed inspector per kind (`inspectorTabs.ts`). *Tabs done; panels:*
  - [~] Video: Visual ✓ · Audio (Volume) · Speed (Normal ✓, Curve ✓) ·
    Animation · Adjust ✓. *Speed › Normal: log slider 0.1–100× snapping to
    1×, read-out, length before → after, reset; one undo step per drag;
    a constant speed drops any curve.*
    - [x] Visual › Face Swap (`ui/face_swap.rs`, pipeline in
      `src/face_swap/`; new here, the old editor has none): a face from
      a photo on a chosen person, on this computer (ONNX Runtime; Core ML
      / DirectML / CUDA, CPU fallback). Models downloaded once after a
      licence prompt (pinned URLs + SHA-256; inswapper and ArcFace are
      non-commercial), consent prompt per photo, Detect faces → person
      tiles, Blend, Fast / High, restoration (GFPGAN, High), Process with
      progress / cancel / errors. Non-destructive: `faceSwap` on the
      clip, results in `<data>/cache/face-swap` pasted over decoded
      frames by the player and export (export refuses unprocessed
      swaps and tags the MP4 as AI-edited); timeline tag "AI FACE";
      models and results in Settings → Data and Storage. *Not yet:
      occlusion masks (a hand over the face is pasted over), a
      commercially licensed swap model, showing frames while a job
      runs, image clips, the agent tool, translations.*
    - [x] Speed › Curve (`ui/speed_curve.rs`, document side
      `speed_curve.rs`): None, Custom and six ramp tiles with their
      shapes; with one on, a card: name, length before → after, the
      graph (0.1× / 1× / 10×, the playhead's line), beats to drag (ends
      move up / down only), Speed at the playhead, Reset, Add / Delete
      beat at the playhead. Each pick / release is one undo step that
      re-times the clip to the ramp's average speed (`speedCurve` +
      `speed`, the app's JSON; the engine plays it) and the magnet
      closes up the main lane. Timeline tag: the ramp's name. *Not yet:
      keyframes / motion path re-timed with the clip (also not on a
      Normal speed change), the per-half curve on Split, the card's play
      button, beat snapping.*
    - [~] Adjust (`ui/adjust_panel.rs`, document side `adjust.rs`): the
      clip's filter (name, delete, Intensity, the player following the
      held slider), then Adjust with Reset (the effect and its
      keyframes, one step) and the Color (Temperature, Tint, Saturation,
      Hue) / Lightness (Exposure, Brightness, Contrast) sliders as an
      "Adjust" effect; keyframed sliders key the playhead and show its
      values. The player draws it with the engine's matrix
      (`reel_adjust_color_matrix`), inside a Classic filter, after a
      packaged one, keyframes evaluated, live while dragged. *Not yet:
      Auto adjust (measure a few frames), the coloured tracks of
      Temperature / Tint / Saturation / Hue; the Rust stand-in engine
      shows no Adjust.*
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
      Mirror, Flip (lit when on), Crop (opens the Crop dialog).
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
  - [~] Image: Visual ✓ · Speed ✓ (like the old editor, "Select a video
    clip first": a photo's length is trimmed, not sped) · Animation ·
    Adjust ✓ (no Auto adjust on a photo, as before)
  - [~] Text: Text › Basic (`ui/text_panel.rs`): the words (multi-line
    field, one undo step per run of typing), Font (the app's eight, in a
    menu), Size 12–46, Pattern B / I / U, Color swatches, Opacity,
    Alignment; Stroke, Glow, Background (Box / Per line, colour, opacity,
    roundness, height, width), Shadow (colour, opacity, blur, distance,
    angle) each switched at its title; Spacing (letter, line); Curve.
    Visual › Basic as on pictures. *Not yet: the "Aa" presets row,
    Templates, Fonts, Effects, Bubbles, Animation; custom colours.*
  - [~] Caption: Text (Basic, Fonts, Effects, Bubbles) · Edit captions
    (the list, see Captions) · Animation. *Only Basic and Edit captions
    are built.*
  - [~] Audio (and a video's Audio tab): Volume › Volume — slider with
    100 % at its middle (to 1000 %), read-out, mute, reset (setting a
    level unmutes) — and Fade in / Fade out (0.1 s steps, each side at most
    10 s and never over the other). *No keyframed volume.*
  - [x] Audio › Beats (`ui/beats.rs`, `src/beats/`): status (`{bpm} BPM ·
    {count} beats` / Analyzing… / No reliable beats detected), Auto
    generate (the engine's `AudioBeatAnalyzer` through the shim, fed by
    our FFmpeg decode off the UI thread; one analysis per song per
    session, so a split or copy gets its beats at once; a map from another
    analyzer version re-analyzes), Detection Strong / Normal / Dense
    (from the cached `detected`, never re-decoded; picked before analysis
    too), Add / Delete beat at the playhead, Move nearest beat here (half a
    beat's reach), Reset beats, Clear beat markers. Same `beats` JSON as
    the app (source time; moved automatic beats are `override`s with
    `originalTime`). *Beyond the old sheet:* ‹ › step between beats (⌥← /
    ⌥→), M = add / delete beat (selected music clip, else the one under
    the playhead; in the menus and Shortcuts), Shift all beats a frame
    earlier / later, and the beat under the playhead lit red on the
    selected clip. Without the engine, a small Rust stand-in
    (`beats/standin.rs`) gives the same result format.
  - [x] Audio › Duration (`ui/audio_duration.rs`): slider 0.5 s → what's
    left of the song (library length, else its waveform, else its
    analysis), read-out follows the drag and lands on release (one undo
    step), from the end (start stays). Fit to video, Full song. *Beyond
    the old sheet:* Ends at (timecode) and Song left read-outs; with auto
    snapping on the slider catches the song's beats and the video's end;
    End on a beat; fades that no longer fit shrink with the clip.
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
- [~] AI agent panel in the inspector slot (`agent/`, `ui/agent.rs`):
  - [x] Providers: Ollama, LM Studio, llama.cpp, custom OpenAI-compatible;
    Claude, ChatGPT, Gemini, OpenRouter, Grok, DeepSeek, Mistral with the
    user's key (macOS keychain; never shown again). Model + vision model
    pickers, each server's picks remembered.
  - [x] Chat: requests, each tool run (label, arguments, result or
    progress), answers, errors; Stop, New chat, Undo AI edits (n); every
    tool call is one undo step.
  - [x] The old app's tools (timeline edits, text, transform, keyframes,
    filters, adjust, effects, blend, ratio, media, freeze, extract audio,
    lanes, `look`), plus what goes beyond it: `add_title` (animated titles:
    pop, slide up, typewriter, word by word, lower third, headline, neon,
    countdown), `animate` (zoom / pan / pop / slide / shake / spin as
    keyframes), `find_sounds` / `add_sound` / `sound_design` (real
    recordings from the app's library + Openverse CC0 / CC BY, placed by
    onset / peak / tail on cuts, transitions, titles, text, stickers,
    effects, typewriter runs and scene changes found in the picture).
  - [x] Small local models: macro tools in their core set, worked
    examples, forgiving arguments ("5s", "1:05", fuzzy names that list
    the closest), text tool calls, a loop stop, a nudge when an answer
    claims an edit no tool made. `agent/eval.rs` (ignored) scores a real
    model: Qwen3-VL 4B instruct (the default pick: it also does `look`,
    so no second model evicts it) and qwen3 4B instruct both pass 9/9.
  - [x] Skills (`agent/skills.rs`): a job's steps, worked examples and
    extra tools, loaded per request so a small model reaches the cloud-only
    tools one job at a time (keyframes, overlay / picture-in-picture, clip
    tools, lanes, stickers, captions, scene, colour grade, highlight reel).
    Routed by the request's words (no extra model call), or loaded by the
    model with `use_skill` (a skill called by its own name counts too).
    Instructions ride in the user message and tools are appended after the
    fixed ones, so the warmed-up prefix stays cached. The user's own skills:
    Markdown files in `<data>/skills/` (name, description, triggers, tools
    header + steps), "Open skills folder" in the setup. `eval_skills`:
    Qwen3-VL 4B 7/8 (unrouted discovery still misses one); core eval 9/9.
  - [x] Local speed: the model is warmed up (loaded, instructions + tools
    read and cached by the server) when the panel opens, at launch with
    it open, after Connect / a model pick and on the first keystroke, and
    kept loaded 30 min. A cold first request went from ~20 s to ~3.5 s on
    an M1 Pro (`eval_warm_up`; `REEL_AGENT_LOG=1` prints each call's
    load / prompt / reply times).
  - [x] Making a video from one prompt (`make_video`, `agent/tools/produce.rs`):
    the model writes the script, and per scene the narration is spoken
    (`agent/voice.rs`: OpenAI or Gemini speech with the user's key, else
    the system voice), measured, and a motion-graphic scene drawn exactly
    that long (`agent/graphics.rs` → H.264 via `agent/movie.rs`: animated
    backdrop, growing bar chart, counter, line chart, comparison, steps;
    7 palettes); its key point as an animated title, captions timed to
    the voice's pauses (`agent/captions.rs`), library stickers popping in,
    a music bed under it (looped, faded, low under the voice) and sound
    design on the cuts and titles. Tagged, so asking again replaces it.
  - [x] Videos of any length, written scene by scene (`agent/tools/board.rs`):
    `start_video` (settings + an outline, a line per scene), `write_scene`
    (one scene a call; each answer lists at most two fixes and names the
    next step), `build_video` (the scenes through `make_video`'s build).
    The storyboard lasts the chat and rides in the project state, so "change
    scene 3" is one call and a rebuild; a turn that stops before building
    is sent back once. Qwen3-VL 4B writes 5-scene, ~67 s videos with it
    (`eval_make_video`, `REEL_EVAL_DRY=1` skips the drawing).
  - [x] Composed scenes (`visual: "canvas"`, `agent/graphics/canvas.rs`):
    text, counting numbers, icons, shapes, bars, cards and joining arrows,
    each placed, timed, brought in (fade, rise, pop, grow, spin, draw, type,
    words…), moved, looped and taken out; laid out (stack / row / grid) and
    timed in turn when not said, and hand-placed words kept apart. Skills
    `video`, `motion_graphics`, `review`.
  - [x] `check_video` (`agent/tools/check.rs`): black between clips, sound
    or words past the end, two voices at once, music over the voice,
    captions or texts on top of each other, words too quick to read; each
    with the call that fixes it.
  - [x] The parts on their own: `add_voiceover` (+ captions), `add_captions`,
    `add_music`, `add_sticker`, `add_scene`. Drawn scenes join the media
    library.
  - [ ] Auto captions for the user's own speech (speech to text,
    on-device); generated pictures / footage (stock or image models);
    scene transitions (the native editor has none yet: cuts get whooshes);
    showing `look`'s frames in the chat; credits for CC BY sounds at
    export; the chat field focused when the panel opens at launch.

## Timeline toolbar

- [x] Icon buttons (28 px) with hover tooltips "Label  key"
- [x] Undo / Redo (⌘Z, ⌘⇧Z / ⌘Y), disabled when there's nothing to do
- [x] Select tool (A) / Split tool (B): the blade cuts a clicked clip at the
  click (frame-snapped); Esc goes back to Select
- [x] Delete left (Q), Delete right (W): selected clip under the playhead,
  else the main clip; trims the source window; magnet closes the gap
- [~] The selected clip's actions, per kind in the old order; nothing
  selected: Split + disabled Delete. A video: Replace, Crop, Duplicate,
  Split, Delete, Freeze, Extract audio, Overlay, Layers (a photo: no
  Freeze / Extract audio).
  - Replace: a file pick (video or photo) swapped into the slot, keeping
    the edits on it; a video too short for the slot is refused ("Pick a
    video at least 4.2s long"); crop, cutout, reversed copy, freeze and
    camera track go with the old picture.
  - Crop: the dialog (stage with box, thirds grid, 8 handles + move; a
    video's frame scrubber; Custom + 9 ratio buttons; Rotate ±20° with
    its value; Reset · Cancel · Confirm, Esc / Return), one undo step.
    *The degree box doesn't take typing yet.*
  - Freeze: a 3 s still of the frame at the playhead between the halves
    (the engine's source time), the rest of the lane moving right.
  - Extract audio: "Extracted audio N" on the audio lane, video muted.
  - Overlay: main lane → a free overlay lane (magnet closes up); overlay
    → main lane at the nearest cut (magnet off: where it is).
  - Layers: Arrange layers docked in the inspector (front-first tiles of
    what's on screen at the playhead, Front / Back, press-drag to
    re-stack, click picks; `Track.order` / `stackOrder`).
  - *Still to port: a sticker's Replace (back to the browser), and the
    tools this build can't run yet (reverse, voice, cut pauses, auto
    title, tracking).*
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
  ⌘= / ⌘- zoom around the playhead; ⇧Z fits (80 px left after the end);
  a trackpad pinch over the lanes zooms around the pointer (macOS: an
  AppKit magnify monitor, `src/pinch.rs`, since gpui drops the event)
- [x] Playhead: shield on the ruler + line; timecode readout top left;
  while playing the view pages when the playhead runs off the right edge
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
  diamonds snap to the beats (a clip not to its own). The selected
  clip's beat under the playhead is drawn red (what Delete beat removes).
- [x] Keyframe diamonds on the selected clip: one ◆ per keyframe time
  (enabled property tracks, 50 ms merged, thinned to one per 12 px), on
  the clip's middle; the one under the playhead red. Click = playhead onto
  it; drag = move it on every property track (snaps to the clip's edges,
  the playhead and the other diamonds; one undo step; a diamond already
  there gives way). *No Graphs badge, no tracked-path diamonds yet.*
- [x] Transition seams: boxes meet mid-overlap; a button between main
  clips (moves the playhead to that cut and opens the browser's
  Transitions tab)
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
- [x] Crop (`crop` box + `rotation`), freeze frame, extract audio
  (`extractedNumber`), replace media, layer order (`ReorderLayers`:
  places reshuffled, ties pulled apart) — `clip_edits.rs`, tested.
- [~] Keyframes, transforms, masks, adjust, transitions, drawings,
  captions, speed curves, reverse (baked copy). *Transitions and
  captions export (same compositor as the player).*
- [ ] Media relink for projects from the web app

## Export

- [x] Export pipeline in Rust (`src/export.rs`), on its own thread, the
  player's own path made frame-exact: engine layers at each frame time →
  a `FrameReader` per layer (exact source time; the preview's decode
  server runs on it too), text drawn at the output size, sticker frames,
  Background photo → the GPU compositor (Metal / wgpu) on a canvas the
  output's size → H.264 (VideoToolbox, Media Foundation, x264 /
  openh264 when present, MPEG-4 last) + AAC 48 kHz stereo (the playback
  mixer run offline, `audio::TimelineMix`) in an MP4 (`src/encode.rs`,
  `+faststart`). Bitrate as the engine's rule (2.5 / 8 / 16 Mbps at
  1080p30 for 720p / 1080p / 4K, scaled by pixels × fps, 0.5–50 Mbps).
  Missing effect rows / packages and stickers are fetched as frames need
  them. Written to `<file>.part`, renamed when whole; removed on cancel
  or failure. Tested end to end (`exports_a_clip`).
- [x] Export screen (`ui/export.rs`): percent title, progress ring round a
  frame of the project's shape with the frame just written, resolution ·
  size · fps; Cancel (✕, Esc, button) asks "Stop exporting?"; done: Done,
  Show in Finder, Play video, time taken; failed / cancelled: Retry,
  Close. Monochrome (the old screen's orange ring is shell accent).
- [ ] Linux release builds have no LGPL H.264 encoder (FFmpeg ships
  none): they write MPEG-4 part 2. Build libopenh264 (BSD) into
  `packaging/linux/build-ffmpeg.sh` for H.264.
- [ ] Bitrate field (custom Mbps) and estimated size from the old Export
  settings sheet.
- [ ] Not exported yet because the player doesn't draw them either:
  transitions, drawings, Background patterns, volume keyframes, sound of
  reversed / frozen clips.
- [ ] An export is lost if the app quits mid-way (its `.part` stays).

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
| Export | ⌘E | [x] |
| Shortcuts dialog | ⌘/ | [x] |

All of them come from one table (`src/shortcuts.rs`): key bindings, menu
items, tooltips and the dialog.

## Still to build next (in order)

1. Use the new text field for project rename and typed number fields.
2. Timeline leftovers: the cover editor; group move of a multi-selection;
   Graphs badge and tracked-path diamonds.
3. Inspector panels on the real document fields: Adjust; Text's
   Templates / Fonts / Effects / Bubbles / Animation.
4. Browser: Voiceover, Text templates; Transitions search; bundle
   whisper.cpp for auto captions (no Homebrew), Edit captions.
5. Export leftovers (see Export): H.264 on Linux, custom bitrate.

## Platform

- [x] macOS (Apple Silicon) builds and runs
- [ ] Windows, Linux builds (GPUI supports both; FFmpeg + engine backends)
- [ ] Khmer / Vietnamese / Chinese / Indonesian UI strings and fonts
- [x] Bundled FFmpeg (LGPL, static, decode only) instead of Homebrew dylibs
  in released apps (`packaging/macos/build-ffmpeg.sh`)
- [x] CI: fmt, clippy, tests on every pull request (`ci.yml`)
- [x] Product analytics into the phone app's PostHog project, same event
  names (`src/analytics.rs`): app / project / import / edit-derived /
  undo / export events, session summary; release builds only, Settings
  switch to opt out. *Not yet: AI agent turns, library / browser picks,
  preview performance.*
