//! The AI agent panel (in the inspector's slot, opened from the header)
//! and the turn loop that drives it.
//!
//! A turn runs as one task: the model call on a background thread, each
//! tool on the UI thread against the editor (one undo step per call), a
//! slow tool's job (downloads, frames, a vision call) on a background
//! thread again. Stop and New chat bump the turn number, so a late answer
//! from an old turn is dropped.
//!
//! The panel has two faces: the setup (provider, address or API key,
//! models) and the chat (requests, each tool run with its result, the
//! answers, Undo AI edits).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui::{
    AnyElement, AsyncApp, Context, FontWeight, MouseButton, ScrollHandle, WeakEntity, div,
    prelude::*, px,
};
use serde_json::{Map, Value};

use crate::agent::llm::{self, Message, Server};
use crate::agent::providers::{PRESETS, preset};
use crate::agent::skills::{self, Loaded, Skill};
use crate::agent::snapshot::{SnapshotContext, snapshot};
use crate::agent::tools::{self, Env, Finish, Outcome, Progress, ToolResult};
use crate::agent::{self, Item, Status, keys};
use crate::editor::{Editor, PopupItem};
use crate::i18n::t;
use crate::model::Micros;
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::text_input::{InputEvent, TextInput};
use crate::ui::widgets::{chip, icon_button, panel_note, pill_button};

/// The panel's fields and scroll.
pub struct AgentUi {
    pub input: gpui::Entity<TextInput>,
    pub address: gpui::Entity<TextInput>,
    pub key: gpui::Entity<TextInput>,
    pub scroll: ScrollHandle,
    /// The request box's own scroll, once it's taller than its most lines.
    pub input_scroll: ScrollHandle,
}

impl AgentUi {
    pub fn new(cx: &mut Context<Editor>) -> Self {
        // Several lines: Enter sends, Shift+Enter breaks the line.
        let input = cx.new(|cx| {
            TextInput::new("Ask for an edit…", true, cx)
                .submit_on_enter()
                .max_len(2000)
        });
        cx.subscribe(
            &input,
            |ed: &mut Editor, input, ev: &InputEvent, cx| match ev {
                InputEvent::Submit(text) => ed.agent_send(text.clone(), cx),
                InputEvent::Changed(_) => {
                    // Typing at the end keeps the last line in view.
                    if input.read(cx).caret_at_end() {
                        ed.agent_ui.input_scroll.scroll_to_bottom();
                    }
                    // The model loads while the request is typed.
                    ed.agent_warm_up(cx)
                }
                InputEvent::Blur => {}
            },
        )
        .detach();
        let address = cx.new(|cx| TextInput::new("http://localhost:11434", false, cx).max_len(300));
        cx.subscribe(&address, |ed: &mut Editor, _, ev: &InputEvent, cx| {
            if let InputEvent::Submit(_) = ev {
                ed.agent_connect(cx);
            }
        })
        .detach();
        let key = cx.new(|cx| TextInput::new("API key", false, cx).max_len(400));
        cx.subscribe(&key, |ed: &mut Editor, _, ev: &InputEvent, cx| {
            if let InputEvent::Submit(_) = ev {
                ed.agent_save_key(cx);
            }
        })
        .detach();
        AgentUi {
            input,
            address,
            key,
            scroll: ScrollHandle::new(),
            input_scroll: ScrollHandle::new(),
        }
    }
}

/// The request box's line height and how many lines it grows to.
const REQUEST_LINE: f32 = 16.;
const REQUEST_MAX_LINES: f32 = 6.;

const SUGGESTIONS: [&str; 4] = [
    "Make a 60-second explainer about saving money, with voiceover and captions",
    "Add sound effects to the whole video",
    "Put an animated title “My trip” at the start",
    "What happens in my video?",
];

/// What a finished turn left.
enum TurnEnd {
    Replied,
    Stopped,
    Failed(String),
    /// The editor went away.
    Gone,
}

struct TurnInput {
    server: Server,
    system: Message,
    history: Vec<Message>,
    request: Message,
    /// Every skill (the system prompt lists them) and the request's own.
    skills: Vec<Skill>,
    loaded: Loaded,
    max_rounds: usize,
    cloud: bool,
    stop: Arc<AtomicBool>,
}

/// What a tool run asked of the editor, once its borrows are let go.
struct Applied {
    project: crate::model::Project,
    changed: Option<&'static str>,
    magnet: bool,
    seek_to: Option<Micros>,
    select: Option<Option<String>>,
    effect_names: Vec<String>,
    library_add: Vec<crate::model::MediaItem>,
}

impl Applied {
    fn from(env: Env) -> Self {
        Applied {
            project: env.project,
            changed: env.changed,
            magnet: env.magnet,
            seek_to: env.seek_to,
            select: env.select,
            effect_names: env.effect_names,
            library_add: env.library_add,
        }
    }
}

impl Editor {
    // ---- tools on the editor ------------------------------------------------------

    fn agent_env_parts(&self) -> (Vec<String>, Option<Server>) {
        let mut names = self.agent.effect_names.clone();
        names.extend(
            self.effects
                .entries
                .values()
                .filter(|e| e.enabled)
                .map(|e| e.name.clone()),
        );
        names.sort();
        names.dedup();
        (names, self.agent.settings.vision_server())
    }

    /// Runs one tool call against the project (its edit is one undo step).
    pub fn agent_run_tool(
        &mut self,
        name: &str,
        args: Map<String, Value>,
        cx: &mut Context<Self>,
    ) -> ToolResult<Outcome> {
        let (effect_names, vision) = self.agent_env_parts();
        let engine = &self.engine;
        let layers = |t: Micros| engine.active_layers(t);
        let mut env = Env {
            project: self.project.clone(),
            aliases: &mut self.agent.aliases,
            sounds: &mut self.agent.sounds,
            board: &mut self.agent.board,
            media: &self.media,
            playhead: self.playhead,
            selected: self.selected.primary().map(String::from),
            effect_names,
            layers_at: &layers,
            vision,
            changed: None,
            magnet: false,
            seek_to: None,
            select: None,
            library_add: vec![],
        };
        let out = tools::run(name, args, &mut env);
        let applied = Applied::from(env);
        self.agent_apply(applied, cx);
        out
    }

    /// The UI-thread half of a background tool.
    fn agent_finish_tool(&mut self, finish: Finish, cx: &mut Context<Self>) -> ToolResult {
        let (effect_names, vision) = self.agent_env_parts();
        let engine = &self.engine;
        let layers = |t: Micros| engine.active_layers(t);
        let mut env = Env {
            project: self.project.clone(),
            aliases: &mut self.agent.aliases,
            sounds: &mut self.agent.sounds,
            board: &mut self.agent.board,
            media: &self.media,
            playhead: self.playhead,
            selected: self.selected.primary().map(String::from),
            effect_names,
            layers_at: &layers,
            vision,
            changed: None,
            magnet: false,
            seek_to: None,
            select: None,
            library_add: vec![],
        };
        let out = finish(&mut env);
        let applied = Applied::from(env);
        self.agent_apply(applied, cx);
        out
    }

    fn agent_apply(&mut self, a: Applied, cx: &mut Context<Self>) {
        for item in a.library_add {
            self.add_to_library(item, cx);
        }
        if a.effect_names.len() > self.agent.effect_names.len() {
            self.agent.effect_names = a.effect_names;
        }
        if let Some(label) = a.changed {
            let project = a.project;
            self.edit(
                label,
                a.magnet,
                None,
                move |p| {
                    *p = project;
                    true
                },
                cx,
            );
        }
        if let Some(t) = a.seek_to {
            self.seek(t, cx);
        }
        if let Some(sel) = a.select {
            self.selected
                .set_opt(sel.filter(|id| self.project.find_clip(id).is_some()));
            self.inspector_entry = None;
        }
        cx.notify();
    }

    fn agent_snapshot(&mut self) -> Value {
        let selected = self.selected.primary().map(String::from);
        snapshot(
            &self.project,
            &mut self.agent.aliases,
            &SnapshotContext {
                playhead: self.playhead,
                selected: selected.as_deref(),
                media: &self.media,
            },
        )
    }

    // ---- the turn -----------------------------------------------------------------

    pub fn agent_send(&mut self, text: String, cx: &mut Context<Self>) {
        let text = text.trim().to_string();
        if text.is_empty() || self.agent.busy {
            return;
        }
        let Some(server) = self.agent.settings.server() else {
            self.agent.setup_open = true;
            cx.notify();
            return;
        };
        self.agent_ui.input.update(cx, |i, cx| i.set_text("", cx));
        let cloud = server.cloud();
        self.agent.items.push(Item::User(text.clone()));
        // The request's words pick its skills: their know-how comes with
        // it, their tools with the turn.
        let all_skills = skills::load();
        let routed = skills::route(&all_skills, &text);
        for skill in &routed {
            self.agent.items.push(Item::Tool {
                name: skills::USE_SKILL.into(),
                detail: skill.name.clone(),
                status: Status::Ok,
                result: skill.tools.join(", "),
                progress: None,
            });
        }
        let loaded = Loaded::routed(&routed);
        self.agent.busy = true;
        self.agent.last_turn = None;
        let stop = Arc::new(AtomicBool::new(false));
        self.agent.stop = stop.clone();
        self.agent.turn += 1;
        let turn = self.agent.turn;
        let undo_before = self.history.undo_len();
        let mut state = self.agent_snapshot();
        // A video being written scene by scene: a later request can
        // change one scene of it.
        if let Some(board) = &self.agent.board {
            state["storyboard"] = board.summary();
        }
        let input = TurnInput {
            server,
            system: Message::System(agent::system_prompt(cloud, &all_skills)),
            history: self.agent.history.clone(),
            request: Message::User(skills::request(&state, &routed, &text, cloud)),
            skills: all_skills.clone(),
            loaded,
            max_rounds: if cloud {
                agent::MAX_CLOUD_ROUNDS
            } else {
                agent::MAX_LOCAL_ROUNDS
            },
            cloud,
            stop,
        };
        self.agent_ui.scroll.scroll_to_bottom();
        self.agent_ticker(turn, cx);
        cx.spawn(async move |this, cx| {
            let mut turn_msgs = vec![];
            let end = run_turn(&this, cx, turn, &input, &mut turn_msgs).await;
            this.update(cx, |ed, cx| {
                if ed.agent.turn != turn {
                    return; // New chat while it ran.
                }
                ed.agent.busy = false;
                match end {
                    TurnEnd::Replied | TurnEnd::Gone => {}
                    TurnEnd::Stopped => ed.agent.items.push(Item::Assistant("Stopped.".into())),
                    TurnEnd::Failed(e) => ed.agent.items.push(Item::Error(e)),
                }
                // Every running row is over now.
                for item in ed.agent.items.iter_mut() {
                    if let Item::Tool {
                        status, progress, ..
                    } = item
                        && *status == Status::Running
                    {
                        *status = Status::Error;
                        *progress = None;
                    }
                }
                let mut history = std::mem::take(&mut ed.agent.history);
                history.push(Message::User(text));
                history.extend(complete_pairs(turn_msgs));
                ed.agent.history = agent::trim_history(history);
                let after = ed.history.undo_len();
                ed.agent.last_turn = (after > undo_before).then_some((undo_before, after));
                ed.agent_ui.scroll.scroll_to_bottom();
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Repaints while a turn runs (progress lines, the dots).
    fn agent_ticker(&self, turn: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                let Ok(going) = this.update(cx, |ed, cx| {
                    let going = ed.agent.busy && ed.agent.turn == turn;
                    if going {
                        cx.notify();
                    }
                    going
                }) else {
                    return;
                };
                if !going {
                    return;
                }
            }
        })
        .detach();
    }

    pub fn agent_stop(&mut self, cx: &mut Context<Self>) {
        self.agent.stop.store(true, Ordering::Relaxed);
        cx.notify();
    }

    pub fn agent_new_chat(&mut self, cx: &mut Context<Self>) {
        self.agent.reset();
        cx.notify();
    }

    /// Takes back the last turn's edits (when nothing was done since).
    pub fn agent_undo_turn(&mut self, cx: &mut Context<Self>) {
        let Some((before, after)) = self.agent.last_turn.take() else {
            return;
        };
        if self.history.undo_len() != after {
            self.toast(
                format!(
                    "Edited since: undo by hand ({})",
                    crate::shortcuts::hint(crate::shortcuts::Shortcut::Undo)
                ),
                cx,
            );
            return;
        }
        for _ in before..after {
            self.undo(cx);
        }
        self.agent.items.push(Item::Assistant(format!(
            "Undid {} AI edit{}.",
            after - before,
            if after - before == 1 { "" } else { "s" }
        )));
        cx.notify();
    }

    // ---- setup --------------------------------------------------------------------

    /// Gets a local model loaded and its instructions + tools read before
    /// the first request (`llm::warm_up`): otherwise the first request waits
    /// for both, 15–20 s on a laptop. Once per model while the server keeps
    /// it loaded; a request sent meanwhile simply queues behind it.
    pub fn agent_warm_up(&mut self, cx: &mut Context<Self>) {
        const FRESH: std::time::Duration = std::time::Duration::from_secs(25 * 60);
        let Some(server) = self.agent.settings.server() else {
            return;
        };
        if server.cloud() || self.agent.busy {
            return;
        }
        if let Some((base, model, at)) = &self.agent.warmed
            && *base == server.base_url
            && *model == server.model
            && at.elapsed() < FRESH
        {
            return;
        }
        self.agent.warmed = Some((
            server.base_url.clone(),
            server.model.clone(),
            std::time::Instant::now(),
        ));
        // The same fixed start every request has: no skill loaded.
        let all_skills = skills::load();
        let system = agent::system_prompt(false, &all_skills);
        let tools = Loaded::default().tools(&all_skills, false);
        let task = cx
            .background_executor()
            .spawn(async move { llm::warm_up(&server, &system, &tools) });
        cx.spawn(async move |this, cx| {
            if task.await.is_err() {
                // Not there yet (server down, model missing): the next try warms.
                this.update(cx, |ed, _| ed.agent.warmed = None).ok();
            }
        })
        .detach();
    }

    pub fn agent_toggle(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let s = &mut self.agent.settings;
        s.open = !s.open;
        s.save();
        if s.open {
            if s.model.is_empty() {
                self.agent.setup_open = true;
            }
            let base = self.agent.settings.base_url.clone();
            self.agent_ui
                .address
                .update(cx, |i, cx| i.set_text(&base, cx));
            if self.agent.models.is_empty() && !self.agent.settings.model.is_empty() {
                self.agent_connect(cx);
            }
            self.agent_warm_up(cx);
            if !self.agent.setup_open {
                self.agent_ui.input.read(cx).focus(window);
            }
        }
        cx.notify();
    }

    pub fn agent_pick_preset(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.agent.settings.pick_preset(id);
        self.agent.settings.save();
        self.agent.models.clear();
        self.agent.setup_error = None;
        let base = self.agent.settings.base_url.clone();
        self.agent_ui
            .address
            .update(cx, |i, cx| i.set_text(&base, cx));
        let p = preset(id);
        if !p.cloud || keys::has(p.id) {
            self.agent_connect(cx);
        }
        cx.notify();
    }

    /// Lists the server's models (and keeps the address).
    pub fn agent_connect(&mut self, cx: &mut Context<Self>) {
        let p = preset(&self.agent.settings.preset);
        if !p.cloud {
            let typed = self.agent_ui.address.read(cx).text().trim().to_string();
            self.agent.settings.base_url = if typed.is_empty() {
                p.base_url.to_string()
            } else {
                typed
            };
        } else if !keys::has(p.id) {
            self.agent.setup_error = Some("Save your API key first.".into());
            cx.notify();
            return;
        }
        self.agent.settings.save();
        self.agent.connecting = true;
        self.agent.setup_error = None;
        let (dialect, base) = (p.dialect, self.agent.settings.base_url.clone());
        let auth = p.cloud.then(|| p.id.to_string());
        let task = cx
            .background_executor()
            .spawn(async move { llm::list_models(dialect, &base, auth.as_deref()) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |ed, cx| {
                ed.agent.connecting = false;
                match result {
                    Ok(ids) => {
                        let models = preset(&ed.agent.settings.preset).chat_models(ids);
                        if models.is_empty() {
                            ed.agent.setup_error = Some(
                                "This server has no models yet. With Ollama, run “ollama pull qwen3-vl:4b-instruct” in a terminal."
                                    .into(),
                            );
                        } else {
                            let s = &mut ed.agent.settings;
                            if !models.contains(&s.model) {
                                s.model = models[0].clone();
                            }
                            if !s.vision_model.is_empty() && !models.contains(&s.vision_model) {
                                s.vision_model.clear();
                            }
                            s.save();
                        }
                        ed.agent.models = models;
                        ed.agent_warm_up(cx);
                    }
                    Err(e) => {
                        ed.agent.models.clear();
                        ed.agent.setup_error = Some(format!("Couldn't reach the server: {e:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn agent_save_key(&mut self, cx: &mut Context<Self>) {
        let id = preset(&self.agent.settings.preset).id;
        let key = self.agent_ui.key.read(cx).text().trim().to_string();
        if key.is_empty() {
            return;
        }
        match keys::save(id, &key, cx) {
            Ok(()) => {
                self.agent_ui.key.update(cx, |i, cx| i.set_text("", cx));
                self.agent_connect(cx);
            }
            Err(e) => self.agent.setup_error = Some(format!("Couldn't save the key: {e:#}")),
        }
        cx.notify();
    }

    fn agent_remove_key(&mut self, cx: &mut Context<Self>) {
        let id = preset(&self.agent.settings.preset).id;
        let _ = keys::save(id, "", cx);
        self.agent.models.clear();
        cx.notify();
    }

    fn model_menu(&mut self, vision: bool, at: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let current = if vision {
            self.agent.settings.vision_model.clone()
        } else {
            self.agent.settings.model.clone()
        };
        let mut items = vec![];
        if vision {
            items.push(
                PopupItem::new("Same as the chat model", |ed: &mut Editor, _, cx| {
                    ed.agent.settings.vision_model.clear();
                    ed.agent.settings.save();
                    cx.notify();
                })
                .checked(current.is_empty()),
            );
            items.push(PopupItem::separator());
        }
        for m in self.agent.models.clone() {
            let on = m == current;
            items.push(
                PopupItem::new(m.clone(), move |ed: &mut Editor, _, cx| {
                    if vision {
                        ed.agent.settings.vision_model = m.clone();
                    } else {
                        ed.agent.settings.model = m.clone();
                    }
                    ed.agent.settings.save();
                    if !vision {
                        ed.agent_warm_up(cx);
                    }
                    cx.notify();
                })
                .checked(on),
            );
        }
        // Opened left of the pointer: the panel sits at the window's edge.
        self.open_popup(gpui::point(at.x - px(200.), at.y), items, cx);
    }

    // ---- the panel ----------------------------------------------------------------

    pub fn render_agent(&self, cx: &mut Context<Self>) -> AnyElement {
        let setup = self.agent.setup_open || self.agent.settings.model.is_empty();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.agent_header(setup, cx))
            .child(if setup {
                self.agent_setup(cx)
            } else {
                self.agent_chat(cx)
            })
            .into_any_element()
    }

    fn agent_header(&self, setup: bool, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_none()
            .h(px(40.))
            .pl(px(14.))
            .pr(px(6.))
            .flex()
            .items_center()
            .gap(px(6.))
            .border_b_1()
            .border_color(theme::card_border())
            .child(Icon::Sparkles.el(15., theme::text()))
            .child(
                div()
                    .text_size(px(theme::SM))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t("AI agent")),
            )
            .child(
                div()
                    .px(px(5.))
                    .h(px(16.))
                    .flex()
                    .items_center()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme::chip_border())
                    .font_family(theme::font_mono())
                    .text_size(px(theme::XXS))
                    .text_color(theme::text_tertiary())
                    .child(t("EXPERIMENTAL")),
            )
            .child(div().flex_1())
            .child(
                icon_button("agent-new", Icon::Add, "New chat")
                    .disabled(self.agent.items.is_empty() && !self.agent.busy)
                    .on_click(cx.listener(|ed, _, _, cx| ed.agent_new_chat(cx))),
            )
            .child(
                icon_button("agent-settings", Icon::Settings, "Model settings")
                    .active(setup)
                    .on_click(cx.listener(|ed, _, _, cx| {
                        ed.agent.setup_open = !ed.agent.setup_open;
                        cx.notify();
                    })),
            )
            .child(
                icon_button("agent-close", Icon::Close, "Close AI agent")
                    .on_click(cx.listener(|ed, _, w, cx| ed.agent_toggle(w, cx))),
            )
    }

    fn field_box(&self, field: &gpui::Entity<TextInput>) -> gpui::Div {
        div()
            .flex_1()
            .min_w_0()
            .h(px(30.))
            .px(px(8.))
            .rounded(px(6.))
            .bg(theme::lane_bg())
            .border_1()
            .border_color(theme::chip_border())
            .flex()
            .items_center()
            .text_size(px(theme::SM))
            .child(field.clone())
    }

    /// The request: one line tall when empty, growing to
    /// `REQUEST_MAX_LINES` with what's typed, then scrolling.
    fn request_box(&self) -> impl IntoElement {
        div()
            .flex_1()
            .min_w_0()
            .rounded(px(6.))
            .bg(theme::lane_bg())
            .border_1()
            .border_color(theme::chip_border())
            .text_size(px(theme::SM))
            .line_height(px(REQUEST_LINE))
            .child(
                div()
                    .id("agent-request")
                    .max_h(px(REQUEST_LINE * REQUEST_MAX_LINES + 12.))
                    .overflow_y_scroll()
                    .track_scroll(&self.agent_ui.input_scroll)
                    .px(px(8.))
                    .py(px(6.))
                    .child(self.agent_ui.input.clone()),
            )
    }

    fn label(text: &'static str) -> gpui::Div {
        div()
            .mt(px(12.))
            .mb(px(6.))
            .font_family(theme::font_mono())
            .text_size(px(theme::XXS))
            .text_color(theme::text_tertiary())
            .child(crate::i18n::t(text).to_uppercase())
    }

    fn agent_setup(&self, cx: &mut Context<Self>) -> AnyElement {
        let s = &self.agent.settings;
        let p = preset(&s.preset);
        let has_key = p.cloud && keys::has(p.id);
        let mut col = div()
            .id("agent-setup")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(14.))
            .py(px(10.))
            .flex()
            .flex_col()
            .child(panel_note(
                "Let an AI model edit your timeline: run one on this computer (nothing leaves your machine), or use Claude, ChatGPT, Gemini and others with your own API key. It can cut, title, animate, add effects and sound-design your edit.",
            ))
            .child(Self::label("Provider"))
            .child(
                chip("agent-provider")
                    .justify_between()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|ed, e: &gpui::ClickEvent, _, cx| {
                        let current = ed.agent.settings.preset.clone();
                        let mut items = vec![];
                        for (i, p) in PRESETS.iter().enumerate() {
                            if i > 0 && p.cloud && !PRESETS[i - 1].cloud {
                                items.push(PopupItem::separator());
                            }
                            let id = p.id;
                            items.push(
                                PopupItem::new(p.label, move |ed: &mut Editor, _, cx| {
                                    ed.agent_pick_preset(id, cx)
                                })
                                .checked(current == p.id),
                            );
                        }
                        let at = e.position();
                        ed.open_popup(gpui::point(at.x - px(160.), at.y), items, cx);
                    }))
                    .child(div().text_size(px(theme::SM)).child(p.label))
                    .child(Icon::ChevronDown.el(12., theme::text_secondary())),
            );

        if p.cloud {
            col = col.child(Self::label("API key"));
            if has_key {
                col = col
                    .child(panel_note(if keys::persistent() {
                        "Saved in your system keychain. Only this app can use it, and it's never shown again."
                    } else {
                        "Kept for this session only."
                    }))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                pill_button("agent-key-remove", "Remove key", false)
                                    .on_click(cx.listener(|ed, _, _, cx| ed.agent_remove_key(cx))),
                            )
                            .when(self.agent.models.is_empty(), |d| {
                                d.child(
                                    pill_button("agent-connect", "Load models", true)
                                        .on_click(cx.listener(|ed, _, _, cx| ed.agent_connect(cx))),
                                )
                            }),
                    );
            } else {
                let url = p.help_url;
                col = col
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(self.field_box(&self.agent_ui.key))
                            .child(
                                pill_button("agent-key-save", "Save", true)
                                    .on_click(cx.listener(|ed, _, _, cx| ed.agent_save_key(cx))),
                            ),
                    )
                    .child(
                        div()
                            .id("agent-get-key")
                            .mt(px(6.))
                            .text_size(px(theme::XS))
                            .text_color(theme::text_secondary())
                            .underline()
                            .cursor_pointer()
                            .on_click(move |_, _, cx| cx.open_url(url))
                            .child(format!("Get a key ({})", p.key_hint)),
                    );
            }
        } else {
            col = col.child(Self::label("Address")).child(
                div()
                    .flex()
                    .gap_2()
                    .child(self.field_box(&self.agent_ui.address))
                    .child(
                        pill_button("agent-connect", "Connect", true)
                            .on_click(cx.listener(|ed, _, _, cx| ed.agent_connect(cx))),
                    ),
            );
        }
        if self.agent.connecting {
            col = col.child(panel_note("Connecting…"));
        }
        if let Some(e) = &self.agent.setup_error {
            col = col.child(
                div()
                    .mt(px(8.))
                    .text_size(px(theme::XS))
                    .line_height(px(16.))
                    .text_color(theme::danger())
                    .child(e.clone()),
            );
        }
        if !self.agent.models.is_empty() {
            let vision = if s.vision_model.is_empty() {
                "Same as the chat model".to_string()
            } else {
                s.vision_model.clone()
            };
            col = col
                .child(Self::label("Model"))
                .child(self.menu_chip("agent-model", s.model.clone(), false, cx))
                .child(Self::label("Vision model"))
                .child(self.menu_chip("agent-vision", vision, true, cx))
                .child(panel_note(if p.cloud {
                    "Cloud models can see images themselves. Keep the chat model, or pick a cheaper one just for looking."
                } else {
                    "Lets the AI look at your video. Best: a chat model that sees images itself (Qwen3-VL) with this left on “Same as the chat model”. A separate vision model works, but most computers can hold only one model at a time, so every look reloads both."
                }));
        }
        col = col.child(div().mt(px(10.)).child(panel_note(if p.cloud {
            format!(
                "Your timeline and the video frames the AI looks at are sent to {}. Usage is billed to your key. Sound effects come from the app's library and Openverse (free CC0 / CC BY recordings).",
                p.label
            )
        } else {
            "Pick a model that supports tool calling. Qwen3-VL 4B Instruct (“ollama pull qwen3-vl:4b-instruct”, 3.3 GB) edits and sees your video in one model; on 32 GB or more, qwen3-vl:8b-instruct plans longer edits. Sound effects come from the app's library and Openverse (free CC0 / CC BY recordings).".to_string()
        })));
        // Skills: the built-in ones, and the user's own files.
        let all_skills = skills::load();
        let yours = all_skills.iter().filter(|k| k.user).count();
        col = col
            .child(div().mt(px(6.)).child(Self::label("Skills")))
            .child(panel_note(format!(
                "{} built in{}. A skill is a job's steps and tools, loaded when a request needs it. Add your own as Markdown files in the skills folder: a name, a description, trigger words and tools on top, then the steps.",
                all_skills.len() - yours,
                match yours {
                    0 => String::new(),
                    1 => " · 1 of yours".into(),
                    n => format!(" · {n} of yours"),
                }
            )))
            .child(
                div().flex().child(
                    pill_button("agent-skills", "Open skills folder", false).on_click(
                        |_, _, cx| {
                            let dir = skills::folder();
                            let _ = std::fs::create_dir_all(&dir);
                            cx.open_with_system(&dir);
                        },
                    ),
                ),
            );
        let ready = !s.model.is_empty();
        col = col.child(
            div().mt(px(14.)).flex().child(
                pill_button("agent-start", "Start chatting", true)
                    .when(!ready, |d| d.opacity(0.4).cursor_default())
                    .on_click(cx.listener(move |ed, _, w, cx| {
                        if !ed.agent.settings.model.is_empty() {
                            ed.agent.setup_open = false;
                            ed.agent_ui.input.read(cx).focus(w);
                            cx.notify();
                        }
                    })),
            ),
        );
        col.into_any_element()
    }

    fn menu_chip(
        &self,
        id: &'static str,
        label: String,
        vision: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        chip(id)
            .justify_between()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |ed, e: &gpui::ClickEvent, _, cx| {
                ed.model_menu(vision, e.position(), cx)
            }))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(theme::SM))
                    .child(label),
            )
            .child(Icon::ChevronDown.el(12., theme::text_secondary()))
    }

    fn agent_chat(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.agent.busy;
        let mut list = div()
            .id("agent-chat")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.agent_ui.scroll)
            .px(px(12.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(8.));
        if self.agent.items.is_empty() {
            list = list.child(self.agent_empty(cx));
        }
        for (i, item) in self.agent.items.iter().enumerate() {
            list = list.child(self.agent_item(i, item));
        }
        if busy {
            let dots = ".".repeat(
                1 + (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() / 300)
                    .unwrap_or(0)
                    % 3) as usize,
            );
            list = list.child(
                div()
                    .text_size(px(theme::XS))
                    .text_color(theme::text_tertiary())
                    .child(format!("Thinking{dots}")),
            );
        }
        let mut footer = div()
            .flex_none()
            .p(px(10.))
            .border_t_1()
            .border_color(theme::card_border())
            .flex()
            .flex_col()
            .gap(px(8.));
        if let Some((before, after)) = self.agent.last_turn
            && !busy
        {
            let n = after - before;
            footer = footer.child(
                div().flex().child(
                    pill_button("agent-undo", format!("Undo AI edits ({n})"), false)
                        .on_click(cx.listener(|ed, _, _, cx| ed.agent_undo_turn(cx))),
                ),
            );
        }
        footer = footer.child(
            div()
                .flex()
                .items_end()
                .gap_2()
                .child(self.request_box())
                .child(if busy {
                    pill_button("agent-stop", "Stop", false)
                        .on_click(cx.listener(|ed, _, _, cx| ed.agent_stop(cx)))
                } else {
                    pill_button("agent-send", "Send", true).on_click(cx.listener(|ed, _, _, cx| {
                        let text = ed.agent_ui.input.read(cx).text().to_string();
                        ed.agent_send(text, cx);
                    }))
                }),
        );
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(list)
            .child(footer)
            .into_any_element()
    }

    fn agent_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let cloud = self.agent.settings.cloud();
        let mut col = div()
            .pt(px(24.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(theme::MD))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("What should I make?"),
            )
            .child(panel_note(if cloud {
                "I can make a whole video from one prompt (narration, animated scenes and charts, captions, music and sound effects) or work across your timeline: cut, trim, move and copy clips, add your media, voiceover, animated titles, motion, effects, filters and colour, and sound-design the edit. I can also look at your video to find moments."
            } else {
                "I can make a short video from one prompt (voiceover, animated scenes, captions, music and sound effects), or cut, trim and move clips, add animated titles and text, animate clips, add effects, filters and colour, and sound-design your edit. I can also look at your video to find moments."
            }))
            .child(
                div()
                    .text_size(px(theme::XS))
                    .text_color(theme::text_tertiary())
                    .child(format!("Model: {}", self.agent.settings.model)),
            );
        for (i, s) in SUGGESTIONS.iter().enumerate() {
            col = col.child(
                div()
                    .id(("agent-suggest", i))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(8.))
                    .bg(theme::chip_bg())
                    .border_1()
                    .border_color(theme::chip_border())
                    .text_size(px(theme::XS))
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::button_hover()))
                    .on_click(cx.listener(move |ed, _, _, cx| ed.agent_send(s.to_string(), cx)))
                    .child(*s),
            );
        }
        col
    }

    fn agent_item(&self, i: usize, item: &Item) -> AnyElement {
        match item {
            Item::User(text) => div()
                .flex()
                .justify_end()
                .child(
                    div()
                        .max_w(px(260.))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(10.))
                        .bg(theme::button_bg())
                        .text_size(px(theme::SM))
                        .line_height(px(18.))
                        .child(text.clone()),
                )
                .into_any_element(),
            Item::Assistant(text) => div()
                .text_size(px(theme::SM))
                .line_height(px(18.))
                .child(text.clone())
                .into_any_element(),
            Item::Error(text) => div()
                .flex()
                .gap(px(6.))
                .child(Icon::Warning.el(13., theme::danger()))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(theme::XS))
                        .line_height(px(16.))
                        .text_color(theme::danger())
                        .child(text.clone()),
                )
                .into_any_element(),
            Item::Tool {
                name,
                detail,
                status,
                result,
                progress,
            } => {
                let (icon, color) = match status {
                    Status::Running => (Icon::Ellipsis, theme::text_tertiary()),
                    Status::Ok => (Icon::Checkmark, theme::success()),
                    Status::Error => (Icon::Warning, theme::danger()),
                };
                let line = match (status, progress) {
                    (Status::Running, Some(p)) => p.get(),
                    _ => result.clone(),
                };
                div()
                    .id(("agent-tool", i))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme::card_border())
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(icon.el(12., color))
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(theme::XS))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(tools::label(name)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(theme::font_mono())
                                    .text_size(px(theme::XXS))
                                    .text_color(theme::text_tertiary())
                                    .child(detail.clone()),
                            ),
                    )
                    .when(!line.is_empty(), |d| {
                        d.child(
                            div()
                                .pl(px(18.))
                                .text_size(px(theme::XS))
                                .line_height(px(15.))
                                .text_color(if *status == Status::Error {
                                    theme::danger()
                                } else {
                                    theme::text_secondary()
                                })
                                .child(line),
                        )
                    })
                    .into_any_element()
            }
        }
    }
}

/// Drops a trailing assistant call whose results never came (a stopped
/// turn): providers refuse a call without its result.
fn complete_pairs(mut msgs: Vec<Message>) -> Vec<Message> {
    let mut i = 0;
    while i < msgs.len() {
        if let Message::Assistant { calls, .. } = &msgs[i]
            && !calls.is_empty()
        {
            let answered = calls.iter().all(|c| {
                msgs[i + 1..]
                    .iter()
                    .any(|m| matches!(m, Message::Tool { call_id, .. } if call_id == &c.id))
            });
            if !answered {
                msgs.truncate(i);
                break;
            }
        }
        i += 1;
    }
    msgs
}

async fn run_turn(
    this: &WeakEntity<Editor>,
    cx: &mut AsyncApp,
    turn: u64,
    t: &TurnInput,
    turn_msgs: &mut Vec<Message>,
) -> TurnEnd {
    let stopped = |cx: &mut AsyncApp| -> Option<TurnEnd> {
        if t.stop.load(Ordering::Relaxed) {
            return Some(TurnEnd::Stopped);
        }
        match this.update(cx, |ed, _| ed.agent.turn == turn) {
            Ok(true) => None,
            Ok(false) => Some(TurnEnd::Stopped),
            Err(_) => Some(TurnEnd::Gone),
        }
    };
    let mut seen: Vec<(String, String)> = vec![];
    // An editing tool succeeded this turn; the claim check ran once.
    let mut edited = false;
    let mut nudged = false;
    // A reply that stopped with a storyboard written but not built, or
    // with nothing in it at all: each sent back once.
    let mut storyboarding = false;
    let mut board_nudged = false;
    let mut empty_nudged = false;
    // Skills the model loads join the routed ones for the next rounds.
    let mut loaded = t.loaded.clone();
    for _ in 0..t.max_rounds {
        if let Some(end) = stopped(cx) {
            return end;
        }
        let mut msgs = vec![t.system.clone()];
        msgs.extend(t.history.iter().cloned());
        msgs.push(t.request.clone());
        msgs.extend(turn_msgs.iter().cloned());
        let (server, tools) = (t.server.clone(), loaded.tools(&t.skills, t.cloud));
        let reply = cx
            .background_executor()
            .spawn(async move { llm::chat(&server, &msgs, &tools) })
            .await;
        if let Some(end) = stopped(cx) {
            return end;
        }
        let reply = match reply {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("{e:#}");
                return TurnEnd::Failed(if msg.contains("timed out") || msg.contains("Timeout") {
                    "The model took too long to answer.".into()
                } else {
                    format!("The model couldn't answer: {msg}")
                });
            }
        };
        turn_msgs.push(Message::Assistant {
            content: reply.content.clone(),
            calls: reply.calls.clone(),
        });
        if reply.truncated && reply.calls.is_empty() {
            return TurnEnd::Failed(
                "The model ran out of room before doing anything. Reasoning (“thinking”) models do this: pick an instruct model, like qwen3-vl:4b-instruct.".into(),
            );
        }
        let text = reply.content.trim().to_string();
        // "I've added a title" with no edit made: send it back to do it.
        if reply.calls.is_empty() && !edited && !nudged && agent::claims_edit(&text) {
            nudged = true;
            turn_msgs.push(Message::User(agent::NUDGE.into()));
            continue;
        }
        if reply.calls.is_empty() && storyboarding && !board_nudged {
            let pending = this
                .update(cx, |ed, _| {
                    ed.agent.board.as_ref().and_then(|b| b.pending())
                })
                .ok()
                .flatten();
            if let Some(pending) = pending {
                board_nudged = true;
                turn_msgs.push(Message::User(pending));
                continue;
            }
        }
        if reply.calls.is_empty() && text.is_empty() && !empty_nudged {
            empty_nudged = true;
            turn_msgs.push(Message::User(agent::EMPTY_NUDGE.into()));
            continue;
        }
        if !text.is_empty() {
            let _ = this.update(cx, |ed, cx| {
                ed.agent.items.push(Item::Assistant(text));
                ed.agent_ui.scroll.scroll_to_bottom();
                cx.notify();
            });
        }
        if reply.calls.is_empty() {
            return TurnEnd::Replied;
        }
        // In order: a later call may use an id an earlier one returned.
        for call in reply.calls {
            if let Some(end) = stopped(cx) {
                return end;
            }
            seen.push((
                call.name.clone(),
                Value::Object(call.arguments.clone()).to_string(),
            ));
            storyboarding |= matches!(call.name.as_str(), "start_video" | "write_scene");
            if agent::looping(&seen) {
                return TurnEnd::Failed(
                    "The model kept repeating the same step, so it was stopped. Try a simpler request.".into(),
                );
            }
            // A skill called by its own name shows as the skill it loads.
            let (row, detail) =
                if call.name != skills::USE_SKILL && Loaded::handles(&t.skills, &call.name) {
                    (skills::USE_SKILL.to_string(), call.name.clone())
                } else {
                    (call.name.clone(), agent::describe_args(&call.arguments))
                };
            let Ok(index) = this.update(cx, |ed, cx| {
                ed.agent.items.push(Item::Tool {
                    name: row,
                    detail,
                    status: Status::Running,
                    result: String::new(),
                    progress: None,
                });
                ed.agent_ui.scroll.scroll_to_bottom();
                cx.notify();
                ed.agent.items.len() - 1
            }) else {
                return TurnEnd::Gone;
            };
            let outcome = if Loaded::handles(&t.skills, &call.name) {
                // Not an edit: the harness's own tool.
                loaded
                    .load_call(&t.skills, &call.name, &call.arguments, t.cloud)
                    .map(Outcome::Done)
            } else {
                match this.update(cx, |ed, cx| {
                    ed.agent_run_tool(&call.name, call.arguments.clone(), cx)
                }) {
                    Ok(o) => o,
                    Err(_) => return TurnEnd::Gone,
                }
            };
            let result: ToolResult = match outcome {
                Err(e) => Err(e),
                Ok(Outcome::Done(v)) => Ok(v),
                Ok(Outcome::Later(job)) => {
                    let progress = Progress::new(t.stop.clone());
                    let shown = progress.clone();
                    let _ = this.update(cx, |ed, _| {
                        if let Some(Item::Tool { progress, .. }) = ed.agent.items.get_mut(index) {
                            *progress = Some(shown);
                        }
                    });
                    let p = progress.clone();
                    let done = cx.background_executor().spawn(async move { job(&p) }).await;
                    match done {
                        Err(e) => Err(e),
                        Ok(_) if progress.stopped() => Err("Stopped".into()),
                        Ok(finish) => {
                            match this.update(cx, |ed, cx| ed.agent_finish_tool(finish, cx)) {
                                Ok(r) => r,
                                Err(_) => return TurnEnd::Gone,
                            }
                        }
                    }
                }
            };
            if result.is_ok()
                && !agent::READ_ONLY.contains(&call.name.as_str())
                && !Loaded::handles(&t.skills, &call.name)
            {
                edited = true;
            }
            let (status, gist) = match &result {
                Ok(v) => (Status::Ok, agent::describe_result(v)),
                Err(e) => (Status::Error, e.0.clone()),
            };
            let _ = this.update(cx, |ed, cx| {
                if let Some(Item::Tool {
                    status: s,
                    result: r,
                    progress,
                    ..
                }) = ed.agent.items.get_mut(index)
                {
                    *s = status;
                    *r = gist;
                    *progress = None;
                }
                cx.notify();
            });
            turn_msgs.push(Message::Tool {
                call_id: call.id,
                name: call.name,
                content: agent::tool_content(&result, t.cloud),
                error: result.is_err(),
            });
        }
    }
    TurnEnd::Failed(
        "The model kept going without finishing, so it was stopped. Try a simpler request.".into(),
    )
}
