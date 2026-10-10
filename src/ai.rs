//! The assistant panel: a chat that drives the open document through the app's own command
//! surface (the same verbs the CLI and MCP servers use).
//!
//! The model is any OpenAI-compatible chat endpoint. Settings come from
//! `~/.config/officecraft/ai.json` (`base_url`, `model`); a key, if the endpoint wants one, is
//! read from `OFFICECRAFT_AI_KEY` and never written anywhere.

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

const MAX_STEPS: u8 = 8;
const MAX_RESULT_CHARS: usize = 12_000;
const DEFAULT_BASE: &str = "http://127.0.0.1:11434/v1";
// Small and quick while the suite is being tested. Override with `model` in ai.json or
// OFFICECRAFT_AI_MODEL (qwen3-coder:30b is far better at multi-step edits).
const DEFAULT_MODEL: &str = "qwen3:0.6b";

#[derive(Clone)]
pub struct Config {
    pub base_url: String,
    pub model: String,
    key: Option<String>,
}

impl Config {
    pub fn load() -> Config {
        let file = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
            .map(|c| c.join("officecraft").join("ai.json"));
        let json: Value = file.and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
        let get = |k: &str, d: &str| json.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(d).to_string();
        Config {
            base_url: std::env::var("OFFICECRAFT_AI_URL").unwrap_or_else(|_| get("base_url", DEFAULT_BASE)).trim_end_matches('/').to_string(),
            model: std::env::var("OFFICECRAFT_AI_MODEL").unwrap_or_else(|_| get("model", DEFAULT_MODEL)),
            key: std::env::var("OFFICECRAFT_AI_KEY").ok().filter(|k| !k.is_empty()),
        }
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
    Graph,
}

impl App {
    fn noun(self) -> &'static str {
        match self {
            App::Docs => "document",
            App::Sheets => "workbook",
            App::Slides => "presentation",
            App::Pdf => "PDF",
            App::Graph => "graph",
        }
    }
}

enum Line {
    User(String),
    Assistant(String),
    Tool(String),
    Error(String),
}

struct PendingCall {
    id: String,
    name: String,
    rx: Receiver<Result<Value, String>>,
}

pub struct Chat {
    pub open: bool,
    history: Vec<Value>,
    lines: Vec<Line>,
    input: String,
    reply: Option<Receiver<Result<Value, String>>>,
    pending: Vec<PendingCall>,
    /// Tool results gathered for the model's last turn, in call order.
    results: Vec<Value>,
    steps: u8,
}

impl Chat {
    pub fn new() -> Chat {
        Chat { open: false, history: Vec::new(), lines: Vec::new(), input: String::new(), reply: None, pending: Vec::new(), results: Vec::new(), steps: 0 }
    }

    /// Sends `text` as if it had been typed and submitted.
    pub fn ask_text(&mut self, text: String, app: App, cfg: &Config, ctx: &egui::Context) {
        self.input = text;
        self.send(app, cfg, ctx);
    }

    pub fn has_error(&self) -> bool {
        self.lines.iter().any(|l| matches!(l, Line::Error(_)))
    }

    /// The conversation as plain text.
    pub fn transcript(&self) -> String {
        let mut out = String::new();
        for l in &self.lines {
            let (tag, t) = match l {
                Line::User(t) => ("USER", t),
                Line::Assistant(t) => ("ASSISTANT", t),
                Line::Tool(t) => ("  tool", t),
                Line::Error(t) => ("ERROR", t),
            };
            out.push_str(&format!("{tag}: {t}\n"));
        }
        out
    }

    pub fn busy(&self) -> bool {
        self.reply.is_some() || !self.pending.is_empty()
    }

    pub fn clear(&mut self) {
        *self = Chat { open: self.open, ..Chat::new() };
    }

    /// Draws the panel. Returns true when the user submitted a message (call `send`).
    pub fn ui(&mut self, ui: &mut egui::Ui, app: App, cfg: &Config) -> bool {
        let mut submit = false;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.strong("Assistant");
            ui.weak(&cfg.model);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Clear").clicked() && !self.busy() {
                    self.clear();
                }
            });
        });
        ui.separator();
        let input_h = 92.0;
        let avail = ui.available_height() - input_h;
        egui::ScrollArea::vertical().max_height(avail.max(60.0)).auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
            if self.lines.is_empty() {
                ui.weak(format!("Ask me to read or change this {}. I work on the open file through its own commands.", app.noun()));
            }
            for l in &self.lines {
                match l {
                    Line::User(t) => {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(t).strong());
                    }
                    Line::Assistant(t) => {
                        ui.add_space(4.0);
                        ui.label(t);
                    }
                    Line::Tool(t) => {
                        ui.weak(format!("→ {t}"));
                    }
                    Line::Error(t) => {
                        ui.colored_label(egui::Color32::LIGHT_RED, t);
                    }
                }
            }
            if self.busy() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("working…");
                });
            }
        });
        ui.separator();
        let edit = ui.add_sized(
            [ui.available_width(), 56.0],
            egui::TextEdit::multiline(&mut self.input).hint_text("Ask about this file…  (Enter to send, Shift+Enter for a new line)").desired_rows(3),
        );
        let enter = edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
        let clicked = ui.add_enabled(!self.busy(), egui::Button::new("Send")).clicked();
        if enter {
            // The Enter that sent the message also typed a newline; drop it.
            while self.input.ends_with('\n') {
                self.input.pop();
            }
        }
        if (enter || clicked) && !self.busy() && !self.input.trim().is_empty() {
            submit = true;
        }
        submit
    }

    /// Sends the typed message and starts the model.
    pub fn send(&mut self, app: App, cfg: &Config, ctx: &egui::Context) {
        let text = std::mem::take(&mut self.input).trim().to_string();
        if text.is_empty() {
            return;
        }
        self.lines.push(Line::User(text.clone()));
        self.history.push(json!({"role": "user", "content": text}));
        self.steps = 0;
        self.ask(app, cfg, ctx);
    }

    fn ask(&mut self, app: App, cfg: &Config, ctx: &egui::Context) {
        let mut messages = vec![json!({"role": "system", "content": system_prompt(app)})];
        messages.extend(self.history.iter().cloned());
        let (tx, rx) = channel();
        self.reply = Some(rx);
        let tool_list = tools(app);
        let cfg = cfg.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = request(&cfg, messages, tool_list);
            let _ = tx.send(r);
            ctx.request_repaint();
        });
    }

    /// Advances the conversation: reads the model's reply, runs its tool calls on `host`, and
    /// asks again until it answers in words.
    pub fn poll(&mut self, host: &mut dyn Host, app: App, cfg: &Config, ctx: &egui::Context) {
        if self.reply.is_some() || !self.pending.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(120));
        }
        if let Some(rx) = &self.reply
            && let Ok(r) = rx.try_recv()
        {
            self.reply = None;
            match r {
                Err(e) => self.lines.push(Line::Error(e)),
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
                self.lines.push(Line::Error("The model returned an empty reply.".into()));
            } else {
                self.lines.push(Line::Assistant(text));
            }
            return;
        }
        if !text.is_empty() {
            self.lines.push(Line::Assistant(text));
        }
        self.steps += 1;
        if self.steps > MAX_STEPS {
            self.lines.push(Line::Error("Stopped: too many tool steps for one request.".into()));
            // Keep the transcript valid for the next message: every call gets an answer.
            for c in &calls {
                let id = c.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                self.history.push(json!({"role": "tool", "tool_call_id": id, "content": "stopped"}));
            }
            return;
        }
        self.results.clear();
        for c in calls {
            let id = c.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            let f = c.get("function").cloned().unwrap_or(Value::Null);
            let name = f.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let args = match f.get("arguments") {
                Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
                Some(v) => v.clone(),
                None => Value::Null,
            };
            match run_tool(host, app, &name, &args) {
                CallResult::Done(r) => self.finish_call(&id, &name, r),
                CallResult::Pending(rx) => self.pending.push(PendingCall { id, name, rx }),
            }
        }
        self.continue_if_ready(app, cfg, ctx);
    }

    fn finish_call(&mut self, id: &str, name: &str, r: Result<Value, String>) {
        let (summary, content) = match r {
            Ok(v) => (format!("{name}: ok"), truncate(&v)),
            Err(e) => (format!("{name}: {e}"), json!({"error": e}).to_string()),
        };
        self.lines.push(Line::Tool(summary));
        self.results.push(json!({"role": "tool", "tool_call_id": id, "content": content}));
    }

    fn continue_if_ready(&mut self, app: App, cfg: &Config, ctx: &egui::Context) {
        if self.pending.is_empty() && !self.results.is_empty() && self.reply.is_none() {
            self.history.append(&mut self.results);
            self.ask(app, cfg, ctx);
        }
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
    let extra = match app {
        App::Sheets => " Use read_range to read cells; engine commands change them.",
        App::Pdf => " This is a PDF editor; inspect_document reports the open file and view state.",
        App::Graph => " This is a graphing calculator: items are expressions such as y=x^2, points like (1,2), inequalities and r=... polar curves; a free letter becomes a slider. Commands are the engine's JSON commands (setExpr, addItem, setSlider, setView, ...).",
        _ => "",
    };
    format!(
        "You are the assistant built into OfficeCraft, working inside the user's open {kind}. \
         You act on the live {kind} only through your tools. Look before you change anything: call \
         inspect_document first, find commands with list_commands, run them with run_command. \
         Use the smallest change that does what was asked, and never remove content unless asked. \
         Do not invent command ids or parameters; if a call returns an error, read it and correct the call. \
         When done, say what you changed in one or two plain sentences.{extra}"
    )
}

fn tools(app: App) -> Value {
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

/// One chat-completions round trip. Returns the assistant message.
fn request(cfg: &Config, messages: Vec<Value>, tools: Value) -> Result<Value, String> {
    let url = format!("{}/chat/completions", cfg.base_url);
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(240))).http_status_as_error(false).build().into();
    let mut req = agent.post(&url);
    if let Some(k) = &cfg.key {
        req = req.header("Authorization", &format!("Bearer {k}"));
    }
    let body = json!({"model": cfg.model, "messages": messages, "tools": tools, "stream": false});
    let mut resp = req.header("Content-Type", "application/json").send(body.to_string()).map_err(|e| format!("Can't reach {url}: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().map_err(|e| format!("Bad reply from {url}: {e}"))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{url} did not answer with JSON ({e}): {}", text.chars().take(160).collect::<String>()))?;
    if !(200..300).contains(&status) {
        let msg = v.pointer("/error/message").and_then(Value::as_str).or_else(|| v.get("error").and_then(Value::as_str)).unwrap_or("request failed");
        return Err(format!("{url} answered {status}: {msg}"));
    }
    v.pointer("/choices/0/message").cloned().ok_or_else(|| "The reply had no message.".to_string())
}
