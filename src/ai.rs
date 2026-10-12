//! The assistant panel: a chat that drives the open document through the app's own command
//! surface (the same verbs the CLI and MCP servers use).
//!
//! The model is any OpenAI-compatible chat endpoint. Settings come from
//! `~/.config/officecraft/ai.json` (`base_url`, `model`); a key, if the endpoint wants one, is
//! read from Settings (kept in an owner-only `ai-keys.json`) or `OFFICECRAFT_AI_KEY`.

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use web_time::Instant;

use crate::platform;

use serde_json::{Value, json};

const MAX_STEPS: u8 = 16;
const MAX_RESULT_CHARS: usize = 12_000;
const DEFAULT_BASE: &str = "http://127.0.0.1:11434/v1";
// Small and quick while the suite is being tested. Override with `model` in ai.json or
// OFFICECRAFT_AI_MODEL (qwen3-coder:30b is far better at multi-step edits).
const DEFAULT_MODEL: &str = "qwen3-coder:30b";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Provider {
    Local,
    Cloud,
    OpenRouter,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Provider::Local, Provider::Cloud, Provider::OpenRouter];
    fn index(self) -> usize {
        self as usize
    }
    fn id(self) -> &'static str {
        match self {
            Provider::Local => "local",
            Provider::Cloud => "cloud",
            Provider::OpenRouter => "openrouter",
        }
    }
    fn from_id(s: &str) -> Option<Provider> {
        Provider::ALL.into_iter().find(|p| p.id() == s)
    }
    pub fn title(self) -> &'static str {
        match self {
            Provider::Local => "Local endpoint",
            Provider::Cloud => "Ollama Cloud",
            Provider::OpenRouter => "OpenRouter",
        }
    }
    fn default_base(self) -> &'static str {
        match self {
            Provider::Local => DEFAULT_BASE,
            Provider::Cloud => "https://ollama.com/v1",
            Provider::OpenRouter => "https://openrouter.ai/api/v1",
        }
    }
    fn default_model(self) -> &'static str {
        match self {
            Provider::Local => DEFAULT_MODEL,
            Provider::Cloud => "gpt-oss:120b",
            Provider::OpenRouter => "openai/gpt-oss-120b",
        }
    }
}

/// Where the assistant talks to. `base_url`, `model` and `key` are the ones in use right now (the
/// active provider's, or an `OFFICECRAFT_AI_*` environment override); the rest is what Settings edits.
#[derive(Clone)]
pub struct Config {
    pub base_url: String,
    pub model: String,
    key: Option<String>,
    pub provider: Provider,
    /// The endpoint of the local server (Ollama, LM Studio, llama.cpp, vLLM: anything OpenAI-compatible).
    pub local_url: String,
    /// The chosen model for each provider, in `Provider::ALL` order; empty = that provider's default.
    models: [String; 3],
    /// API keys for Ollama Cloud and OpenRouter (`keys[provider.index()]`; the local slot is unused).
    keys: [String; 3],
}

fn read_json(name: &str) -> Value {
    platform::read(name).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}

impl Config {
    pub fn load() -> Config {
        let json = read_json("ai.json");
        let keys_json = read_json("ai-keys.json");
        let get = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let mut c = Config {
            base_url: String::new(),
            model: String::new(),
            key: None,
            provider: Provider::from_id(&get(&json, "provider")).unwrap_or(Provider::Local),
            local_url: get(&json, "base_url"),
            models: [get(&json, "model"), get(&json, "cloud_model"), get(&json, "openrouter_model")],
            keys: [String::new(), get(&keys_json, "cloud"), get(&keys_json, "openrouter")],
        };
        c.apply();
        c
    }

    /// Recomputes the values in use from the settings and the environment overrides.
    pub fn apply(&mut self) {
        let p = self.provider;
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let base = match p {
            Provider::Local if !self.local_url.is_empty() => self.local_url.clone(),
            _ => p.default_base().to_string(),
        };
        self.base_url = env("OFFICECRAFT_AI_URL").unwrap_or(base).trim_end_matches('/').to_string();
        let model = &self.models[p.index()];
        self.model = env("OFFICECRAFT_AI_MODEL").unwrap_or_else(|| if model.is_empty() { p.default_model().to_string() } else { model.clone() });
        let key = &self.keys[p.index()];
        self.key = env("OFFICECRAFT_AI_KEY").or_else(|| (!key.is_empty()).then(|| key.clone()));
    }

    pub fn set_model(&mut self, model: &str) {
        self.models[self.provider.index()] = model.to_string();
        self.apply();
    }

    /// True when an `OFFICECRAFT_AI_*` variable overrides what Settings says.
    pub fn env_override() -> bool {
        ["OFFICECRAFT_AI_URL", "OFFICECRAFT_AI_MODEL", "OFFICECRAFT_AI_KEY"].iter().any(|k| std::env::var(k).is_ok_and(|v| !v.is_empty()))
    }

    /// Writes `ai.json` (provider, endpoint, models) and, separately, `ai-keys.json` (owner-only).
    pub fn save(&self) -> Result<(), String> {
        let settings = json!({
            "provider": self.provider.id(),
            "base_url": self.local_url,
            "model": self.models[0],
            "cloud_model": self.models[1],
            "openrouter_model": self.models[2],
        });
        platform::write("ai.json", &serde_json::to_string_pretty(&settings).unwrap_or_default())?;
        let keys = json!({"cloud": self.keys[1], "openrouter": self.keys[2]});
        platform::write_private("ai-keys.json", &serde_json::to_string_pretty(&keys).unwrap_or_default())
    }
}

/// The Settings window's working copy; `ui` returns the config when the user saves.
pub struct Settings {
    draft: Config,
    reveal: bool,
    saved_note: String,
    test: std::sync::Arc<std::sync::Mutex<Option<Result<usize, String>>>>,
}

impl Settings {
    pub fn new(cfg: &Config) -> Settings {
        Settings { draft: cfg.clone(), reveal: false, saved_note: String::new(), test: Default::default() }
    }

    /// `Some(Some(cfg))`: saved; `Some(None)`: closed without saving; `None`: still open.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Option<Config>> {
        let mut result = None;
        let mut open = true;
        egui::Window::new("Assistant settings").open(&mut open).collapsible(false).resizable(false).default_width(440.0).show(ctx, |ui| {
            let d = &mut self.draft;
            ui.label(egui::RichText::new("Provider").strong());
            ui.horizontal(|ui| {
                for p in Provider::ALL {
                    if ui.selectable_label(d.provider == p, p.title()).clicked() {
                        d.provider = p;
                        *self.test.lock().unwrap() = None;
                    }
                }
            });
            ui.add_space(6.0);
            let p = d.provider;
            egui::Grid::new("ai_settings_grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                match p {
                    Provider::Local => {
                        ui.label("Endpoint");
                        ui.add(egui::TextEdit::singleline(&mut d.local_url).hint_text(DEFAULT_BASE).desired_width(300.0));
                        ui.end_row();
                    }
                    _ => {
                        ui.label("Endpoint");
                        ui.label(egui::RichText::new(p.default_base()).monospace());
                        ui.end_row();
                        ui.label("API key");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut d.keys[p.index()]).password(!self.reveal).hint_text("paste your key").desired_width(260.0));
                            ui.checkbox(&mut self.reveal, "Show");
                        });
                        ui.end_row();
                    }
                }
                ui.label("Model");
                ui.add(egui::TextEdit::singleline(&mut d.models[p.index()]).hint_text(p.default_model()).desired_width(300.0));
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(match p {
                    Provider::Local => "Any OpenAI-compatible server: Ollama (http://127.0.0.1:11434/v1), LM Studio (http://127.0.0.1:1234/v1), llama.cpp, vLLM.",
                    Provider::Cloud => "Large hosted models. Your prompts and documents are sent to ollama.com.",
                    Provider::OpenRouter => "Many hosted models. Your prompts and documents are sent to openrouter.ai and on to the lab running the model.",
                })
                .size(12.0)
                .weak(),
            );
            if p != Provider::Local {
                ui.label(
                    egui::RichText::new(if cfg!(target_arch = "wasm32") {
                        "The key is kept in this browser's storage for this site. Anyone using this browser profile can read it."
                    } else {
                        "The key is kept in ~/.config/officecraft/ai-keys.json, readable only by you."
                    })
                    .size(12.0)
                    .weak(),
                );
            }
            if Config::env_override() {
                ui.colored_label(egui::Color32::from_rgb(180, 110, 0), "An OFFICECRAFT_AI_* environment variable overrides these settings.");
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Test connection").clicked() {
                    let mut probe = self.draft.clone();
                    probe.apply();
                    *self.test.lock().unwrap() = None;
                    let slot = self.test.clone();
                    let ctx = ui.ctx().clone();
                    spawn_task(async move {
                        let r = fetch_models(&probe, 8).await;
                        *slot.lock().unwrap() = Some(r.map(|v| v.len()));
                        ctx.request_repaint();
                    });
                }
                match &*self.test.lock().unwrap() {
                    Some(Ok(n)) => {
                        ui.colored_label(egui::Color32::from_rgb(46, 125, 79), format!("Connected: {n} model{}", if *n == 1 { "" } else { "s" }));
                    }
                    Some(Err(e)) => {
                        ui.colored_label(egui::Color32::from_rgb(194, 74, 47), e);
                    }
                    None => {}
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(egui::RichText::new("Save").strong()).clicked() {
                    let mut cfg = self.draft.clone();
                    cfg.apply();
                    match cfg.save() {
                        Ok(()) => result = Some(Some(cfg)),
                        Err(e) => self.saved_note = format!("Could not save: {e}"),
                    }
                }
                if ui.button("Cancel").clicked() {
                    result = Some(None);
                }
                if !self.saved_note.is_empty() {
                    ui.colored_label(egui::Color32::from_rgb(194, 74, 47), &self.saved_note);
                }
            });
        });
        if !open && result.is_none() {
            result = Some(None);
        }
        result
    }
}

/// What a tool call produced: now, or on a later frame (PDF answers once its window is drawn).
pub enum CallResult {
    Done(Result<Value, String>),
    Pending(Receiver<Result<Value, String>>),
}

/// The open document, as the assistant sees it.
pub trait Host {
    fn call(&mut self, method: &str, params: Value) -> CallResult;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum App {
    Docs,
    Sheets,
    Slides,
    Pdf,
}

impl App {
    fn noun(self) -> &'static str {
        match self {
            App::Docs => "document",
            App::Sheets => "workbook",
            App::Slides => "presentation",
            App::Pdf => "PDF",
        }
    }
}

/// One thing in the conversation, as drawn.
enum Item {
    User(String),
    Assistant(String),
    /// A run of tool calls the assistant made, shown as one activity card.
    Steps(Vec<Step>),
    Error(String),
    Note(String),
}

#[derive(Clone)]
struct Step {
    id: String,
    label: String,
    state: StepState,
    /// The call changed the file (not just looked at it).
    edit: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StepState {
    Running,
    Done,
    Failed,
}

struct PendingCall {
    id: String,
    name: String,
    rx: Receiver<Result<Value, String>>,
}

/// What the panel asks the window to do.
pub enum Action {
    /// The user submitted a message (call `send`).
    Send,
    /// A model was picked in the header: remember it.
    SetModel(String),
}

/// A streaming reply's events.
enum Ev {
    Text(String),
    Done(Result<Value, String>),
}

pub struct Chat {
    pub open: bool,
    history: Vec<Value>,
    items: Vec<Item>,
    input: String,
    reply: Option<Receiver<Ev>>,
    /// The words of the reply so far.
    streaming: String,
    pending: Vec<PendingCall>,
    /// Tool results gathered for the model's last turn, in call order.
    results: Vec<Value>,
    steps: u8,
    /// Which request's events count (Stop bumps it so a late reply is dropped).
    epoch: u64,
    /// When the current request began.
    started: Option<Instant>,
    /// How long the last answer took.
    took: Option<f32>,
    /// The composer should take the keyboard.
    focus_input: bool,
}

impl Chat {
    pub fn new() -> Chat {
        Chat {
            open: false,
            history: Vec::new(),
            items: Vec::new(),
            input: String::new(),
            reply: None,
            streaming: String::new(),
            pending: Vec::new(),
            results: Vec::new(),
            steps: 0,
            epoch: 0,
            started: None,
            took: None,
            focus_input: true,
        }
    }

    /// Sends `text` as if it had been typed and submitted.
    pub fn ask_text(&mut self, text: String, app: App, cfg: &Config, ctx: &egui::Context) {
        self.input = text;
        self.send(app, cfg, ctx);
    }

    pub fn has_error(&self) -> bool {
        self.items.iter().any(|l| matches!(l, Item::Error(_)))
    }

    /// The conversation as plain text.
    pub fn transcript(&self) -> String {
        let mut out = String::new();
        for l in &self.items {
            match l {
                Item::User(t) => out.push_str(&format!("USER: {t}\n")),
                Item::Assistant(t) => out.push_str(&format!("ASSISTANT: {t}\n")),
                Item::Steps(s) => {
                    for st in s {
                        out.push_str(&format!("  tool: {}\n", st.label));
                    }
                }
                Item::Error(t) => out.push_str(&format!("ERROR: {t}\n")),
                Item::Note(t) => out.push_str(&format!("NOTE: {t}\n")),
            }
        }
        out
    }

    pub fn busy(&self) -> bool {
        self.reply.is_some() || !self.pending.is_empty()
    }

    pub fn clear(&mut self) {
        let open = self.open;
        *self = Chat { open, ..Chat::new() };
    }

    /// Stops the current request: whatever arrives for it later is dropped.
    fn stop(&mut self) {
        self.epoch += 1;
        self.reply = None;
        self.pending.clear();
        self.results.clear();
        let partial = std::mem::take(&mut self.streaming);
        if !partial.trim().is_empty() {
            self.items.push(Item::Assistant(partial));
        }
        // Keep the history valid for the next message: no call may be left without an answer.
        if let Some(last) = self.history.last()
            && last.get("tool_calls").is_some()
        {
            let ids: Vec<String> = last
                .get("tool_calls")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|c| c.get("id").and_then(Value::as_str).map(str::to_string)).collect())
                .unwrap_or_default();
            for id in ids {
                self.history.push(json!({"role": "tool", "tool_call_id": id, "content": "stopped by the user"}));
            }
        }
        for it in &mut self.items {
            if let Item::Steps(s) = it {
                for st in s.iter_mut().filter(|st| st.state == StepState::Running) {
                    st.state = StepState::Failed;
                }
            }
        }
        self.items.push(Item::Note("Stopped.".into()));
        self.started = None;
    }

    /// Draws the panel in `accent` (the app's colour). `file` names what the assistant is working
    /// on; `models` is what the server offers.
    pub fn ui(&mut self, ui: &mut egui::Ui, app: App, cfg: &Config, accent: egui::Color32, file: &str, models: &[String]) -> Option<Action> {
        let mut action = None;
        let v = ui.visuals().clone();
        let ink = v.text_color();
        let muted = v.weak_text_color();
        let hairline = v.widgets.noninteractive.bg_stroke.color;
        let busy = self.busy();

        // Header: who, on what, with which model.
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (r, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
            paint_spark(ui.painter(), r, accent);
            ui.label(egui::RichText::new("Assistant").strong().size(15.0).color(ink));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can_new = !self.items.is_empty() && !busy;
                let r = ui.add_enabled(can_new, egui::Button::new(egui::RichText::new("New chat").size(12.0)).small()).on_hover_text("Start a new conversation");
                if r.clicked() {
                    self.clear();
                }
                // The model, as a quiet picker.
                let label = egui::RichText::new(short_model(&cfg.model)).size(12.0).color(muted);
                egui::ComboBox::from_id_salt("assistant_model").selected_text(label).width(150.0).show_ui(ui, |ui| {
                    if models.is_empty() {
                        ui.label(egui::RichText::new("No models found").color(muted));
                    }
                    for m in models {
                        if ui.selectable_label(*m == cfg.model, m).clicked() {
                            action = Some(Action::SetModel(m.clone()));
                        }
                    }
                });
            });
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let (r, _) = ui.allocate_exact_size(egui::vec2(6.0, 6.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 3.0, accent);
            ui.label(egui::RichText::new(format!("Working on {file}")).size(11.5).color(muted));
        });
        ui.add_space(6.0);
        ui.painter().hline(ui.max_rect().x_range(), ui.cursor().min.y, egui::Stroke::new(1.0, hairline));
        ui.add_space(2.0);

        // The composer is sized first so the conversation takes the rest.
        let composer_h = 92.0;
        let convo_h = (ui.available_height() - composer_h - 8.0).max(80.0);
        let empty = self.items.is_empty() && !busy;
        egui::ScrollArea::vertical().max_height(convo_h).min_scrolled_height(convo_h).auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add_space(8.0);
            if empty {
                if let Some(text) = empty_state(ui, app, accent, ink, muted) {
                    self.input = text.0;
                    if text.1 {
                        action = Some(Action::Send);
                    } else {
                        self.focus_input = true;
                    }
                }
            }
            let n = self.items.len();
            for (i, it) in self.items.iter().enumerate() {
                match it {
                    Item::User(t) => user_bubble(ui, t, accent, ink),
                    Item::Assistant(t) => {
                        assistant_message(ui, t, accent, ink, muted, i + 1 == n && !busy, self.took);
                    }
                    Item::Steps(steps) => steps_card(ui, steps, &v, muted, ink),
                    Item::Error(t) => {
                        ui.add_space(6.0);
                        egui::Frame::new()
                            .fill(egui::Color32::from_rgb(253, 236, 234).gamma_multiply(if v.dark_mode { 0.15 } else { 1.0 }))
                            .corner_radius(8)
                            .inner_margin(egui::Margin::symmetric(10, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(egui::RichText::new(t).size(12.5).color(egui::Color32::from_rgb(180, 50, 45)));
                            });
                    }
                    Item::Note(t) => {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(t).size(11.5).italics().color(muted));
                    }
                }
            }
            if busy {
                ui.add_space(6.0);
                if !self.streaming.is_empty() {
                    let live = self.streaming.clone();
                    assistant_message(ui, &live, accent, ink, muted, false, None);
                } else {
                    thinking_row(ui, accent, muted, self.started.map(|t| t.elapsed().as_secs()).unwrap_or(0));
                }
            }
            ui.add_space(8.0);
        });

        // The composer: one rounded box, the text above its send / stop button.
        ui.add_space(6.0);
        let mut send_clicked = false;
        let mut stop_clicked = false;
        let frame_stroke = if self.focus_input { egui::Stroke::new(1.5, accent) } else { egui::Stroke::new(1.0, hairline) };
        let resp = egui::Frame::new()
            .fill(v.extreme_bg_color)
            .stroke(frame_stroke)
            .corner_radius(14)
            .inner_margin(egui::Margin { left: 12, right: 8, top: 8, bottom: 6 })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let edit = egui::TextEdit::multiline(&mut self.input)
                    .id_salt("assistant_input")
                    .hint_text(format!("Ask me to change this {}…", app.noun()))
                    .frame(egui::Frame::NONE)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .lock_focus(true);
                let r = ui.add(edit);
                if self.focus_input {
                    r.request_focus();
                    self.focus_input = false;
                }
                let has_text = !self.input.trim().is_empty();
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Enter to send · Shift+Enter for a new line").size(10.5).color(muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if busy {
                            if round_button(ui, accent, true, true).on_hover_text("Stop").clicked() {
                                stop_clicked = true;
                            }
                        } else if round_button(ui, accent, false, has_text).on_hover_text("Send").clicked() && has_text {
                            send_clicked = true;
                        }
                    });
                });
                r
            });
        let edit = resp.inner;
        let enter = edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift && !i.modifiers.command);
        if enter {
            // The Enter that sent the message also typed a newline; drop it.
            while self.input.ends_with('\n') {
                self.input.pop();
            }
        }
        if stop_clicked {
            self.stop();
        }
        if (enter || send_clicked) && !busy && !self.input.trim().is_empty() {
            action = Some(Action::Send);
        }
        action
    }

    /// Sends the typed message and starts the model.
    pub fn send(&mut self, app: App, cfg: &Config, ctx: &egui::Context) {
        let text = std::mem::take(&mut self.input).trim().to_string();
        if text.is_empty() {
            return;
        }
        self.items.push(Item::User(text.clone()));
        self.history.push(json!({"role": "user", "content": text}));
        self.steps = 0;
        self.took = None;
        self.started = Some(Instant::now());
        self.focus_input = true;
        self.ask(app, cfg, ctx);
    }

    fn ask(&mut self, app: App, cfg: &Config, ctx: &egui::Context) {
        let mut messages = vec![json!({"role": "system", "content": system_prompt(app)})];
        messages.extend(self.history.iter().cloned());
        let (tx, rx) = channel();
        self.reply = Some(rx);
        self.streaming.clear();
        let tool_list = tools(app);
        let cfg = cfg.clone();
        let ctx = ctx.clone();
        let epoch = self.epoch;
        let _ = epoch;
        spawn_task(async move {
            stream_request(&cfg, messages, tool_list, &tx, &ctx).await;
            ctx.request_repaint();
        });
    }

    /// Advances the conversation: reads the model's reply as it streams, runs its tool calls on
    /// `host`, and asks again until it answers in words.
    pub fn poll(&mut self, host: &mut dyn Host, app: App, cfg: &Config, ctx: &egui::Context) {
        if self.reply.is_some() || !self.pending.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(60));
        }
        // Drain what has streamed in.
        let mut finished: Option<Result<Value, String>> = None;
        if let Some(rx) = &self.reply {
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    Ev::Text(t) => self.streaming.push_str(&t),
                    Ev::Done(r) => {
                        finished = Some(r);
                        break;
                    }
                }
            }
        }
        if let Some(r) = finished {
            self.reply = None;
            self.streaming.clear();
            match r {
                Err(e) => {
                    self.items.push(Item::Error(e));
                    self.started = None;
                }
                Ok(msg) => self.on_message(msg, host, app, cfg, ctx),
            }
        }
        if !self.pending.is_empty() {
            let mut still = Vec::new();
            for p in std::mem::take(&mut self.pending) {
                match p.rx.try_recv() {
                    Ok(r) => self.finish_call(&p.id, &p.name, r),
                    Err(std::sync::mpsc::TryRecvError::Empty) => still.push(p),
                    Err(_) => self.finish_call(&p.id, &p.name, Err("the app closed before answering".into())),
                }
            }
            self.pending = still;
            self.continue_if_ready(app, cfg, ctx);
        }
    }

    fn on_message(&mut self, msg: Value, host: &mut dyn Host, app: App, cfg: &Config, ctx: &egui::Context) {
        let calls = msg.get("tool_calls").and_then(Value::as_array).cloned().unwrap_or_default();
        let text = msg.get("content").and_then(Value::as_str).unwrap_or("").trim().to_string();
        self.history.push(msg);
        if calls.is_empty() {
            if text.is_empty() {
                self.items.push(Item::Error("The model returned an empty reply.".into()));
            } else {
                self.items.push(Item::Assistant(text));
            }
            self.took = self.started.map(|t| t.elapsed().as_secs_f32());
            self.started = None;
            return;
        }
        if !text.is_empty() {
            self.items.push(Item::Assistant(text));
        }
        self.steps += 1;
        if self.steps > MAX_STEPS {
            self.items.push(Item::Error("Stopped: too many steps for one request. Ask for a smaller change.".into()));
            self.started = None;
            // Keep the transcript valid for the next message: every call gets an answer.
            for c in &calls {
                let id = c.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                self.history.push(json!({"role": "tool", "tool_call_id": id, "content": "stopped"}));
            }
            return;
        }
        self.results.clear();
        let mut steps = Vec::new();
        let mut parsed = Vec::new();
        for c in calls {
            let id = c.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            let f = c.get("function").cloned().unwrap_or(Value::Null);
            let name = f.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let args = match f.get("arguments") {
                Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
                Some(v) => v.clone(),
                None => Value::Null,
            };
            steps.push(Step { id: id.clone(), label: describe(&name, &args), state: StepState::Running, edit: !is_read_only(&name, &args) });
            parsed.push((id, name, args));
        }
        self.items.push(Item::Steps(steps));
        for (id, name, args) in parsed {
            match run_tool(host, app, &name, &args) {
                CallResult::Done(r) => self.finish_call(&id, &name, r),
                CallResult::Pending(rx) => self.pending.push(PendingCall { id, name, rx }),
            }
        }
        self.continue_if_ready(app, cfg, ctx);
    }

    fn finish_call(&mut self, id: &str, name: &str, r: Result<Value, String>) {
        let (ok, content) = match r {
            Ok(v) => (true, truncate(&v)),
            Err(e) => (false, json!({"error": e}).to_string()),
        };
        // The step in the activity card turns done (or failed, with the reason).
        for it in self.items.iter_mut().rev() {
            if let Item::Steps(steps) = it
                && let Some(st) = steps.iter_mut().find(|s| s.id == id)
            {
                st.state = if ok { StepState::Done } else { StepState::Failed };
                if !ok {
                    let why: String = content.chars().take(140).collect();
                    st.label = format!("{} — {}", st.label, why.trim_matches(|c| c == '{' || c == '}'));
                }
                break;
            }
        }
        let _ = name;
        self.results.push(json!({"role": "tool", "tool_call_id": id, "content": content}));
    }

    fn continue_if_ready(&mut self, app: App, cfg: &Config, ctx: &egui::Context) {
        if self.pending.is_empty() && !self.results.is_empty() && self.reply.is_none() {
            self.history.append(&mut self.results);
            self.ask(app, cfg, ctx);
        }
    }
}

// ---- drawing -------------------------------------------------------------------------------

/// The assistant's mark: a four-point spark.
fn paint_spark(p: &egui::Painter, r: egui::Rect, color: egui::Color32) {
    let c = r.center();
    let (a, b) = (r.width() * 0.5, r.width() * 0.16);
    let pts = vec![
        egui::pos2(c.x, c.y - a),
        egui::pos2(c.x + b, c.y - b),
        egui::pos2(c.x + a, c.y),
        egui::pos2(c.x + b, c.y + b),
        egui::pos2(c.x, c.y + a),
        egui::pos2(c.x - b, c.y + b),
        egui::pos2(c.x - a, c.y),
        egui::pos2(c.x - b, c.y - b),
    ];
    p.add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
}

/// "qwen3-coder:30b" stays; a path-like id is cut to its last part.
fn short_model(m: &str) -> String {
    let m = m.rsplit('/').next().unwrap_or(m);
    if m.chars().count() > 22 { format!("{}…", m.chars().take(21).collect::<String>()) } else { m.to_string() }
}

fn round_button(ui: &mut egui::Ui, accent: egui::Color32, stop: bool, enabled: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), if enabled { egui::Sense::click() } else { egui::Sense::hover() });
    let fill = if enabled { if resp.hovered() { accent.gamma_multiply(0.85) } else { accent } } else { ui.visuals().widgets.inactive.bg_fill };
    let p = ui.painter();
    p.circle_filled(r.center(), 15.0, fill);
    let ink = if enabled { egui::Color32::WHITE } else { ui.visuals().weak_text_color() };
    let c = r.center();
    if stop {
        p.rect_filled(egui::Rect::from_center_size(c, egui::vec2(10.0, 10.0)), 2.0, ink);
    } else {
        let s = egui::Stroke::new(2.0, ink);
        p.line_segment([egui::pos2(c.x, c.y + 6.0), egui::pos2(c.x, c.y - 6.0)], s);
        p.line_segment([egui::pos2(c.x - 5.0, c.y - 1.0), egui::pos2(c.x, c.y - 6.0)], s);
        p.line_segment([egui::pos2(c.x + 5.0, c.y - 1.0), egui::pos2(c.x, c.y - 6.0)], s);
    }
    resp
}

fn user_bubble(ui: &mut egui::Ui, text: &str, accent: egui::Color32, ink: egui::Color32) {
    ui.add_space(10.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        let max = (ui.available_width() * 0.86).max(120.0);
        egui::Frame::new().fill(accent.gamma_multiply(0.16)).corner_radius(14).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.set_max_width(max);
            ui.add(egui::Label::new(egui::RichText::new(text).size(13.5).color(ink)).wrap());
        });
    });
}

fn assistant_message(ui: &mut egui::Ui, text: &str, accent: egui::Color32, ink: egui::Color32, muted: egui::Color32, last: bool, took: Option<f32>) {
    ui.add_space(10.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (r, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
        paint_spark(ui.painter(), r.shrink(1.0).translate(egui::vec2(0.0, 3.0)), accent);
        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            markdown(ui, text, ink, accent);
            if last {
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(egui::RichText::new("Copy").size(11.0).color(muted)).small().frame(false)).clicked() {
                        ui.ctx().copy_text(text.to_string());
                    }
                    if let Some(s) = took {
                        ui.label(egui::RichText::new(format!("{s:.0}s")).size(11.0).color(muted));
                    }
                });
            }
        });
    });
}

fn thinking_row(ui: &mut egui::Ui, accent: egui::Color32, muted: egui::Color32, secs: u64) {
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        let t = ui.input(|i| i.time);
        let (r, _) = ui.allocate_exact_size(egui::vec2(28.0, 12.0), egui::Sense::hover());
        for k in 0..3 {
            let phase = ((t * 3.0 - f64::from(k) * 0.6).sin() * 0.5 + 0.5) as f32;
            let c = egui::pos2(r.min.x + 4.0 + 10.0 * k as f32, r.center().y);
            ui.painter().circle_filled(c, 2.0 + 1.6 * phase, accent.gamma_multiply(0.35 + 0.65 * phase));
        }
        ui.label(egui::RichText::new(if secs >= 3 { format!("Working… {secs}s") } else { "Thinking…".to_string() }).size(12.0).color(muted));
    });
    ui.ctx().request_repaint_after(Duration::from_millis(60));
}

/// A run of tool calls, as a compact card: one row per call.
fn steps_card(ui: &mut egui::Ui, steps: &[Step], v: &egui::Visuals, muted: egui::Color32, ink: egui::Color32) {
    ui.add_space(8.0);
    egui::Frame::new().fill(v.faint_bg_color).corner_radius(10).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        for st in steps {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                let c = r.center();
                match st.state {
                    StepState::Running => {
                        let t = ui.input(|i| i.time) as f32;
                        let a = t * 5.0;
                        let (p0, p1) = (c + egui::vec2(a.cos(), a.sin()) * 5.0, c + egui::vec2((a + 2.4).cos(), (a + 2.4).sin()) * 5.0);
                        ui.painter().line_segment([p0, p1], egui::Stroke::new(2.0, muted));
                        ui.ctx().request_repaint_after(Duration::from_millis(60));
                    }
                    StepState::Done => {
                        let s = egui::Stroke::new(1.8, egui::Color32::from_rgb(31, 143, 92));
                        ui.painter().line_segment([c + egui::vec2(-4.0, 0.5), c + egui::vec2(-1.0, 3.5)], s);
                        ui.painter().line_segment([c + egui::vec2(-1.0, 3.5), c + egui::vec2(4.5, -3.5)], s);
                    }
                    StepState::Failed => {
                        let s = egui::Stroke::new(1.8, egui::Color32::from_rgb(194, 59, 69));
                        ui.painter().line_segment([c + egui::vec2(-3.5, -3.5), c + egui::vec2(3.5, 3.5)], s);
                        ui.painter().line_segment([c + egui::vec2(-3.5, 3.5), c + egui::vec2(3.5, -3.5)], s);
                    }
                }
                let col = if st.edit { ink } else { muted };
                ui.add(egui::Label::new(egui::RichText::new(&st.label).size(12.0).color(col)).wrap());
            });
        }
    });
}

/// The panel before the first message: a greeting and a few things to try. Returns the text of
/// the chip clicked and whether it should be sent at once.
fn empty_state(ui: &mut egui::Ui, app: App, accent: egui::Color32, ink: egui::Color32, muted: egui::Color32) -> Option<(String, bool)> {
    let mut out = None;
    ui.add_space(18.0);
    ui.vertical_centered(|ui| {
        let (r, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
        paint_spark(ui.painter(), r, accent);
        ui.add_space(6.0);
        ui.label(egui::RichText::new("How can I help?").size(17.0).strong().color(ink));
        ui.add_space(2.0);
        ui.label(egui::RichText::new(format!("I can read and change this {} for you.", app.noun())).size(12.5).color(muted));
    });
    ui.add_space(16.0);
    // (text, send at once): a trailing ellipsis is for the user to finish.
    let chips: &[(&str, bool)] = match app {
        App::Docs => &[
            ("Summarize this document", true),
            ("Fix spelling and grammar", true),
            ("Make it more concise", true),
            ("Draft a one-page report on…", false),
        ],
        App::Slides => &[("Summarize this deck", true), ("Add a summary slide", true), ("Tighten the bullet points", true), ("Draft a 5-slide deck on…", false)],
        App::Sheets => &[("Explain what this sheet does", true), ("Add a totals row", true), ("Create a sample budget table", true), ("Chart the first column against…", false)],
        App::Pdf => &[("What is in this file?", true), ("How many pages does it have?", true)],
    };
    for (text, send) in chips {
        let r = ui.add_sized(
            [ui.available_width(), 32.0],
            egui::Button::new(egui::RichText::new(*text).size(13.0).color(ink)).corner_radius(9).stroke(egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)),
        );
        if r.clicked() {
            out = Some((text.trim_end_matches('…').to_string(), *send));
        }
        ui.add_space(2.0);
    }
    out
}

/// Markdown, as far as a reply needs it: paragraphs, headings, bullet and numbered lists, code
/// fences, and **bold** / `code` inline.
fn markdown(ui: &mut egui::Ui, text: &str, ink: egui::Color32, accent: egui::Color32) {
    let code_bg = ui.visuals().faint_bg_color;
    let mut in_code = false;
    let mut code = String::new();
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.trim_start().starts_with("```") {
            if in_code {
                egui::Frame::new().fill(code_bg).corner_radius(6).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(egui::RichText::new(code.trim_end()).monospace().size(12.0).color(ink)).wrap());
                });
                code.clear();
            }
            in_code = !in_code;
            continue;
        }
        if in_code {
            code.push_str(line);
            code.push('\n');
            continue;
        }
        let t = line.trim_start();
        if t.is_empty() {
            ui.add_space(4.0);
        } else if let Some(h) = t.strip_prefix("### ").or_else(|| t.strip_prefix("## ")).or_else(|| t.strip_prefix("# ")) {
            ui.add_space(2.0);
            ui.add(egui::Label::new(egui::RichText::new(h).strong().size(14.5).color(ink)).wrap());
        } else if let Some(item) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_space(4.0);
                ui.label(egui::RichText::new("•").color(accent));
                ui.vertical(|ui| inline(ui, item, ink, accent));
            });
        } else if let Some((n, item)) = numbered(t) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_space(2.0);
                ui.label(egui::RichText::new(format!("{n}.")).color(accent));
                ui.vertical(|ui| inline(ui, item, ink, accent));
            });
        } else {
            inline(ui, t, ink, accent);
        }
    }
}

fn numbered(t: &str) -> Option<(&str, &str)> {
    let dot = t.find(". ")?;
    let n = &t[..dot];
    (!n.is_empty() && n.len() <= 3 && n.chars().all(|c| c.is_ascii_digit())).then(|| (n, &t[dot + 2..]))
}

/// One line with **bold** and `code` spans.
fn inline(ui: &mut egui::Ui, text: &str, ink: egui::Color32, _accent: egui::Color32) {
    use egui::text::{LayoutJob, TextFormat};
    let strong = ui.visuals().strong_text_color();
    let code_bg = ui.visuals().faint_bg_color;
    let size = 13.5;
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    let plain = TextFormat { font_id: egui::FontId::proportional(size), color: ink, ..Default::default() };
    let bold = TextFormat { font_id: egui::FontId::proportional(size), color: strong, underline: egui::Stroke::NONE, ..Default::default() };
    let mono = TextFormat { font_id: egui::FontId::monospace(size - 1.0), color: ink, background: code_bg, ..Default::default() };
    let mut rest = text;
    while !rest.is_empty() {
        let next_bold = rest.find("**");
        let next_code = rest.find('`');
        let (at, kind) = match (next_bold, next_code) {
            (Some(b), Some(c)) if c < b => (c, 'c'),
            (Some(b), _) => (b, 'b'),
            (None, Some(c)) => (c, 'c'),
            (None, None) => {
                job.append(rest, 0.0, plain.clone());
                break;
            }
        };
        job.append(&rest[..at], 0.0, plain.clone());
        let after = &rest[at..];
        if kind == 'b' {
            if let Some(end) = after[2..].find("**") {
                job.append(&after[2..2 + end], 0.0, bold.clone());
                rest = &after[2 + end + 2..];
            } else {
                job.append(after, 0.0, plain.clone());
                break;
            }
        } else if let Some(end) = after[1..].find('`') {
            job.append(&after[1..1 + end], 0.0, mono.clone());
            rest = &after[1 + end + 1..];
        } else {
            job.append(after, 0.0, plain.clone());
            break;
        }
    }
    ui.add(egui::Label::new(job).wrap());
}

// ---- step labels -----------------------------------------------------------------------------

/// The tools that only look.
fn is_read_only(name: &str, args: &Value) -> bool {
    match name {
        "inspect_document" | "inspect_slide" | "inspect_workbook" | "get_text" | "read_range" | "get_cell" | "list_commands" | "list_functions"
        | "evaluate_formula" => true,
        "run_command" | "execute" | "execute_command" => {
            let c = args.get("command").and_then(Value::as_str).unwrap_or("");
            c.starts_with("document.inspect") || c.ends_with(".get") || c.starts_with("find")
        }
        _ => false,
    }
}

fn count(args: &Value, key: &str) -> usize {
    args.get(key).and_then(Value::as_array).map_or(0, Vec::len)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn prettify(id: &str) -> String {
    let mut out = String::new();
    for (i, c) in id.chars().enumerate() {
        if c == '_' || c == '.' {
            out.push(' ');
        } else if c.is_uppercase() && i > 0 {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    let mut cs = out.chars();
    cs.next().map(|f| f.to_uppercase().collect::<String>() + cs.as_str()).unwrap_or_default()
}

/// What a tool call says in the activity card.
fn describe(name: &str, args: &Value) -> String {
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("");
    match name {
        "inspect_document" | "inspect_slide" | "inspect_workbook" | "get_text" => "Read the file".into(),
        "read_range" => format!("Read {}", s("range")),
        "get_cell" => format!("Read {}", s("cell")),
        "list_commands" | "list_functions" => "Looked up commands".into(),
        "insert_blocks" => format!("Wrote {}", plural(count(args, "blocks"), "paragraph", "paragraphs")),
        "replace_text" => format!("Replaced “{}” with “{}”", s("find"), s("replace")),
        "type_text" => "Typed text".into(),
        "select_text" => format!("Selected “{}”", s("text")),
        "build_slides" => format!("Added {}", plural(count(args, "slides"), "slide", "slides")),
        "add_slide" => "Added a slide".into(),
        "add_text_box" => "Added a text box".into(),
        "add_shape" => "Added a shape".into(),
        "set_text" => "Set text".into(),
        "insert_table" | "create_table" => "Added a table".into(),
        "insert_chart" => "Added a chart".into(),
        "insert_picture" => "Added a picture".into(),
        "write_range" => format!("Wrote {}", s("range")),
        "set_cell" => format!("Set {}", s("cell")),
        "format_range" => format!("Formatted {}", s("range")),
        "sort_range" => format!("Sorted {}", s("range")),
        "filter" => "Filtered".into(),
        "go_to_slide" => "Went to a slide".into(),
        "select" => "Selected".into(),
        "batch" => format!("Ran {}", plural(count(args, "commands"), "command", "commands")),
        "run_command" | "execute" | "execute_command" => format!("Ran {}", prettify(s("command"))),
        other => prettify(other),
    }
}

fn truncate(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() <= MAX_RESULT_CHARS {
        return s;
    }
    let cut: String = s.chars().take(MAX_RESULT_CHARS).collect();
    format!("{cut}…[truncated; ask for a narrower range or use a filter]")
}

fn system_prompt(app: App) -> String {
    let kind = app.noun();
    let how = match app {
        App::Docs => {
            " Read the text with get_text or inspect_document. To add or draft content call insert_blocks \
              once with every paragraph and its style (Heading 1, Normal, Bullet...); do not type it piece \
              by piece. To change text in place use replace_text, or select_text then a command such as \
              format.bold or para.style with execute. Use batch to run several commands in one go."
        }
        App::Slides => {
            " Look with inspect_document and inspect_slide. To draft or extend a deck call build_slides once \
              with every slide's title and bullets; a new presentation's empty first slide is replaced by it. \
              Body lines are bullets already: do not reformat them. For single changes use add_text_box, \
              set_text, add_shape, insert_table and insert_chart, run_command for anything else and \
              batch for several commands in a row."
        }
        App::Sheets => {
            " Look with inspect_workbook and read_range. Change cells with set_cell or write_range \
              (typed input: numbers, text or =formulas); format_range, create_table, sort_range and \
              insert_chart cover the rest, and execute_command runs any command by id."
        }
        App::Pdf => " This is a PDF editor; inspect_document reports the open file and view state. Find commands with list_commands and run them with run_command.",
    };
    format!(
        "You are the assistant built into OfficeCraft, working inside the user's open {kind}. \
         You act on the live {kind} only through your tools, and your changes appear in it right away \
         (the user can undo them). Look before you change anything. Use the smallest change that does \
         what was asked, and never remove content unless asked. Do not invent command ids or \
         parameters; if a call returns an error, read it and correct the call. When done, say what you \
         changed in one or two plain sentences.{how}"
    )
}

/// Tools that need the app's window, a file dialog or whole-file replacement: not for a model
/// working on the open file from inside the app.
fn agent_tool(name: &str) -> bool {
    const SKIP: &[&str] = &[
        "screenshot", "click", "key", "pointer", "select_tool", "ui_inspect", "parity", "call_app", "undo", "redo", "steps",
    ];
    !SKIP.contains(&name)
        && !name.starts_with("new_")
        && !name.starts_with("open_")
        && !name.starts_with("save_")
        && !name.starts_with("render_")
        && name != "export"
}

/// The app's own MCP tool definitions, as the model's function list.
fn mcp_definitions(app: App) -> Option<Vec<Value>> {
    let defs: Vec<Value> = match app {
        App::Docs => match wordcraft_mcp::tools::tool_definitions() {
            Value::Array(a) => a,
            _ => return None,
        },
        App::Slides => deckcraft_mcp::tool_definitions(),
        App::Sheets => gridcraft_mcp::tool_definitions(),
        _ => return None,
    };
    let mut out: Vec<Value> = defs
        .into_iter()
        .filter(|d| d.get("name").and_then(Value::as_str).is_some_and(agent_tool))
        .map(|d| {
            json!({"type": "function", "function": {
                "name": d.get("name"),
                "description": d.get("description"),
                "parameters": d.get("inputSchema").cloned().unwrap_or_else(|| json!({"type": "object", "properties": {}})),
            }})
        })
        .collect();
    if app == App::Docs {
        out.push(json!({"type": "function", "function": {
            "name": "insert_blocks",
            "description": "Write paragraphs into the document in one step, each with its own style: a heading, \
                            body text, a bullet or numbered list item. Use this to add or draft content.",
            "parameters": {"type": "object", "properties": {
                "blocks": {"type": "array", "description": "in reading order", "items": {"type": "object", "properties": {
                    "text": {"type": "string"},
                    "style": {"type": "string", "enum": ["Title", "Heading 1", "Heading 2", "Heading 3", "Normal", "Bullet", "Number", "Quote"]}},
                    "required": ["text"]}},
                "at": {"type": "string", "enum": ["end", "start", "cursor"], "description": "where to write (default end)"}},
                "required": ["blocks"]}}}));
        out.push(json!({"type": "function", "function": {
            "name": "replace_text",
            "description": "Replace every occurrence of some text in the document.",
            "parameters": {"type": "object", "properties": {
                "find": {"type": "string"}, "replace": {"type": "string"}, "match_case": {"type": "boolean"}},
                "required": ["find", "replace"]}}}));
    }
    if app == App::Slides {
        out.push(json!({"type": "function", "function": {
            "name": "build_slides",
            "description": "Add slides in one step: each has a title and optional bullet points. A new \
                            presentation's empty first slide is replaced. Use this to draft or extend a deck.",
            "parameters": {"type": "object", "properties": {
                "slides": {"type": "array", "description": "in order", "items": {"type": "object", "properties": {
                    "title": {"type": "string"},
                    "bullets": {"type": "array", "items": {"type": "string"}},
                    "layout": {"type": "string", "enum": ["title", "titleAndContent", "sectionHeader", "twoContent", "titleOnly", "blank"],
                               "description": "default: title for a slide without bullets first in the deck, else titleAndContent"}},
                    "required": ["title"]}}},
                "required": ["slides"]}}}));
    }
    Some(out)
}

/// `build_slides`: slides go in through the deck's own `slide.new`, bullets as the body lines.
fn build_slides(host: &mut dyn Host, args: &Value) -> Result<Value, String> {
    let slides = args.get("slides").and_then(Value::as_array).ok_or("build_slides needs `slides`")?;
    if slides.is_empty() {
        return Err("`slides` is empty".into());
    }
    let call = |host: &mut dyn Host, method: &str, params: Value| -> Result<Value, String> {
        match host.call(method, params) {
            CallResult::Done(r) => r,
            CallResult::Pending(_) => Err("that needs the app's window".into()),
        }
    };
    // A new presentation: one empty title slide, which the first slide here replaces.
    let before = call(host, "document.inspect", json!({}))?;
    let existing = before.get("slides").and_then(Value::as_array).cloned().unwrap_or_default();
    let pristine = existing.len() == 1 && existing[0].get("title").and_then(Value::as_str).is_some_and(str::is_empty);
    let mut made = 0;
    for (i, sl) in slides.iter().enumerate() {
        let title = sl.get("title").and_then(Value::as_str).unwrap_or("");
        let bullets: Vec<&str> = sl.get("bullets").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let layout = sl.get("layout").and_then(Value::as_str).unwrap_or(if bullets.is_empty() && i == 0 && existing.len() <= 1 { "title" } else { "titleAndContent" });
        let mut p = json!({"layout": layout, "title": title});
        if !bullets.is_empty() {
            p["body"] = json!(bullets.join("\n"));
        }
        call(host, "engine.execute", json!({"command": "slide.new", "params": p})).map_err(|e| format!("slide {}: {e}", i + 1))?;
        made += 1;
    }
    if pristine {
        call(host, "engine.execute", json!({"command": "slide.delete", "params": {"index": 0}}))?;
    }
    Ok(json!({"slides_added": made}))
}

/// `insert_blocks`: the paragraphs go in one by one through the document's own commands, so the
/// model never has to juggle the caret, styles and list state itself.
fn insert_blocks(host: &mut dyn Host, args: &Value) -> Result<Value, String> {
    let blocks = args.get("blocks").and_then(Value::as_array).ok_or("insert_blocks needs `blocks`")?;
    if blocks.is_empty() {
        return Err("`blocks` is empty".into());
    }
    // Start on a paragraph of its own: leave the one the caret is in alone unless it is empty.
    let at = args.get("at").and_then(Value::as_str).unwrap_or("end");
    let blank = match host.call("document.text", json!({})) {
        CallResult::Done(Ok(v)) => {
            let t = v.get("text").and_then(Value::as_str).map(str::to_string).or_else(|| v.as_str().map(str::to_string)).unwrap_or_default();
            let t = t.trim_end_matches('\n');
            match at {
                "start" => t.split('\n').next().is_some_and(|l| l.trim().is_empty()),
                "cursor" => false,
                _ => t.rsplit('\n').next().is_some_and(|l| l.trim().is_empty()),
            }
        }
        _ => false,
    };
    let mut run = |command: &str, params: Value| -> Result<Value, String> {
        match host.call("engine.execute", json!({"command": command, "params": params})) {
            CallResult::Done(r) => r,
            CallResult::Pending(_) => Err("that needs the app's window".into()),
        }
    };
    match args.get("at").and_then(Value::as_str).unwrap_or("end") {
        "start" => {
            run("caret.docStart", json!({}))?;
        }
        "cursor" => {}
        _ => {
            run("caret.docEnd", json!({}))?;
        }
    }
    let at_start = args.get("at").and_then(Value::as_str) == Some("start");
    let mut first = true;
    for (i, b) in blocks.iter().enumerate() {
        let text = b.get("text").and_then(Value::as_str).unwrap_or("");
        let style = b.get("style").and_then(Value::as_str).unwrap_or("Normal");
        if !(first && blank) && (!first || !at_start) {
            run("text.newParagraph", json!({})).map_err(|e| format!("block {i}: {e}"))?;
        }
        first = false;
        // A fresh paragraph: plain, then the style asked for.
        let (para_style, list) = match style {
            "Bullet" => ("Normal", Some("para.bullets")),
            "Number" => ("Normal", Some("para.numbering")),
            other => (other, None),
        };
        run("para.style", json!({"style": para_style})).map_err(|e| format!("block {i}: {e}"))?;
        if let Some(cmd) = list {
            run(cmd, json!({})).map_err(|e| format!("block {i}: {e}"))?;
        }
        if !text.is_empty() {
            run("text.insert", json!({"text": text})).map_err(|e| format!("block {i}: {e}"))?;
        }
    }
    // Leave the paragraph after the last block plain, so the next typing is not a list item.
    if at_start {
        run("text.newParagraph", json!({})).ok();
        run("para.style", json!({"style": "Normal"})).ok();
    }
    Ok(json!({"inserted": blocks.len()}))
}

/// Lets an app's MCP tools call the open document through the host.
struct Bridge<'a>(&'a mut dyn Host);

impl Bridge<'_> {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        match self.0.call(method, params) {
            CallResult::Done(r) => r,
            CallResult::Pending(_) => Err("that needs the app's window".into()),
        }
    }
}

impl wordcraft_mcp::Backend for Bridge<'_> {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        Bridge::call(self, method, params)
    }
    fn has_ui(&self) -> bool {
        true
    }
    fn describe(&self) -> String {
        "OfficeCraft".into()
    }
}

impl deckcraft_mcp::Backend for Bridge<'_> {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        Bridge::call(self, method, params)
    }
    fn has_ui(&self) -> bool {
        true
    }
    fn describe(&self) -> String {
        "OfficeCraft".into()
    }
}

impl gridcraft_mcp::Backend for Bridge<'_> {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        Bridge::call(self, method, params)
    }
    fn has_ui(&self) -> bool {
        true
    }
    fn describe(&self) -> String {
        "OfficeCraft".into()
    }
}

/// An MCP tool's reply as the text the model reads (JSON stays JSON).
fn mcp_text(content: &[Value], is_error: bool) -> Result<Value, String> {
    let text: String = content.iter().filter_map(|c| c.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n");
    if is_error {
        return Err(text);
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

fn tools(app: App) -> Value {
    if let Some(defs) = mcp_definitions(app) {
        return Value::Array(defs);
    }
    let mut t = vec![
        json!({"type": "function", "function": {
            "name": "inspect_document",
            "description": "Summarise the open file: its content and structure, and the current selection.",
            "parameters": {"type": "object", "properties": {}}}}),
        json!({"type": "function", "function": {
            "name": "list_commands",
            "description": "List the commands this app can run, with their parameter docs. Pass a filter (a word from the command id or label) to narrow the list.",
            "parameters": {"type": "object", "properties": {"filter": {"type": "string"}}}}}),
        json!({"type": "function", "function": {
            "name": "run_command",
            "description": "Run one command by id with its parameters. Changes the open file.",
            "parameters": {"type": "object", "properties": {
                "command": {"type": "string", "description": "a command id from list_commands"},
                "params": {"type": "object", "description": "the command's parameters"}},
                "required": ["command"]}}}),
    ];
    if app == App::Sheets {
        t.push(json!({"type": "function", "function": {
            "name": "read_range",
            "description": "Read cell values from a range such as A1:D20.",
            "parameters": {"type": "object", "properties": {
                "range": {"type": "string"}, "sheet": {"type": "string"}, "formulas": {"type": "boolean"}}}}}));
    }
    Value::Array(t)
}

/// Maps the model's tool vocabulary onto each app's control methods.
fn run_tool(host: &mut dyn Host, app: App, name: &str, args: &Value) -> CallResult {
    if app == App::Slides && name == "build_slides" {
        return CallResult::Done(build_slides(host, args));
    }
    if app == App::Docs {
        match name {
            "insert_blocks" => return CallResult::Done(insert_blocks(host, args)),
            "replace_text" => {
                let find = args.get("find").and_then(Value::as_str).unwrap_or("");
                let with = args.get("replace").and_then(Value::as_str).unwrap_or("");
                if find.is_empty() {
                    return CallResult::Done(Err("`find` must not be empty".into()));
                }
                let mut p = json!({"text": find, "with": with});
                if let Some(c) = args.get("match_case").and_then(Value::as_bool) {
                    p["matchCase"] = json!(c);
                }
                return host.call("engine.execute", json!({"command": "edit.replaceAll", "params": p}));
            }
            _ => {}
        }
    }
    if mcp_definitions(app).is_some() {
        if !agent_tool(name) {
            return CallResult::Done(Err(format!("unknown tool {name}")));
        }
        let mut b = Bridge(host);
        let r = match app {
            App::Docs => {
                let r = wordcraft_mcp::tools::call_tool(&mut b, name, args);
                mcp_text(&r.content, r.is_error)
            }
            App::Slides => {
                let r = deckcraft_mcp::call_tool(&mut b, name, args);
                mcp_text(&r.content, r.is_error)
            }
            _ => {
                let r = gridcraft_mcp::call_tool(&mut b, name, args);
                mcp_text(&r.content, r.is_error)
            }
        };
        return CallResult::Done(r);
    }
    let pdf = app == App::Pdf;
    match name {
        "inspect_document" => {
            if pdf {
                host.call("ui.state", json!({}))
            } else {
                host.call("document.inspect", json!({"text": true}))
            }
        }
        "list_commands" => {
            let filter = args.get("filter").and_then(Value::as_str).unwrap_or("").to_lowercase();
            let method = if pdf { "ui.commands" } else { "engine.commands" };
            match host.call(method, json!({})) {
                CallResult::Done(r) => CallResult::Done(r.map(|v| filter_commands(v, &filter))),
                // The filter is applied by the model's next question; PDF answers whole.
                other => other,
            }
        }
        "run_command" => {
            let Some(cmd) = args.get("command").and_then(Value::as_str) else {
                return CallResult::Done(Err("run_command needs a `command` id".into()));
            };
            let params = args.get("params").cloned().unwrap_or_else(|| json!({}));
            if pdf {
                host.call("ui.command", json!({"id": cmd, "params": params}))
            } else {
                host.call("engine.execute", json!({"command": cmd, "params": params}))
            }
        }
        "read_range" if app == App::Sheets => host.call("sheet.read", args.clone()),
        other => CallResult::Done(Err(format!("unknown tool {other}"))),
    }
}

fn filter_commands(v: Value, filter: &str) -> Value {
    let Value::Array(items) = v else { return v };
    let total = items.len();
    let keep: Vec<Value> = items
        .into_iter()
        .filter(|c| {
            filter.is_empty()
                || ["id", "label"].iter().any(|k| c.get(*k).and_then(Value::as_str).is_some_and(|s| s.to_lowercase().contains(filter)))
        })
        .take(60)
        .map(|c| json!({"id": c.get("id"), "label": c.get("label"), "params": c.get("params"), "enabled": c.get("enabled")}))
        .collect();
    json!({"total": total, "shown": keep.len(), "commands": keep})
}

/// Runs `fut` off the UI thread: a worker thread natively (the blocking HTTP client parks in
/// it), the browser's event loop on the web.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_task(fut: impl std::future::Future<Output = ()> + Send + 'static) {
    std::thread::spawn(move || block_on(fut));
}

#[cfg(target_arch = "wasm32")]
fn spawn_task(fut: impl std::future::Future<Output = ()> + 'static) {
    wasm_bindgen_futures::spawn_local(fut);
}

/// Drives a future whose awaits never actually wait (the native transport blocks inside them).
#[cfg(not(target_arch = "wasm32"))]
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, Wake, Waker};
    struct Noop;
    impl Wake for Noop {
        fn wake(self: std::sync::Arc<Self>) {}
    }
    let waker = Waker::from(std::sync::Arc::new(Noop));
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

/// A reply being assembled from server-sent events, one `data:` line at a time.
#[derive(Default)]
struct StreamAcc {
    content: String,
    /// Tool calls arrive in pieces keyed by index: (id, name, arguments so far).
    calls: Vec<(String, String, String)>,
    saw_event: bool,
}

impl StreamAcc {
    /// Takes one line of the stream. `Ok(true)` once the server said `[DONE]`.
    fn feed(&mut self, line: &str, url: &str, tx: &std::sync::mpsc::Sender<Ev>, ctx: &egui::Context) -> Result<bool, String> {
        let Some(data) = line.strip_prefix("data:") else { return Ok(false) };
        let data = data.trim();
        if data == "[DONE]" {
            return Ok(true);
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else { return Ok(false) };
        if let Some(e) = v.pointer("/error/message").and_then(Value::as_str) {
            return Err(format!("{url}: {e}"));
        }
        self.saw_event = true;
        let Some(delta) = v.pointer("/choices/0/delta") else { return Ok(false) };
        if let Some(t) = delta.get("content").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            self.content.push_str(t);
            let _ = tx.send(Ev::Text(t.to_string()));
            ctx.request_repaint();
        }
        if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
            for tc in tcs {
                let i = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                while self.calls.len() <= i {
                    self.calls.push((String::new(), String::new(), String::new()));
                }
                if let Some(id) = tc.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                    self.calls[i].0 = id.to_string();
                }
                if let Some(f) = tc.get("function") {
                    if let Some(n) = f.get("name").and_then(Value::as_str) {
                        self.calls[i].1.push_str(n);
                    }
                    if let Some(a) = f.get("arguments").and_then(Value::as_str) {
                        self.calls[i].2.push_str(a);
                    }
                }
            }
        }
        Ok(false)
    }

    /// The whole assistant message, with its tool calls.
    fn finish(self, url: &str) -> Result<Value, String> {
        let StreamAcc { mut content, mut calls, saw_event } = self;
        if !saw_event {
            return Err(format!("{url} did not stream a reply."));
        }
        // Some models write their tool calls as text (`<function=name><parameter=key>value...`) and the
        // server does not always turn that into a structured call: do it here.
        if calls.iter().all(|(_, n, _)| n.is_empty()) && content.contains("<function=") {
            let (text, found) = salvage_tool_calls(&content);
            if !found.is_empty() {
                content = text;
                calls = found;
            }
        }
        let mut msg = json!({"role": "assistant", "content": content});
        let calls: Vec<Value> = calls
            .into_iter()
            .enumerate()
            .filter(|(_, (_, n, _))| !n.is_empty())
            .map(|(i, (id, name, args))| {
                let id = if id.is_empty() { format!("call_{i}") } else { id };
                json!({"id": id, "type": "function", "function": {"name": name, "arguments": if args.is_empty() { "{}".to_string() } else { args }}})
            })
            .collect();
        if !calls.is_empty() {
            msg["tool_calls"] = Value::Array(calls);
        }
        Ok(msg)
    }
}

/// The server's own words for a failed request.
fn error_message(url: &str, status: u16, body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let msg = v.pointer("/error/message").and_then(Value::as_str).or_else(|| v.get("error").and_then(Value::as_str)).unwrap_or("request failed");
    format!("{url} answered {status}: {msg}")
}

/// A model list from a `GET /models` body.
fn parse_models(status: u16, text: &str) -> Result<Vec<String>, String> {
    if !(200..300).contains(&status) {
        let hint = if status == 401 || status == 403 { " (check the API key)" } else { "" };
        return Err(format!("HTTP {status}{hint}"));
    }
    let v: Value = serde_json::from_str(text).map_err(|_| "The endpoint did not return a model list".to_string())?;
    let list = v.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(list.iter().filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string)).collect())
}

/// One chat-completions round trip, streamed: the words go to `tx` as they arrive, and the whole
/// assistant message (with its tool calls) comes last.
async fn stream_request(cfg: &Config, messages: Vec<Value>, tools: Value, tx: &std::sync::mpsc::Sender<Ev>, ctx: &egui::Context) {
    let r = stream_inner(cfg, messages, tools, tx, ctx).await;
    let _ = tx.send(Ev::Done(r));
}

#[cfg(not(target_arch = "wasm32"))]
async fn stream_inner(cfg: &Config, messages: Vec<Value>, tools: Value, tx: &std::sync::mpsc::Sender<Ev>, ctx: &egui::Context) -> Result<Value, String> {
    use std::io::{BufRead, BufReader};
    let url = format!("{}/chat/completions", cfg.base_url);
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(300))).http_status_as_error(false).build().into();
    let mut req = agent.post(&url);
    if let Some(k) = &cfg.key {
        req = req.header("Authorization", &format!("Bearer {k}"));
    }
    let body = json!({"model": cfg.model, "messages": messages, "tools": tools, "stream": true});
    let mut resp = req.header("Content-Type", "application/json").send(body.to_string()).map_err(|e| format!("Can't reach {url}: {e}"))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        return Err(error_message(&url, status, &text));
    }
    let mut acc = StreamAcc::default();
    let reader = BufReader::new(resp.body_mut().as_reader());
    for line in reader.lines() {
        let line = line.map_err(|e| format!("The stream from {url} broke: {e}"))?;
        if acc.feed(&line, &url, tx, ctx)? {
            break;
        }
    }
    acc.finish(&url)
}

/// The browser's `fetch`, with the response body read chunk by chunk.
#[cfg(target_arch = "wasm32")]
async fn stream_inner(cfg: &Config, messages: Vec<Value>, tools: Value, tx: &std::sync::mpsc::Sender<Ev>, ctx: &egui::Context) -> Result<Value, String> {
    use wasm_bindgen::JsCast as _;
    let url = format!("{}/chat/completions", cfg.base_url);
    let body = json!({"model": cfg.model, "messages": messages, "tools": tools, "stream": true});
    let resp = fetch(&url, "POST", cfg.key.as_deref(), Some(body.to_string()), 300).await?;
    let status = resp.status();
    if !(200..300).contains(&status) {
        let text = response_text(&resp).await.unwrap_or_default();
        return Err(error_message(&url, status, &text));
    }
    let stream = resp.body().ok_or_else(|| format!("{url} did not stream a reply."))?;
    let reader: web_sys::ReadableStreamDefaultReader = stream.get_reader().dyn_into().map_err(|_| "no stream reader".to_string())?;
    let mut acc = StreamAcc::default();
    let mut pending: Vec<u8> = Vec::new();
    'read: loop {
        let chunk = wasm_bindgen_futures::JsFuture::from(reader.read()).await.map_err(|e| format!("The stream from {url} broke: {e:?}"))?;
        let done = js_sys::Reflect::get(&chunk, &"done".into()).ok().and_then(|d| d.as_bool()).unwrap_or(true);
        if let Ok(value) = js_sys::Reflect::get(&chunk, &"value".into())
            && !value.is_undefined()
        {
            pending.extend(js_sys::Uint8Array::new(&value).to_vec());
        }
        // Whole lines only: a chunk can end in the middle of one (or of a UTF-8 character).
        while let Some(nl) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line);
            if acc.feed(line.trim_end_matches(['\r', '\n']), &url, tx, ctx)? {
                break 'read;
            }
        }
        if done {
            if !pending.is_empty() {
                let line = String::from_utf8_lossy(&pending).into_owned();
                acc.feed(line.trim_end_matches(['\r', '\n']), &url, tx, ctx)?;
            }
            break;
        }
    }
    let _ = wasm_bindgen_futures::JsFuture::from(reader.cancel()).await;
    acc.finish(&url)
}

/// `GET {base}/models` with the key, as a list of ids.
#[cfg(not(target_arch = "wasm32"))]
async fn fetch_models(cfg: &Config, timeout_secs: u64) -> Result<Vec<String>, String> {
    let url = format!("{}/models", cfg.base_url);
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(timeout_secs))).http_status_as_error(false).build().into();
    let mut req = agent.get(&url);
    if let Some(k) = &cfg.key {
        req = req.header("Authorization", &format!("Bearer {k}"));
    }
    let mut resp = req.call().map_err(|e| format!("Can't reach {url}: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    parse_models(status, &text)
}

#[cfg(target_arch = "wasm32")]
async fn fetch_models(cfg: &Config, timeout_secs: u64) -> Result<Vec<String>, String> {
    let url = format!("{}/models", cfg.base_url);
    let resp = fetch(&url, "GET", cfg.key.as_deref(), None, timeout_secs).await?;
    let status = resp.status();
    let text = response_text(&resp).await?;
    parse_models(status, &text)
}

/// `fetch(url)` with a JSON body, an optional bearer key and a timeout.
#[cfg(target_arch = "wasm32")]
async fn fetch(url: &str, method: &str, key: Option<&str>, body: Option<String>, timeout_secs: u64) -> Result<web_sys::Response, String> {
    use wasm_bindgen::JsCast as _;
    let js = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let headers = web_sys::Headers::new().map_err(js)?;
    if let Some(k) = key {
        headers.set("Authorization", &format!("Bearer {k}")).map_err(js)?;
    }
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    init.set_mode(web_sys::RequestMode::Cors);
    if let Some(b) = body {
        headers.set("Content-Type", "application/json").map_err(js)?;
        init.set_body(&wasm_bindgen::JsValue::from_str(&b));
    }
    init.set_headers(&headers);
    let signal = web_sys::AbortSignal::timeout_with_u32(u32::try_from(timeout_secs.saturating_mul(1000)).unwrap_or(u32::MAX));
    init.set_signal(Some(&signal));
    let window = web_sys::window().ok_or("no window")?;
    let promise = window.fetch_with_str_and_init(url, &init);
    let resp = wasm_bindgen_futures::JsFuture::from(promise).await.map_err(|_| {
        // The page cannot tell a CORS refusal from a server that is not there.
        format!("Can't reach {url}. The server may be off, or it does not allow requests from this page (CORS).")
    })?;
    resp.dyn_into::<web_sys::Response>().map_err(|_| "not a response".to_string())
}

#[cfg(target_arch = "wasm32")]
async fn response_text(resp: &web_sys::Response) -> Result<String, String> {
    let promise = resp.text().map_err(|e| format!("{e:?}"))?;
    let v = wasm_bindgen_futures::JsFuture::from(promise).await.map_err(|e| format!("{e:?}"))?;
    Ok(v.as_string().unwrap_or_default())
}

static MODELS: std::sync::OnceLock<std::sync::Mutex<(bool, Vec<String>)>> = std::sync::OnceLock::new();

/// Forget the cached model list (the provider or endpoint changed); the next `models` refetches.
pub fn refresh_models() {
    if let Some(c) = MODELS.get()
        && let Ok(mut g) = c.lock()
    {
        *g = (false, Vec::new());
    }
}

/// The model ids the server offers (`GET /models`), fetched once in the background.
pub fn models(cfg: &Config, ctx: &egui::Context) -> Vec<String> {
    let cell = MODELS.get_or_init(|| std::sync::Mutex::new((false, Vec::new())));
    let start = {
        let Ok(mut g) = cell.lock() else { return Vec::new() };
        let start = !g.0;
        g.0 = true;
        start
    };
    if start {
        let cfg = cfg.clone();
        let ctx = ctx.clone();
        spawn_task(async move {
            let ids = fetch_models(&cfg, 8).await.unwrap_or_default();
            if let Some(c) = MODELS.get()
                && let Ok(mut g) = c.lock()
            {
                g.1 = ids;
            }
            ctx.request_repaint();
        });
    }
    cell.lock().map(|g| g.1.clone()).unwrap_or_default()
}

/// Tool calls written as `<function=NAME><parameter=KEY>VALUE</parameter>...</function>` in the
/// reply's text (qwen's coder format). Returns the text with them removed, and the calls as
/// (id, name, JSON arguments).
fn salvage_tool_calls(text: &str) -> (String, Vec<(String, String, String)>) {
    let mut out = String::new();
    let mut calls = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("<function=") {
        out.push_str(&rest[..at]);
        let after = &rest[at + "<function=".len()..];
        let Some(close) = after.find('>') else { break };
        let name = after[..close].trim().to_string();
        let body_start = &after[close + 1..];
        let (body, tail) = match body_start.find("</function>") {
            Some(e) => (&body_start[..e], &body_start[e + "</function>".len()..]),
            None => (body_start, ""),
        };
        let mut args = serde_json::Map::new();
        let mut b = body;
        while let Some(p) = b.find("<parameter=") {
            let rest_p = &b[p + "<parameter=".len()..];
            let Some(c) = rest_p.find('>') else { break };
            let key = rest_p[..c].trim().to_string();
            let val_all = &rest_p[c + 1..];
            let (val, next) = match val_all.find("</parameter>") {
                Some(e) => (&val_all[..e], &val_all[e + "</parameter>".len()..]),
                None => (val_all, ""),
            };
            let v = val.trim();
            let parsed = serde_json::from_str::<Value>(v).ok().filter(|j| j.is_array() || j.is_object() || j.is_number() || j.is_boolean());
            args.insert(key, parsed.unwrap_or_else(|| Value::String(v.to_string())));
            b = next;
        }
        if !name.is_empty() {
            calls.push((format!("call_{}", calls.len()), name, Value::Object(args).to_string()));
        }
        rest = tail;
    }
    out.push_str(rest);
    // The wrapper tags the model put around them.
    let cleaned = out.replace("<tool_call>", "").replace("</tool_call>", "");
    (cleaned.trim().to_string(), calls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_call_written_as_text_is_recovered() {
        let text = "I'll add it.\n\n<function=insert_blocks>\n<parameter=blocks>\n[{\"style\":\"Heading 1\",\"text\":\"Notes\"}]\n</parameter>\n<parameter=at>\nend\n</parameter>\n</function>\n</tool_call>";
        let (said, calls) = salvage_tool_calls(text);
        assert_eq!(said, "I'll add it.");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "insert_blocks");
        let args: Value = serde_json::from_str(&calls[0].2).unwrap();
        assert_eq!(args["blocks"][0]["text"], "Notes");
        assert_eq!(args["at"], "end");
    }

    #[test]
    fn plain_text_has_no_tool_calls() {
        let (said, calls) = salvage_tool_calls("Done. I added a heading.");
        assert_eq!(said, "Done. I added a heading.");
        assert!(calls.is_empty());
    }

    #[test]
    fn markdown_numbers_are_read() {
        assert_eq!(numbered("2. Budget"), Some(("2", "Budget")));
        assert_eq!(numbered("2023. a"), None);
        assert_eq!(numbered("hello. there"), None);
    }

    #[test]
    fn step_labels_are_plain_words() {
        assert_eq!(describe("insert_blocks", &json!({"blocks": [{}, {}, {}]})), "Wrote 3 paragraphs");
        assert_eq!(describe("build_slides", &json!({"slides": [{}]})), "Added 1 slide");
        assert_eq!(describe("run_command", &json!({"command": "format.bold"})), "Ran Format bold");
        assert!(is_read_only("get_text", &json!({})) && !is_read_only("insert_blocks", &json!({})));
    }

    #[test]
    fn settings_round_trip_and_keep_keys_in_their_own_private_file() {
        let dir = std::env::temp_dir().join(format!("officecraft-ai-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: no other test reads the environment or the config directory.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &dir);
            for k in ["OFFICECRAFT_AI_URL", "OFFICECRAFT_AI_MODEL", "OFFICECRAFT_AI_KEY"] {
                std::env::remove_var(k);
            }
        }
        let mut c = Config::load();
        assert_eq!((c.provider, c.base_url.as_str(), c.model.as_str()), (Provider::Local, DEFAULT_BASE, DEFAULT_MODEL));
        c.local_url = "http://127.0.0.1:1234/v1/".into();
        c.keys[1] = "cloud-secret".into();
        c.keys[2] = "router-secret".into();
        c.provider = Provider::OpenRouter;
        c.apply();
        c.save().unwrap();
        let settings = std::fs::read_to_string(dir.join("officecraft/ai.json")).unwrap();
        assert!(!settings.contains("secret"), "a key leaked into ai.json");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("officecraft/ai-keys.json")).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "keys file is readable by others");
        }
        let c = Config::load();
        assert_eq!(c.provider, Provider::OpenRouter);
        assert_eq!(c.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(c.model, "openai/gpt-oss-120b");
        assert_eq!(c.key.as_deref(), Some("router-secret"));
        let mut c = c;
        c.provider = Provider::Local;
        c.apply();
        assert_eq!(c.base_url, "http://127.0.0.1:1234/v1");
        assert_eq!(c.key, None);
        c.provider = Provider::Cloud;
        c.set_model("gpt-oss:20b");
        assert_eq!((c.base_url.as_str(), c.model.as_str(), c.key.as_deref()), ("https://ollama.com/v1", "gpt-oss:20b", Some("cloud-secret")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
