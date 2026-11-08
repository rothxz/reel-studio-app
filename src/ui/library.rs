//! The browser's online and add-something views:
//!
//! - **Audio › Music**: moods in the side nav (or a search), a list of
//!   tracks: cover with ▶ to listen, title, artist, length; a click
//!   downloads the track and adds it on the audio lane, fitted to the
//!   picture.
//! - **Audio › Sound**: My sounds (downloaded before) and the library's
//!   categories in the side nav (or a search), the same rows.
//! - **Text › Add text**: Default text (a plain caption at the playhead,
//!   its words ready to type in the inspector).
//! - **Sticker › Stickers / GIFs**: quick picks and the API's categories
//!   (Stickers), or a search; tiles that play on hover; a click adds the
//!   sticker at the playhead.
//!
//! Lists are fetched off the UI thread as they're shown (stickers a page
//! at a time as the grid nears its end) and kept for the session.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, Context, Entity, FontWeight, MouseButton, ObjectFit, ScrollHandle, SharedString,
    div, img, prelude::*, px, rgba,
};

use crate::editor::Editor;
use crate::i18n::t;
use crate::library::{
    self, MUSIC_MOODS, MusicQuery, MusicTrack, STICKER_PICKS, SoundEffect, SoundQuery, StickerItem,
    StickerKind,
};
use crate::model::{Micros, SECOND};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::text_input::{InputEvent, TextInput};
use crate::ui::widgets::{mono, pill_button};

pub const AUDIO_TAB: usize = 1;
pub const TEXT_TAB: usize = 2;
pub const STICKER_TAB: usize = 3;

/// A row's add action.
type AddFn = Rc<dyn Fn(&mut Editor, &mut Context<Editor>)>;

/// The search boxes (one per searchable view).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Search {
    Stickers = 0,
    Gifs = 1,
    Music = 2,
    Sound = 3,
}

const SEARCH_DELAY: Duration = Duration::from_millis(400);
const TILE_GAP: f32 = 8.;
const TILE_MIN: f32 = 88.;
const INSET: f32 = 12.;

/// A list as fetched so far.
#[derive(Clone, Debug)]
pub struct Listing<T> {
    pub items: Vec<T>,
    pub loading: bool,
    pub failed: bool,
    pub pages: usize,
    pub done: bool,
}

impl<T> Default for Listing<T> {
    fn default() -> Self {
        Listing {
            items: vec![],
            loading: false,
            failed: false,
            pages: 0,
            done: false,
        }
    }
}

pub struct LibraryUi {
    pub stickers: HashMap<(StickerKind, String), Listing<StickerItem>>,
    pub sticker_categories: Listing<(String, String)>,
    pub music: HashMap<MusicQuery, Listing<MusicTrack>>,
    pub sound_categories: Listing<String>,
    pub sounds: HashMap<SoundQuery, Listing<SoundEffect>>,
    /// The search boxes, by `Search`.
    pub search: Vec<Entity<TextInput>>,
    /// Each box's search as applied (typing settles first).
    pub queries: [String; 4],
    search_tokens: [u64; 4],
    /// Tracks / sounds downloading to be added (by id).
    pub adding: HashSet<String>,
    pub scroll: ScrollHandle,
    pub hover: Option<String>,
}

impl LibraryUi {
    pub fn new(cx: &mut Context<Editor>) -> Self {
        let placeholders = [
            "Search stickers…",
            "Search GIFs…",
            "Search music…",
            "Search sound effects…",
        ];
        let search: Vec<Entity<TextInput>> = placeholders
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let input = cx.new(|cx| TextInput::new(*p, false, cx).max_len(80));
                cx.subscribe(&input, move |ed: &mut Editor, _, ev: &InputEvent, cx| {
                    ed.on_search(i, ev, cx)
                })
                .detach();
                input
            })
            .collect();
        LibraryUi {
            stickers: HashMap::new(),
            sticker_categories: Listing::default(),
            music: HashMap::new(),
            sound_categories: Listing::default(),
            sounds: HashMap::new(),
            search,
            queries: Default::default(),
            search_tokens: [0; 4],
            adding: HashSet::new(),
            scroll: ScrollHandle::new(),
            hover: None,
        }
    }

    /// The Stickers view's categories: the quick picks, then the API's own
    /// (those the picks don't already cover). (label, query; None: trending.)
    pub fn sticker_picks(&self) -> Vec<(String, Option<String>)> {
        let mut out: Vec<(String, Option<String>)> = STICKER_PICKS
            .iter()
            .map(|(l, q)| (l.to_string(), q.map(str::to_string)))
            .collect();
        let taken: HashSet<String> = out
            .iter()
            .filter_map(|(_, q)| q.as_ref().map(|q| q.to_lowercase()))
            .collect();
        for (label, query) in &self.sticker_categories.items {
            if !taken.contains(&query.to_lowercase()) {
                let mut l = label.clone();
                if let Some(c) = l.get_mut(0..1) {
                    c.make_ascii_uppercase();
                }
                out.push((l, Some(query.clone())));
            }
        }
        out
    }

    /// Sound's categories: My sounds, then the library's.
    pub fn sound_tabs(&self) -> Vec<String> {
        let mut out = vec!["My sounds".to_string()];
        out.extend(self.sound_categories.items.iter().cloned());
        out
    }
}

/// "M:SS" for a track's length.
fn minutes(secs: f64) -> String {
    let s = secs.max(0.).round() as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// What a library view shows now.
enum Shown {
    Stickers(StickerKind, String),
    Music(MusicQuery),
    Sounds(Option<SoundQuery>),
}

impl Editor {
    /// A view's categories for the side nav (labels), else none.
    pub fn view_categories(&self, tab: usize, view: usize) -> Vec<SharedString> {
        use crate::ui::browser::{EFFECTS_TAB, FILTER_TAB};
        fn labels(list: &'static [(&'static str, &'static str)]) -> Vec<SharedString> {
            list.iter().map(|(_, l)| SharedString::from(*l)).collect()
        }
        match (tab, view) {
            (EFFECTS_TAB, 0) => labels(crate::catalog::EFFECT_TABS),
            (FILTER_TAB, 0) => labels(crate::catalog::FILTER_TABS),
            (AUDIO_TAB, 0) => MUSIC_MOODS
                .iter()
                .map(|(l, _)| SharedString::from(*l))
                .collect(),
            (AUDIO_TAB, 1) => self
                .lib
                .sound_tabs()
                .into_iter()
                .map(SharedString::from)
                .collect(),
            (STICKER_TAB, 0) => self
                .lib
                .sticker_picks()
                .into_iter()
                .map(|(l, _)| SharedString::from(l))
                .collect(),
            _ => vec![],
        }
    }

    /// A search is showing in this view (its category isn't).
    pub fn view_searching(&self, tab: usize, view: usize) -> bool {
        let which = match (tab, view) {
            (STICKER_TAB, 0) => Search::Stickers,
            (AUDIO_TAB, 0) => Search::Music,
            (AUDIO_TAB, 1) => Search::Sound,
            _ => return false,
        };
        !self.lib.queries[which as usize].is_empty()
    }

    /// The category a view opens on.
    pub fn default_category(tab: usize, view: usize) -> usize {
        // Sound opens on its first category, after My sounds.
        if (tab, view) == (AUDIO_TAB, 1) { 1 } else { 0 }
    }

    /// A search box's typing: applied once it settles; Enter applies now.
    fn on_search(&mut self, which: usize, ev: &InputEvent, cx: &mut Context<Self>) {
        let text = match ev {
            InputEvent::Changed(t) | InputEvent::Submit(t) => t.trim().to_string(),
            InputEvent::Blur => return,
        };
        self.lib.search_tokens[which] += 1;
        let token = self.lib.search_tokens[which];
        if matches!(ev, InputEvent::Submit(_)) || text.is_empty() {
            self.apply_search(which, text, cx);
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DELAY).await;
            this.update(cx, |ed, cx| {
                if ed.lib.search_tokens[which] == token {
                    ed.apply_search(which, text, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// A side-nav category was picked: the view's search box empties.
    pub fn clear_view_search(&mut self, tab: usize, cx: &mut Context<Self>) {
        let which = match (tab, self.browser_views.get(tab).copied().unwrap_or(0)) {
            (STICKER_TAB, 0) => Search::Stickers,
            (AUDIO_TAB, 0) => Search::Music,
            (AUDIO_TAB, 1) => Search::Sound,
            _ => return,
        } as usize;
        if self.lib.queries[which].is_empty() && self.lib.search[which].read(cx).text().is_empty() {
            return;
        }
        self.lib.search[which].update(cx, |f, cx| f.set_text("", cx));
        self.lib.search_tokens[which] += 1;
        self.apply_search(which, String::new(), cx);
    }

    fn apply_search(&mut self, which: usize, text: String, cx: &mut Context<Self>) {
        if self.lib.queries[which] != text {
            self.lib.queries[which] = text;
            self.lib.scroll.set_offset(gpui::Point::default());
        }
        cx.notify();
    }

    /// The shown library view's list, if one is.
    fn shown_library(&self) -> Option<Shown> {
        let tab = self.browser_tab;
        let view = self.browser_views.get(tab).copied().unwrap_or(0);
        let category = self.browser_categories.get(tab).copied().unwrap_or(0);
        let q = |s: Search| self.lib.queries[s as usize].clone();
        match (tab, view) {
            (STICKER_TAB, 0) => {
                let query = q(Search::Stickers);
                let query = if query.is_empty() {
                    self.lib
                        .sticker_picks()
                        .get(category)
                        .and_then(|(_, q)| q.clone())
                        .unwrap_or_default()
                } else {
                    query
                };
                Some(Shown::Stickers(StickerKind::Stickers, query))
            }
            (STICKER_TAB, 1) => Some(Shown::Stickers(StickerKind::Gifs, q(Search::Gifs))),
            (AUDIO_TAB, 0) => {
                let query = q(Search::Music);
                Some(Shown::Music(if query.is_empty() {
                    MusicQuery::Mood(MUSIC_MOODS[category.min(MUSIC_MOODS.len() - 1)].1)
                } else {
                    MusicQuery::Search(query)
                }))
            }
            (AUDIO_TAB, 1) => {
                let query = q(Search::Sound);
                Some(Shown::Sounds(if !query.is_empty() {
                    Some(SoundQuery::Search(query))
                } else if category == 0 {
                    None // My sounds
                } else if let Some(c) = self.lib.sound_categories.items.get(category - 1) {
                    Some(SoundQuery::Category(c.clone()))
                } else if self.lib.sound_categories.done
                    && self.lib.sound_categories.items.is_empty()
                {
                    Some(SoundQuery::Newest)
                } else {
                    return None; // categories still coming
                }))
            }
            _ => None,
        }
    }

    /// Fetches what the shown library view needs (each render; a no-op
    /// when it's here or on its way).
    pub fn prefetch_library(&mut self, cx: &mut Context<Self>) {
        if self.screen != crate::editor::Screen::Editor {
            return;
        }
        match self.browser_tab {
            STICKER_TAB => {
                let c = &self.lib.sticker_categories;
                if !c.loading && !c.done && !c.failed {
                    self.lib.sticker_categories.loading = true;
                    self.lib_job(
                        library::sticker_categories,
                        |ed, r, _| {
                            let c = &mut ed.lib.sticker_categories;
                            c.loading = false;
                            c.done = true;
                            match r {
                                Ok(items) => c.items = items,
                                Err(e) => {
                                    eprintln!("sticker categories: {e:#}");
                                    c.failed = true;
                                }
                            }
                        },
                        cx,
                    );
                }
            }
            AUDIO_TAB => {
                let c = &self.lib.sound_categories;
                if !c.loading && !c.done && !c.failed {
                    self.lib.sound_categories.loading = true;
                    self.lib_job(
                        library::sound_categories,
                        |ed, r, _| {
                            let c = &mut ed.lib.sound_categories;
                            c.loading = false;
                            c.done = true;
                            match r {
                                Ok(items) => c.items = items,
                                Err(e) => {
                                    eprintln!("sound categories: {e:#}");
                                    c.failed = true;
                                }
                            }
                        },
                        cx,
                    );
                }
            }
            _ => {}
        }
        let near_end = {
            let max = f32::from(self.lib.scroll.max_offset().height);
            let at = -f32::from(self.lib.scroll.offset().y);
            max - at < 400.
        };
        match self.shown_library() {
            Some(Shown::Stickers(kind, query)) => {
                let key = (kind, query.clone());
                let l = self.lib.stickers.entry(key.clone()).or_default();
                let start = l.pages == 0 || (near_end && !l.done);
                if l.loading || l.failed || !start {
                    return;
                }
                l.loading = true;
                let page = l.pages + 1;
                self.lib_job(
                    move || {
                        library::stickers(
                            kind,
                            Some(&query).filter(|q| !q.is_empty()).map(|q| q.as_str()),
                            page,
                        )
                    },
                    move |ed, r, _| {
                        let l = ed.lib.stickers.entry(key).or_default();
                        l.loading = false;
                        match r {
                            Ok(items) => {
                                l.pages = page;
                                l.done = items.len() < library::PAGE_SIZE / 2;
                                let seen: HashSet<String> =
                                    l.items.iter().map(|i| i.id.clone()).collect();
                                l.items
                                    .extend(items.into_iter().filter(|i| !seen.contains(&i.id)));
                            }
                            Err(e) => {
                                eprintln!("stickers: {e:#}");
                                l.failed = true;
                            }
                        }
                    },
                    cx,
                );
            }
            Some(Shown::Music(query)) => {
                let l = self.lib.music.entry(query.clone()).or_default();
                if l.loading || l.done || l.failed {
                    return;
                }
                l.loading = true;
                let q = query.clone();
                self.lib_job(
                    move || library::music(&q),
                    move |ed, r, _| {
                        let l = ed.lib.music.entry(query).or_default();
                        l.loading = false;
                        l.done = true;
                        match r {
                            Ok(items) => l.items = items,
                            Err(e) => {
                                eprintln!("music: {e:#}");
                                l.failed = true;
                            }
                        }
                    },
                    cx,
                );
            }
            Some(Shown::Sounds(Some(query))) => {
                let l = self.lib.sounds.entry(query.clone()).or_default();
                if l.loading || l.done || l.failed {
                    return;
                }
                l.loading = true;
                let q = query.clone();
                self.lib_job(
                    move || library::sounds(&q),
                    move |ed, r, _| {
                        let l = ed.lib.sounds.entry(query).or_default();
                        l.loading = false;
                        l.done = true;
                        match r {
                            Ok(items) => l.items = items,
                            Err(e) => {
                                eprintln!("sound effects: {e:#}");
                                l.failed = true;
                            }
                        }
                    },
                    cx,
                );
            }
            _ => {}
        }
    }

    /// Runs `job` off the UI thread, then `done` with its result.
    fn lib_job<T: Send + 'static>(
        &mut self,
        job: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Editor, anyhow::Result<T>, &mut Context<Editor>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let task = cx.background_executor().spawn(async move { job() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |ed, cx| {
                done(ed, result, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks the shown view to fetch its list again (Retry).
    fn retry_library(&mut self, cx: &mut Context<Self>) {
        match self.shown_library() {
            Some(Shown::Stickers(kind, q)) => {
                if let Some(l) = self.lib.stickers.get_mut(&(kind, q)) {
                    l.failed = false;
                }
            }
            Some(Shown::Music(q)) => {
                self.lib.music.remove(&q);
            }
            Some(Shown::Sounds(Some(q))) => {
                self.lib.sounds.remove(&q);
            }
            _ => {}
        }
        self.lib.sticker_categories.failed = false;
        self.lib.sound_categories.failed = false;
        cx.notify();
    }

    // ---- adding -------------------------------------------------------------------

    /// A picked sticker / GIF: a new clip at the playhead, selected.
    pub fn add_sticker(&mut self, item: &StickerItem, cx: &mut Context<Self>) {
        let at = self.playhead;
        let mut id = String::new();
        self.edit(
            "Add Sticker",
            false,
            None,
            |p| {
                id = p.add_sticker_clip(at, &item.url, item.width, item.height);
                true
            },
            cx,
        );
        self.want_sticker(item.url.clone(), cx);
        self.selected.set(id);
        self.inspector_entry = None;
        cx.notify();
    }

    /// Default text: a plain caption at the playhead, selected, its words
    /// selected in the inspector so typing replaces them.
    pub fn add_default_text(&mut self, cx: &mut Context<Self>) {
        let at = self.playhead;
        let mut id = String::new();
        self.edit(
            "Add Text",
            false,
            None,
            |p| {
                id = p.add_text_clip(at, "Default text");
                true
            },
            cx,
        );
        self.selected.set(id);
        self.inspector_entry = Some("text:basic");
        self.focus_text_field = true;
        cx.notify();
    }

    /// A library sound: downloaded (kept for the project), measured, then
    /// added on the audio lane fitted to the picture, selected.
    fn add_library_sound(
        &mut self,
        id: String,
        title: String,
        download: impl FnOnce() -> anyhow::Result<std::path::PathBuf> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if !self.lib.adding.insert(id.clone()) {
            return;
        }
        if let Some(a) = self.audio.as_mut() {
            a.stop_preview();
        }
        cx.notify();
        self.lib_job(
            move || {
                let path = download()?;
                let item = crate::decoder::probe(&path.to_string_lossy())?;
                Ok((path, item.duration))
            },
            move |ed, r, cx| {
                ed.lib.adding.remove(&id);
                match r {
                    Ok((path, length)) => {
                        let path = path.to_string_lossy().into_owned();
                        let at = ed.playhead;
                        let length: Micros = length.max(SECOND);
                        let mut clip = String::new();
                        ed.edit(
                            "Add Audio",
                            false,
                            None,
                            |p| {
                                clip = p.add_audio_clip_fitted(at, &path, &title, length);
                                true
                            },
                            cx,
                        );
                        ed.selected.set(clip);
                        ed.inspector_entry = None;
                    }
                    Err(e) => {
                        eprintln!("library sound {id}: {e:#}");
                        ed.toast("Couldn't download that sound", cx);
                    }
                }
            },
            cx,
        );
    }

    pub fn add_music(&mut self, track: &MusicTrack, cx: &mut Context<Self>) {
        let t = track.clone();
        self.add_library_sound(
            format!("music:{}", track.id),
            track.clip_title(),
            move || library::download_track(&t),
            cx,
        );
    }

    pub fn add_sound(&mut self, effect: &SoundEffect, cx: &mut Context<Self>) {
        let e = effect.clone();
        self.add_library_sound(
            format!("sound:{}", effect.id),
            effect.name.clone(),
            move || library::download_sound(&e),
            cx,
        );
    }

    /// ▶ on a row: listens (again: stops).
    fn toggle_listen(&mut self, id: String, source: Option<String>, cx: &mut Context<Self>) {
        let Some(source) = source else {
            return;
        };
        match self.audio.as_mut() {
            Some(a) => a.toggle_preview(&id, &source),
            None => self.toast("No sound output", cx),
        }
        // The icon turns back when a preview ends on its own.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let Ok(playing) = this.update(cx, |ed, cx| {
                    let p = ed.audio.as_ref().and_then(|a| a.previewing());
                    if p.is_none() {
                        cx.notify();
                    }
                    p.is_some()
                }) else {
                    return;
                };
                if !playing {
                    return;
                }
            }
        })
        .detach();
        cx.notify();
    }

    // ---- views -----------------------------------------------------------------------

    /// The search box over a view.
    fn search_box(&self, which: Search, cx: &mut Context<Self>) -> AnyElement {
        let input = self.lib.search[which as usize].clone();
        let has_text = !input.read(cx).text().is_empty();
        div()
            .flex_none()
            .mx(px(INSET))
            .mt(px(10.))
            .h(px(30.))
            .px(px(8.))
            .rounded(px(6.))
            .bg(theme::lane_bg())
            .border_1()
            .border_color(theme::chip_border())
            .flex()
            .items_center()
            .gap(px(6.))
            .child(Icon::Search.el(13., theme::text_tertiary()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(theme::XS))
                    .line_height(theme::line_height(16., theme::XS))
                    .text_color(theme::text())
                    .child(input.clone()),
            )
            .when(has_text, |d| {
                d.child(
                    div()
                        .id(("search-clear", which as usize))
                        .cursor_pointer()
                        .on_click(cx.listener(move |ed, _, _, cx| {
                            ed.lib.search[which as usize].update(cx, |f, cx| f.set_text("", cx));
                            ed.lib.search_tokens[which as usize] += 1;
                            ed.apply_search(which as usize, String::new(), cx);
                        }))
                        .child(Icon::Close.el(13., theme::text_tertiary())),
                )
            })
            .into_any_element()
    }

    /// A view's heading under its search box.
    fn view_heading(label: impl Into<SharedString>) -> AnyElement {
        div()
            .flex_none()
            .h(px(34.))
            .px(px(INSET))
            .flex()
            .items_end()
            .pb(px(4.))
            .text_size(px(theme::SM))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::text())
            .child(crate::i18n::ts(label))
            .into_any_element()
    }

    /// A centred note (loading, nothing found, offline with Retry).
    fn list_message<T>(
        &self,
        l: Option<&Listing<T>>,
        empty: &str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let note = |text: &str| {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .px(px(16.))
                .child(
                    div()
                        .text_size(px(theme::XS))
                        .text_color(theme::text_tertiary())
                        .text_center()
                        .child(text.to_string()),
                )
        };
        match l {
            Some(l) if !l.items.is_empty() => None,
            Some(l) if l.failed => Some(
                note("Couldn't load it. Check your connection and try again.")
                    .child(
                        pill_button("lib-retry", "Retry", false)
                            .on_click(cx.listener(|ed, _, _, cx| ed.retry_library(cx))),
                    )
                    .into_any_element(),
            ),
            Some(l) if l.done => Some(note(empty).into_any_element()),
            _ => Some(note("Loading…").into_any_element()),
        }
    }

    /// Stickers / GIFs: search, heading, tiles.
    pub fn render_sticker_view(&self, kind: StickerKind, cx: &mut Context<Self>) -> AnyElement {
        let which = if kind == StickerKind::Stickers {
            Search::Stickers
        } else {
            Search::Gifs
        };
        let query = self.lib.queries[which as usize].clone();
        let heading = if !query.is_empty() {
            format!("Results for “{query}”")
        } else if kind == StickerKind::Gifs {
            "Trending".into()
        } else {
            let i = self.browser_categories[STICKER_TAB];
            self.lib
                .sticker_picks()
                .get(i)
                .map_or("Stickers".into(), |(l, _)| l.clone())
        };
        let key = match self.shown_library() {
            Some(Shown::Stickers(k, q)) => Some((k, q)),
            _ => None,
        };
        let listing = key.as_ref().and_then(|k| self.lib.stickers.get(k));
        let body = match self.list_message(listing, "No stickers found", cx) {
            Some(m) => m,
            None => {
                let l = listing.expect("a list with items");
                let width = self
                    .measured("sticker-grid")
                    .map_or(0., |b| f32::from(b.size.width) - INSET * 2.);
                let columns = ((width + TILE_GAP) / (TILE_MIN + TILE_GAP)).floor().max(3.);
                let size = if width > 0. {
                    ((width - TILE_GAP * (columns - 1.)) / columns).floor()
                } else {
                    TILE_MIN
                };
                div()
                    .id("sticker-grid")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.lib.scroll)
                    .child(self.measure("sticker-grid"))
                    .child(
                        div()
                            .p(px(INSET))
                            .flex()
                            .flex_wrap()
                            .gap(px(TILE_GAP))
                            .children(l.items.iter().map(|item| self.sticker_tile(item, size, cx))),
                    )
                    .when(l.loading, |d| {
                        d.child(
                            div()
                                .pb(px(12.))
                                .flex()
                                .justify_center()
                                .text_size(px(theme::XS))
                                .text_color(theme::text_tertiary())
                                .child(t("Loading…")),
                        )
                    })
                    .into_any_element()
            }
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.search_box(which, cx))
            .child(Self::view_heading(heading))
            .child(body)
            .into_any_element()
    }

    fn sticker_tile(&self, item: &StickerItem, size: f32, cx: &mut Context<Self>) -> AnyElement {
        let hover_id = format!("sticker:{}", item.id);
        let hovered = self.lib.hover.as_deref() == Some(hover_id.as_str());
        self.want_picture(&item.preview_url, false);
        let still = self.stills.get(&item.preview_url).cloned().flatten();
        let anim = hovered
            .then(|| {
                self.want_picture(&item.preview_url, true);
                self.anim(&item.preview_url)
            })
            .flatten();
        let pick = item.clone();
        let id = SharedString::from(hover_id.clone());
        div()
            .id(id.clone())
            .relative()
            .flex_none()
            .size(px(size))
            .rounded(px(6.))
            .overflow_hidden()
            .bg(theme::button_bg())
            .border(px(1.5))
            .border_color(rgba(0xffffff12))
            .hover(|s| s.border_color(rgba(0xffffff59)))
            .cursor_pointer()
            .on_hover(cx.listener(move |ed, on: &bool, _, cx| {
                if *on {
                    ed.lib.hover = Some(hover_id.clone());
                } else if ed.lib.hover.as_deref() == Some(hover_id.as_str()) {
                    ed.lib.hover = None;
                }
                cx.notify();
            }))
            .on_click(cx.listener(move |ed, _, _, cx| ed.add_sticker(&pick, cx)))
            .map(|d| match anim.or(still) {
                Some(image) => d.child(
                    div().absolute().inset(px(6.)).child(
                        img(image)
                            .absolute()
                            .inset_0()
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    ),
                ),
                None => d,
            })
            .child(
                div()
                    .absolute()
                    .right(px(5.))
                    .bottom(px(5.))
                    .size(px(20.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme::text())
                    .when(!hovered, |d| d.invisible())
                    .child(Icon::Add.el(14., theme::app_bg())),
            )
            .into_any_element()
    }

    /// Music: search, heading, track rows.
    pub fn render_music_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self.lib.queries[Search::Music as usize].clone();
        let shown = match self.shown_library() {
            Some(Shown::Music(q)) => Some(q),
            _ => None,
        };
        let heading = match &shown {
            Some(MusicQuery::Search(q)) => format!("Results for “{q}”"),
            Some(MusicQuery::Mood(tag)) => MUSIC_MOODS
                .iter()
                .find(|(_, t)| t == tag)
                .map_or("Music".into(), |(l, _)| l.to_string()),
            _ => "Music".into(),
        };
        let _ = query;
        let listing = shown.as_ref().and_then(|q| self.lib.music.get(q));
        let playing = self.audio.as_ref().and_then(|a| a.previewing());
        let body = match self.list_message(listing, "Nothing found", cx) {
            Some(m) => m,
            None => {
                let l = listing.expect("a list with items");
                div()
                    .id("music-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.lib.scroll)
                    .px(px(INSET - 4.))
                    .pb(px(12.))
                    .flex()
                    .flex_col()
                    .children(l.items.iter().map(|t| {
                        let id = format!("music:{}", t.id);
                        self.sound_row(
                            id.clone(),
                            t.title.clone(),
                            t.artist.clone(),
                            Some(minutes(t.duration_secs)),
                            (!t.image_url.is_empty()).then(|| t.image_url.clone()),
                            playing.as_deref() == Some(id.as_str()),
                            Some(t.audio_url.clone()),
                            {
                                let t = t.clone();
                                Rc::new(move |ed: &mut Editor, cx: &mut Context<Editor>| {
                                    ed.add_music(&t, cx)
                                })
                            },
                            cx,
                        )
                    }))
                    .into_any_element()
            }
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.search_box(Search::Music, cx))
            .child(Self::view_heading(heading))
            .child(body)
            .into_any_element()
    }

    /// Sound effects: search, heading, rows (My sounds: what's downloaded).
    pub fn render_sound_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let shown = match self.shown_library() {
            Some(Shown::Sounds(q)) => Some(q),
            _ => None,
        };
        let heading = match &shown {
            Some(Some(SoundQuery::Search(q))) => format!("Results for “{q}”"),
            Some(Some(SoundQuery::Category(c))) => c.clone(),
            Some(None) => "My sounds".into(),
            _ => "Sound effects".into(),
        };
        let mine;
        let listing: Option<&Listing<SoundEffect>> = match &shown {
            Some(Some(q)) => self.lib.sounds.get(q),
            Some(None) => {
                mine = Listing {
                    items: library::my_sounds(),
                    done: true,
                    ..Listing::default()
                };
                Some(&mine)
            }
            None => None,
        };
        let playing = self.audio.as_ref().and_then(|a| a.previewing());
        let empty = if matches!(shown, Some(None)) {
            "Sound effects you download will show up here"
        } else {
            "Nothing found"
        };
        let body = match self.list_message(listing, empty, cx) {
            Some(m) => m,
            None => {
                let l = listing.expect("a list with items");
                div()
                    .id("sound-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.lib.scroll)
                    .px(px(INSET - 4.))
                    .pb(px(12.))
                    .flex()
                    .flex_col()
                    .children(l.items.iter().map(|e| {
                        let id = format!("sound:{}", e.id);
                        let source = library::downloaded_sound(e)
                            .map(|p| p.to_string_lossy().into_owned())
                            .or_else(|| library::sound_url(e));
                        self.sound_row(
                            id.clone(),
                            e.name.clone(),
                            e.category.clone(),
                            None,
                            None,
                            playing.as_deref() == Some(id.as_str()),
                            source,
                            {
                                let e = e.clone();
                                Rc::new(move |ed: &mut Editor, cx: &mut Context<Editor>| {
                                    ed.add_sound(&e, cx)
                                })
                            },
                            cx,
                        )
                    }))
                    .into_any_element()
            }
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.search_box(Search::Sound, cx))
            .child(Self::view_heading(heading))
            .child(body)
            .into_any_element()
    }

    /// A track / sound row: ▶ (over its cover), title, subtitle, length,
    /// ⊕. The row adds it; ▶ listens.
    #[allow(clippy::too_many_arguments)]
    fn sound_row(
        &self,
        id: String,
        title: String,
        subtitle: String,
        length: Option<String>,
        cover: Option<String>,
        playing: bool,
        source: Option<String>,
        add: AddFn,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let adding = self.lib.adding.contains(&id);
        let cover_image = cover.as_deref().and_then(|u| {
            self.want_picture(u, false);
            self.stills.get(u).cloned().flatten()
        });
        let listen_id = id.clone();
        div()
            .id(SharedString::from(format!("row:{id}")))
            .h(px(52.))
            .px(px(4.))
            .flex()
            .items_center()
            .gap(px(10.))
            .rounded(px(6.))
            .cursor_pointer()
            .group("row")
            .hover(|s| s.bg(theme::fill_subtle()))
            .on_click(cx.listener(move |ed, _, _, cx| add(ed, cx)))
            .child(
                div()
                    .id(SharedString::from(format!("listen:{id}")))
                    .relative()
                    .flex_none()
                    .size(px(40.))
                    .rounded(px(5.))
                    .overflow_hidden()
                    .bg(theme::button_bg())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |ed, _, _, cx| {
                        cx.stop_propagation();
                        ed.toggle_listen(listen_id.clone(), source.clone(), cx);
                    }))
                    .when_some(cover_image, |d, image| {
                        d.child(
                            img(image)
                                .absolute()
                                .inset_0()
                                .size_full()
                                .object_fit(ObjectFit::Cover),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgba(0x00000040))
                            .hover(|s| s.bg(rgba(0x00000080)))
                            .child(
                                if playing { Icon::Pause } else { Icon::Play }
                                    .el(16., theme::text()),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(theme::SM))
                            .text_color(theme::text())
                            .child(crate::i18n::ts(title)),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(theme::XS))
                            .text_color(theme::text_tertiary())
                            .child(subtitle),
                    ),
            )
            .when_some(length, |d, l| {
                d.child(mono(l, theme::XS, theme::text_tertiary()))
            })
            .child(
                div()
                    .flex_none()
                    .size(px(22.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .map(|d| {
                        if adding {
                            d.child(
                                div()
                                    .text_size(px(theme::XS))
                                    .text_color(theme::text_tertiary())
                                    .child("…"),
                            )
                        } else {
                            d.bg(theme::text())
                                .invisible()
                                .group_hover("row", |s| s.visible())
                                .child(Icon::Add.el(15., theme::app_bg()))
                        }
                    }),
            )
            .into_any_element()
    }

    /// Text › Add text: the Default text tile.
    pub fn render_add_text_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let tile = |id: &'static str, label: &'static str, icon: Option<Icon>| {
            div()
                .id(id)
                .relative()
                .flex_none()
                .size(px(100.))
                .rounded(px(8.))
                .p(px(8.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .bg(theme::button_bg())
                .border(px(1.5))
                .border_color(gpui::transparent_black())
                .hover(|s| s.border_color(rgba(0xffffff59)))
                .cursor_pointer()
                .group("add-tile")
                .when_some(icon, |d, i| d.child(i.el(22., theme::text())))
                .child(
                    div()
                        .text_size(px(theme::SM))
                        .text_color(theme::text())
                        .text_center()
                        .child(crate::i18n::ts(label)),
                )
                .child(
                    div()
                        .absolute()
                        .right(px(5.))
                        .bottom(px(5.))
                        .size(px(20.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(rgba(0x00000099))
                        .invisible()
                        .group_hover("add-tile", |s| s.visible())
                        .child(Icon::Add.el(14., theme::text())),
                )
        };
        div()
            .size_full()
            .p(px(16.))
            .flex()
            .flex_wrap()
            .content_start()
            .gap(px(8.))
            .child(
                tile("add-default-text", "Default text", None)
                    .on_click(cx.listener(|ed, _, _, cx| ed.add_default_text(cx))),
            )
            .into_any_element()
    }
}
