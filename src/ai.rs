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

/// Services the custom endpoint fills in by name: (name, OpenAI-compatible base URL). The model
/// list comes from the service itself (Test connection).
const CUSTOM_PRESETS: &[(&str, &str)] = &[
    ("Ollama on this computer", "http://127.0.0.1:11434/v1"),
    ("LM Studio on this computer", "http://127.0.0.1:1234/v1"),
    ("DeepSeek", "https://api.deepseek.com/v1"),
    ("Groq", "https://api.groq.com/openai/v1"),
    ("Together AI", "https://api.together.xyz/v1"),
    ("Mistral", "https://api.mistral.ai/v1"),
    ("Fireworks", "https://api.fireworks.ai/inference/v1"),
    ("Cerebras", "https://api.cerebras.ai/v1"),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Provider {
    Local,
    Cloud,
    OpenRouter,
    /// The user's own Claude Code (their Claude plan), on this computer or another one over SSH,
    /// working in the open documents through OfficeCraft's MCP server (desktop only).
    ClaudeCode,
}

impl Provider {
    pub const ALL: [Provider; 4] = [Provider::Local, Provider::Cloud, Provider::OpenRouter, Provider::ClaudeCode];
    /// The providers Settings offers here: a browser page cannot start Claude Code.
    pub fn offered() -> impl Iterator<Item = Provider> {
        Provider::ALL.into_iter().filter(|p| *p != Provider::ClaudeCode || cfg!(not(target_arch = "wasm32")))
    }
    fn index(self) -> usize {
        self as usize
    }
    fn id(self) -> &'static str {
        match self {
            Provider::Local => "local",
            Provider::Cloud => "cloud",
            Provider::OpenRouter => "openrouter",
            Provider::ClaudeCode => "claude_code",
        }
    }
    fn from_id(s: &str) -> Option<Provider> {
        Provider::ALL.into_iter().find(|p| p.id() == s)
    }
    pub fn title(self) -> &'static str {
        match self {
            Provider::Local => "Custom endpoint",
            Provider::Cloud => "Ollama Cloud",
            Provider::OpenRouter => "OpenRouter",
            Provider::ClaudeCode => "Claude Code",
        }
    }
    fn default_base(self) -> &'static str {
        match self {
            Provider::Local => DEFAULT_BASE,
            Provider::Cloud => "https://ollama.com/v1",
            Provider::OpenRouter => "https://openrouter.ai/api/v1",
            Provider::ClaudeCode => "",
        }
    }
    fn default_model(self) -> &'static str {
        match self {
            Provider::Local => DEFAULT_MODEL,
            Provider::Cloud => "gemma4:31b",
            Provider::OpenRouter => "openai/gpt-oss-120b",
            // Blank: whatever the user's Claude Code uses by default.
            Provider::ClaudeCode => "",
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
    models: [String; 4],
    /// API keys (`keys[provider.index()]`): the custom endpoint's (optional: a local server needs
    /// none), Ollama Cloud's and OpenRouter's. Claude Code needs none (its own login).
    keys: [String; 4],
    /// How to start Claude Code: `claude`, or `ssh <host> claude` for another computer (the
    /// tunnel back to OfficeCraft is added to the ssh command).
    pub claude_cmd: String,
    /// Keep the MCP server running for Claude Code sessions the user starts themselves.
    pub mcp_listen: bool,
}

/// What `claude_cmd` is when Settings leaves it blank.
pub const DEFAULT_CLAUDE_CMD: &str = "claude";

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
            models: [get(&json, "model"), get(&json, "cloud_model"), get(&json, "openrouter_model"), get(&json, "claude_model")],
            keys: [get(&keys_json, "custom"), get(&keys_json, "cloud"), get(&keys_json, "openrouter"), String::new()],
            claude_cmd: get(&json, "claude_command"),
            mcp_listen: json.get("mcp_listen").and_then(Value::as_bool).unwrap_or(false),
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
        self.key = env("OFFICECRAFT_AI_KEY")
            .or_else(|| (!key.is_empty()).then(|| key.clone()))
            // Ollama Cloud: the standard variable its own tools read, so a key set up there just works.
            .or_else(|| if p == Provider::Cloud { env("OLLAMA_API_KEY") } else { None });
    }

    /// The command that starts Claude Code, as Settings has it or the default.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub fn claude_command(&self) -> String {
        if self.claude_cmd.trim().is_empty() { DEFAULT_CLAUDE_CMD.to_string() } else { self.claude_cmd.trim().to_string() }
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
            "claude_model": self.models[3],
            "claude_command": self.claude_cmd,
            "mcp_listen": self.mcp_listen,
        });
        platform::write("ai.json", &serde_json::to_string_pretty(&settings).unwrap_or_default())?;
        let keys = json!({"custom": self.keys[0], "cloud": self.keys[1], "openrouter": self.keys[2]});
        platform::write_private("ai-keys.json", &serde_json::to_string_pretty(&keys).unwrap_or_default())
    }
}

/// The Settings window's working copy; `ui` returns the config when the user saves.
pub struct Settings {
    draft: Config,
    reveal: bool,
    saved_note: String,
    test: std::sync::Arc<std::sync::Mutex<Option<Result<usize, String>>>>,
    /// The models the endpoint lists and what pinging each one found.
    probe: std::sync::Arc<std::sync::Mutex<Probe>>,
    /// OpenRouter: list only the `:free` models (what a free account can use).
    free_only: bool,
    /// The host the custom endpoint's key was entered for: the key is cleared when the endpoint
    /// moves to another host, so it never goes somewhere it wasn't meant for.
    key_host: String,
    /// The saved MCP token, read once when the Claude Code section is first shown.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    mcp_token: Option<String>,
}

/// The host (and port) of an endpoint URL, lowercased: `https://API.x.com:8443/v1` → `api.x.com:8443`.
fn url_host(url: &str) -> String {
    let rest = url.trim().split_once("://").map(|(_, r)| r).unwrap_or(url.trim());
    rest.split(['/', '?', '#']).next().unwrap_or("").to_ascii_lowercase()
}

/// Plain http to another machine: the key and documents cross the network unencrypted.
fn insecure_remote(url: &str) -> bool {
    let host = url_host(url);
    let name = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(&host);
    url.trim().to_ascii_lowercase().starts_with("http://") && !matches!(name, "localhost" | "127.0.0.1" | "[::1]")
}

/// What the Settings window knows about the endpoint's models.
#[derive(Default)]
struct Probe {
    ids: Vec<String>,
    /// Per model: `Ok(seconds)` when a one-word chat completion came back, else why not.
    results: std::collections::HashMap<String, Result<f32, String>>,
    testing: std::collections::HashSet<String>,
    all_running: bool,
    stop: bool,
}

impl Settings {
    pub fn new(cfg: &Config) -> Settings {
        Settings { draft: cfg.clone(), reveal: false, saved_note: String::new(), test: Default::default(), probe: Default::default(), free_only: true, key_host: url_host(&cfg.local_url), mcp_token: None }
    }

    /// `Some(Some(cfg))`: saved; `Some(None)`: closed without saving; `None`: still open.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Option<Config>> {
        let mut result = None;
        let mut open = true;
        egui::Window::new("Assistant settings").open(&mut open).collapsible(false).resizable(false).default_width(500.0).show(ctx, |ui| {
            ui.weak(format!("This build: {}", crate::platform::build_label()));
            ui.weak(format!("Apps: {}", crate::platform::app_versions()));
            let d = &mut self.draft;
            ui.label(egui::RichText::new("Provider").strong());
            ui.horizontal(|ui| {
                for p in Provider::offered() {
                    if ui.selectable_label(d.provider == p, p.title()).clicked() {
                        d.provider = p;
                        // A test still running belongs to the other provider: stop it, and let
                        // its answers land where no one reads them.
                        self.probe.lock().unwrap().stop = true;
                        self.probe = Default::default();
                        self.test = Default::default();
                    }
                }
            });
            ui.add_space(6.0);
            let p = d.provider;
            egui::Grid::new("ai_settings_grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                match p {
                    Provider::Local => {
                        // Fill the endpoint for a known service; any OpenAI-compatible URL works.
                        ui.label("Service");
                        let shown = if d.local_url.trim().is_empty() { DEFAULT_BASE } else { d.local_url.trim_end_matches('/') };
                        let current = CUSTOM_PRESETS.iter().find(|(_, url)| shown == *url).map(|(name, _)| *name).unwrap_or("Other");
                        egui::ComboBox::from_id_salt("ai_custom_preset").selected_text(current).width(300.0).show_ui(ui, |ui| {
                            for (name, url) in CUSTOM_PRESETS {
                                if ui.selectable_label(current == *name, *name).clicked() {
                                    d.local_url = url.to_string();
                                    // The new service has its own models (pick from its list) and its own key:
                                    // never send one service's key to another.
                                    d.models[0].clear();
                                    d.keys[0].clear();
                                    self.key_host = url_host(url);
                                    // A test still running belongs to the old service (and its key):
                                    // stop it and let its answers land somewhere no one reads.
                                    self.probe.lock().unwrap().stop = true;
                                    self.probe = Default::default();
                                    self.test = Default::default();
                                }
                            }
                        });
                        ui.end_row();
                        ui.label("Endpoint");
                        ui.add(egui::TextEdit::singleline(&mut d.local_url).hint_text(DEFAULT_BASE).desired_width(300.0));
                        ui.end_row();
                        let host = url_host(&d.local_url);
                        if host != self.key_host {
                            if !d.keys[0].is_empty() {
                                d.keys[0].clear();
                                self.saved_note = "The endpoint moved to another host, so its API key was cleared.".into();
                            }
                            // Another server has other models: pick from its list.
                            d.models[0].clear();
                            self.probe.lock().unwrap().stop = true;
                            self.probe = Default::default();
                            self.test = Default::default();
                            self.key_host = host;
                        }
                        if insecure_remote(&d.local_url) {
                            ui.label("");
                            ui.colored_label(egui::Color32::from_rgb(180, 110, 0), "Plain http to another machine: the key and your documents travel unencrypted.");
                            ui.end_row();
                        }
                        ui.label("API key");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut d.keys[0]).password(!self.reveal).hint_text("none for a local server").desired_width(260.0));
                            ui.checkbox(&mut self.reveal, "Show");
                        });
                        ui.end_row();
                    }
                    Provider::ClaudeCode => {
                        ui.label("Runs on");
                        ui.horizontal(|ui| {
                            let local = d.claude_cmd.trim().is_empty() || d.claude_cmd.trim() == DEFAULT_CLAUDE_CMD;
                            if ui.selectable_label(local, "This computer").clicked() {
                                d.claude_cmd = DEFAULT_CLAUDE_CMD.into();
                            }
                            if ui.selectable_label(!local, "Another computer (SSH)").clicked() && local {
                                d.claude_cmd = "ssh my-nas claude".into();
                            }
                        });
                        ui.end_row();
                        ui.label("Command");
                        ui.add(egui::TextEdit::singleline(&mut d.claude_cmd).hint_text(DEFAULT_CLAUDE_CMD).desired_width(300.0).font(egui::TextStyle::Monospace));
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
                let hint = if p == Provider::ClaudeCode { "default (or sonnet, opus, haiku)" } else { p.default_model() };
                ui.add(egui::TextEdit::singleline(&mut d.models[p.index()]).hint_text(hint).desired_width(300.0));
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(match p {
                    Provider::Local => "Any OpenAI-compatible API: a local server (Ollama, LM Studio, llama.cpp, vLLM), your own gateway, or a pay-as-you-go service such as DeepSeek or Groq. A hosted service gets your prompts and documents.",
                    Provider::Cloud => "Large hosted models. Your prompts and documents are sent to ollama.com.",
                    Provider::OpenRouter => "Many hosted models. Your prompts and documents are sent to openrouter.ai and on to the lab running the model.",
                    Provider::ClaudeCode => "Your own Claude Code and Claude plan: it edits the open documents live, step by step, with OfficeCraft's tools only (no shell). It can open files and save to a path, but never replaces an existing file unless asked. For another computer, write the ssh command that reaches it, e.g. ssh my-nas claude; OfficeCraft adds the tunnel back. Sign in to Claude Code there first.",
                })
                .size(12.0)
                .weak(),
            );
            #[cfg(not(target_arch = "wasm32"))]
            if p == Provider::ClaudeCode {
                self.mcp_section(ui);
            }
            let d = &mut self.draft;
            if (p != Provider::Local && p != Provider::ClaudeCode) || (p == Provider::Local && !d.keys[0].is_empty()) {
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
                let test_clicked = ui.button("Test connection").clicked();
                if test_clicked && self.draft.provider == Provider::ClaudeCode {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let cmd = self.draft.claude_command();
                        let slot = self.test.clone();
                        *slot.lock().unwrap() = None;
                        let ctx = ui.ctx().clone();
                        std::thread::spawn(move || {
                            let r = claude_version(&cmd);
                            *slot.lock().unwrap() = Some(r.map(|v| {
                                *CLAUDE_VERSION.lock().unwrap_or_else(|e| e.into_inner()) = v;
                                usize::MAX
                            }));
                            ctx.request_repaint();
                        });
                    }
                } else if test_clicked {
                    let mut probe = self.draft.clone();
                    probe.apply();
                    *self.test.lock().unwrap() = None;
                    let slot = self.test.clone();
                    let models = self.probe.clone();
                    *models.lock().unwrap() = Probe::default();
                    let ctx = ui.ctx().clone();
                    spawn_task(async move {
                        let r = fetch_models(&probe, 8).await;
                        if let Ok(ids) = &r {
                            models.lock().unwrap().ids = ids.clone();
                        }
                        *slot.lock().unwrap() = Some(r.map(|v| v.len()));
                        ctx.request_repaint();
                    });
                }
                match &*self.test.lock().unwrap() {
                    Some(Ok(usize::MAX)) => {
                        ui.colored_label(egui::Color32::from_rgb(46, 125, 79), format!("Found {}", CLAUDE_VERSION.lock().unwrap_or_else(|e| e.into_inner())));
                    }
                    Some(Ok(n)) => {
                        ui.colored_label(egui::Color32::from_rgb(46, 125, 79), format!("Connected: {n} model{}", if *n == 1 { "" } else { "s" }));
                    }
                    Some(Err(e)) => {
                        ui.colored_label(egui::Color32::from_rgb(194, 74, 47), e);
                    }
                    None => {}
                }
            });
            if self.draft.provider != Provider::ClaudeCode {
                self.model_list(ui);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                // A hosted service has no default model here: guessing one would fail at the
                // first message.
                let needs_model = self.draft.provider == Provider::Local
                    && self.draft.models[0].trim().is_empty()
                    && !self.draft.local_url.trim().is_empty()
                    && self.draft.local_url.trim_end_matches('/') != DEFAULT_BASE;
                let save = ui.button(egui::RichText::new("Save").strong()).clicked();
                if save && needs_model {
                    self.saved_note = "Choose a model first: Test connection lists this service's models.".into();
                } else if save {
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

impl Settings {
    /// The endpoint's models with a test ping each, so a free account can see which ones answer.
    fn model_list(&mut self, ui: &mut egui::Ui) {
        let (provider, chosen) = (self.draft.provider, self.draft.models[self.draft.provider.index()].clone());
        let (listed, all_running) = {
            let g = self.probe.lock().unwrap();
            (g.ids.len(), g.all_running)
        };
        if listed == 0 {
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Press Test connection to list the models, then test which ones your account can use.").size(12.0).weak());
            return;
        }
        ui.add_space(6.0);
        ui.separator();
        let free_only = provider == Provider::OpenRouter && self.free_only;
        let visible: Vec<String> = {
            let g = self.probe.lock().unwrap();
            g.ids.iter().filter(|id| !free_only || id.ends_with(":free")).cloned().collect()
        };
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("Models ({})", visible.len())).strong());
            if provider == Provider::OpenRouter {
                ui.checkbox(&mut self.free_only, "Free only");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if all_running {
                    if ui.button("Stop").clicked() {
                        self.probe.lock().unwrap().stop = true;
                    }
                } else if ui.button("Test all").on_hover_text("Ping each listed model once (the first 30), one after another").clicked() {
                    let mut cfg = self.draft.clone();
                    cfg.apply();
                    let probe = self.probe.clone();
                    {
                        let mut g = probe.lock().unwrap();
                        g.stop = false;
                        g.all_running = true;
                    }
                    let ctx = ui.ctx().clone();
                    let ids: Vec<String> = visible.iter().take(30).cloned().collect();
                    spawn_task(async move {
                        for id in ids {
                            {
                                let mut g = probe.lock().unwrap();
                                if g.stop {
                                    break;
                                }
                                g.testing.insert(id.clone());
                            }
                            ctx.request_repaint();
                            let r = ping_model(&cfg, &id).await;
                            let mut g = probe.lock().unwrap();
                            g.testing.remove(&id);
                            g.results.insert(id, r);
                            drop(g);
                            ctx.request_repaint();
                        }
                        probe.lock().unwrap().all_running = false;
                        ctx.request_repaint();
                    });
                }
            });
        });
        let green = egui::Color32::from_rgb(46, 125, 79);
        let red = egui::Color32::from_rgb(194, 74, 47);
        let mut pick: Option<String> = None;
        let mut ping: Option<String> = None;
        egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
            let g = self.probe.lock().unwrap();
            for id in &visible {
                ui.horizontal(|ui| {
                    if ui.selectable_label(*id == chosen, id).on_hover_text("Use this model").clicked() {
                        pick = Some(id.clone());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let busy = g.testing.contains(id);
                        if ui.add_enabled(!busy, egui::Button::new("Test").small()).clicked() {
                            ping = Some(id.clone());
                        }
                        if busy {
                            ui.spinner();
                        }
                        match g.results.get(id) {
                            Some(Ok(secs)) => {
                                ui.colored_label(green, format!("✓ available · {secs:.1}s"));
                            }
                            Some(Err(e)) => {
                                let short: String = if e.chars().count() > 34 { e.chars().take(33).collect::<String>() + "…" } else { e.clone() };
                                ui.colored_label(red, format!("✗ {short}")).on_hover_text(e);
                            }
                            None => {}
                        }
                    });
                });
            }
        });
        if let Some(id) = pick {
            self.draft.models[provider.index()] = id;
        }
        if let Some(id) = ping {
            let mut cfg = self.draft.clone();
            cfg.apply();
            let probe = self.probe.clone();
            probe.lock().unwrap().testing.insert(id.clone());
            let ctx = ui.ctx().clone();
            spawn_task(async move {
                let r = ping_model(&cfg, &id).await;
                let mut g = probe.lock().unwrap();
                g.testing.remove(&id);
                g.results.insert(id, r);
                drop(g);
                ctx.request_repaint();
            });
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
    /// The panel's settings icon was pressed.
    OpenSettings,
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
    /// Presentations the model asked for (`create_presentation`), for the shell to open in a Slides tab.
    decks: Vec<Value>,
    /// Graphs the model asked for (`create_graph`): `title`, `expressions`, `mode`.
    graphs: Vec<Value>,
    /// The tab this chat belongs to (Claude Code is told which document "this" is).
    pub tab: u64,
    /// A Claude Code turn in progress.
    claude: Option<ClaudeRun>,
    /// Claude Code's session, so the next message continues the same conversation.
    claude_session: Option<String>,
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
            decks: Vec::new(),
            graphs: Vec::new(),
            tab: 0,
            claude: None,
            claude_session: None,
        }
    }

    /// Presentations the assistant asked for since the last call: each is a `build_slides` argument
    /// object (`title`, `slides`).
    pub fn take_decks(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.decks)
    }

    /// Graphs the assistant asked for since the last call.
    pub fn take_graphs(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.graphs)
    }

    /// Adds a plain line to the conversation (the shell reports what it did with a request).
    pub fn note(&mut self, text: String) {
        self.items.push(Item::Note(text));
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
        self.reply.is_some() || !self.pending.is_empty() || self.claude.is_some()
    }

    pub fn clear(&mut self) {
        let (open, tab) = (self.open, self.tab);
        self.end_claude();
        *self = Chat { open, tab, ..Chat::new() };
    }

    /// Stops a Claude Code turn (if any) and retires its token.
    fn end_claude(&mut self) {
        // Dropping the run stops its process and revokes its token.
        self.claude = None;
    }

    /// Stops the current request: whatever arrives for it later is dropped.
    fn stop(&mut self) {
        self.epoch += 1;
        self.end_claude();
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
                if ui.add(egui::Button::new(egui::RichText::new("⚙").size(14.0).color(muted)).frame(false)).on_hover_text("Assistant settings: provider, endpoint, API keys, test models").clicked() {
                    action = Some(Action::OpenSettings);
                }
                let can_new = !self.items.is_empty() && !busy;
                let r = ui.add_enabled(can_new, egui::Button::new(egui::RichText::new("New chat").size(12.0)).small()).on_hover_text("Start a new conversation");
                if r.clicked() {
                    self.clear();
                }
                // The model, as a quiet picker.
                let shown = if cfg.model.is_empty() && cfg.provider == Provider::ClaudeCode { "Claude Code".to_string() } else { short_model(&cfg.model) };
                let label = egui::RichText::new(shown).size(12.0).color(muted);
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
        if cfg.provider == Provider::ClaudeCode {
            self.ask_claude(&text, app, cfg, ctx);
            return;
        }
        self.ask(app, cfg, ctx);
    }

    /// One Claude Code turn, continuing its session when there is one.
    fn ask_claude(&mut self, text: &str, app: App, cfg: &Config, ctx: &egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        match start_claude(cfg, text, &claude_system(app, self.tab), self.claude_session.as_deref(), ctx) {
            Ok(run) => self.claude = Some(run),
            Err(e) => {
                self.items.push(Item::Error(e));
                self.started = None;
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (text, app, cfg, ctx);
            self.items.push(Item::Error("Claude Code works in the desktop app.".into()));
            self.started = None;
        }
    }

    /// Shows what a Claude Code turn has done since the last frame.
    pub fn poll_claude(&mut self, ctx: &egui::Context) {
        let Some(run) = &self.claude else { return };
        ctx.request_repaint_after(Duration::from_millis(100));
        let mut evs = Vec::new();
        while let Ok(ev) = run.rx.try_recv() {
            evs.push(ev);
        }
        for ev in evs {
            match ev {
                ClaudeEv::Text(t) => self.items.push(Item::Assistant(t)),
                ClaudeEv::Session(id) => self.claude_session = Some(id),
                ClaudeEv::Tool { id, label, edit } => {
                    let step = Step { id, label, state: StepState::Running, edit };
                    match self.items.last_mut() {
                        Some(Item::Steps(s)) => s.push(step),
                        _ => self.items.push(Item::Steps(vec![step])),
                    }
                }
                ClaudeEv::ToolDone { id, ok } => {
                    for it in self.items.iter_mut().rev() {
                        if let Item::Steps(s) = it
                            && let Some(st) = s.iter_mut().find(|st| st.id == id)
                        {
                            st.state = if ok { StepState::Done } else { StepState::Failed };
                            break;
                        }
                    }
                }
                ClaudeEv::Done(r) => {
                    if let Err(e) = r {
                        // Claude Code also says the error as its answer (a usage limit, say):
                        // show it once, as the error.
                        if matches!(self.items.last(), Some(Item::Assistant(t)) if t.trim() == e.trim()) {
                            self.items.pop();
                        }
                        self.items.push(Item::Error(e));
                    }
                    for it in &mut self.items {
                        if let Item::Steps(s) = it {
                            for st in s.iter_mut().filter(|st| st.state == StepState::Running) {
                                st.state = StepState::Failed;
                            }
                        }
                    }
                    self.took = self.started.map(|t| t.elapsed().as_secs_f32());
                    self.started = None;
                    self.end_claude();
                    return;
                }
            }
        }
    }

    fn ask(&mut self, app: App, cfg: &Config, ctx: &egui::Context) {
        let mut sys = system_prompt(app);
        if web_enabled(cfg) {
            sys.push_str(" You also have web_search and web_fetch: use them only when the user asks for current or outside facts, always with a real query or URL, and name your sources.");
        }
        let mut messages = vec![json!({"role": "system", "content": sys})];
        messages.extend(self.history.iter().cloned());
        let (tx, rx) = channel();
        self.reply = Some(rx);
        self.streaming.clear();
        let mut tool_list = tools(app);
        if web_enabled(cfg) && let Value::Array(a) = &mut tool_list {
            a.extend(web_tools());
        }
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
        self.poll_claude(ctx);
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
            if app == App::Docs && name == "create_graph" {
                // Drawn with viSHual and placed as a picture by the shell, which owns the tabs.
                let ok = args.get("expressions").and_then(Value::as_array).is_some_and(|a| a.iter().any(|e| e.as_str().is_some_and(|t| !t.trim().is_empty())));
                let r = if ok {
                    self.graphs.push(args.clone());
                    Ok(json!({"status": "drawn with viSHual and inserted as a picture at the cursor; double-click it to edit"}))
                } else {
                    Err("create_graph needs a non-empty `expressions` list".to_string())
                };
                self.finish_call(&id, &name, r);
                continue;
            }
            if app == App::Docs && name == "create_presentation" {
                // Needs a new tab, which only the shell can open: queue it and answer the model.
                let r = match args.get("slides").and_then(Value::as_array) {
                    Some(s) if !s.is_empty() => {
                        self.decks.push(args.clone());
                        Ok(json!({"status": "opened a new Slides tab with the deck", "slides": s.len()}))
                    }
                    _ => Err("create_presentation needs a non-empty `slides` list".to_string()),
                };
                self.finish_call(&id, &name, r);
                continue;
            }
            if (name == "web_search" || name == "web_fetch") && web_enabled(cfg) {
                let rx = web_call(cfg, &name, &args, ctx);
                self.pending.push(PendingCall { id, name, rx });
                continue;
            }
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
            Err(e) => {
                // The shell's own tools are not commands: calling one through batch/run_command fails.
                let shell_tool = ["create_graph", "create_presentation", "web_search", "web_fetch", "insert_blocks", "build_slides", "replace_text"]
                    .into_iter()
                    .find(|t| e.contains(&format!("`{t}`")));
                let hint = if let Some(t) = shell_tool {
                    format!(" `{t}` is a tool of its own: call it directly as a separate tool call (once per item), never inside batch, execute or run_command.")
                } else if e.contains("unknown command") {
                    " Command ids look like `format.bold` or `text.insert`, not a description: call list_commands with a filter, or use a dedicated tool (replace_text, insert_blocks, build_slides).".to_string()
                } else {
                    String::new()
                };
                (false, json!({"error": format!("{e}.{hint}")}).to_string())
            }
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
            ("Turn this into a presentation", true),
            ("Fix spelling and grammar", true),
            ("Make it more concise", true),
            ("Draft a one-page report on…", false),
        ],
        App::Slides => &[("Summarize this deck", true), ("Add a summary slide", true), ("Tighten the bullet points", true), ("Draft a 5-slide deck on…", false)],
        App::Sheets => &[("Explain what this sheet does", true), ("Add a totals row", true), ("Create a sample budget table", true), ("Chart the first column against…", false)],
        App::Pdf => &[("What is in this file?", true), ("How many pages does it have?", true)],
        App::Graph => &[("Plot y = sin(x)", true), ("Add a slider for a", true), ("Plot the unit circle", true)],
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
        "web_search" | "web_fetch" | "inspect_document" | "inspect_slide" | "inspect_workbook" | "get_text" | "read_range" | "get_cell" | "list_commands" | "list_functions"
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
        "web_search" => format!("Searched the web for “{}”", s("query")),
        "web_fetch" => format!("Read {}", s("url")),
        "create_graph" => format!("Drew a graph of {}", plural(count(args, "expressions"), "item", "items")),
        "create_presentation" => format!("Made a {} deck", plural(count(args, "slides"), "slide", "slides")),
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
              format.bold or para.style with execute. Use batch to run several commands in one go. \
              To turn the document into a presentation, read all of it first, then call create_presentation \
              once; it opens the deck in a new tab and leaves this document alone. To add a graph or plot \
              call create_graph (it is drawn with viSHual and placed as a picture at the cursor)."
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
        App::Graph => {
            " This is a graphing calculator: items are expressions such as y=x^2, points like (1,2), \
              inequalities and r=... polar curves; a free letter becomes a slider. Look with inspect_graph; \
              add_expressions adds several items, set_mode switches 1D/2D/3D and run_command runs any engine \
              command (see list_commands). start_graph clears the workspace and file_in_gallery files it as a card."
        }
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
        App::Graph => vishual_mcp::tool_definitions(),
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
            "name": "create_presentation",
            "description": "Make a new slide deck from what you have read, in its own new Slides tab (this \
                            document stays open and unchanged). Call it once, after reading the document, \
                            with every slide: a title slide first, then one slide per main point with short \
                            bullets (about 3 to 5, under 12 words each).",
            "parameters": {"type": "object", "properties": {
                "title": {"type": "string", "description": "the deck's title"},
                "slides": {"type": "array", "description": "in order", "items": {"type": "object", "properties": {
                    "title": {"type": "string"},
                    "bullets": {"type": "array", "items": {"type": "string"}},
                    "layout": {"type": "string", "enum": ["title", "titleAndContent", "sectionHeader", "twoContent", "titleOnly", "blank"]}},
                    "required": ["title"]}}},
                "required": ["slides"]}}}));
        out.push(json!({"type": "function", "function": {
            "name": "create_graph",
            "description": "Draw ONE graph with viSHual and insert it into the document as a picture at the \
                            cursor; call this tool once per graph, directly (never inside batch or execute), \
                            when separate graphs are wanted. Give each item as text: y=x^2, y=\\sin(x), a point (1,2), r=1+cos(theta), \
                            x^2+y^2<4. A free letter such as a becomes a slider. Double-clicking the picture \
                            later opens it for editing.",
            "parameters": {"type": "object", "properties": {
                "title": {"type": "string", "description": "short name, used as the picture's description"},
                "expressions": {"type": "array", "items": {"type": "string"}},
                "mode": {"type": "string", "enum": ["2d", "3d"], "description": "default 2d"}},
                "required": ["expressions"]}}}));
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
pub fn build_slides(host: &mut dyn Host, args: &Value) -> Result<Value, String> {
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
pub fn insert_blocks(host: &mut dyn Host, args: &Value) -> Result<Value, String> {
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

impl vishual_mcp::Backend for Bridge<'_> {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        Bridge::call(self, method, params)
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

/// The tools an outside agent (Claude Code over MCP) gets for a tab of `app`, in MCP's shape
/// (`name`, `description`, `inputSchema`): the assistant's own, less the ones only the chat
/// panel can carry out (a deck or graph opened beside the document; MCP has `send_to` for that).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub fn agent_tools(app: App) -> Vec<Value> {
    let Value::Array(defs) = tools(app) else { return Vec::new() };
    defs.into_iter()
        .filter_map(|d| {
            let f = d.get("function")?;
            let name = f.get("name")?.as_str()?;
            (!matches!(name, "create_presentation" | "create_graph")).then(|| json!({"name": name, "description": f.get("description"), "inputSchema": f.get("parameters")}))
        })
        .collect()
}

/// Runs one of [`agent_tools`] on `host`.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub fn agent_call(host: &mut dyn Host, app: App, name: &str, args: &Value) -> CallResult {
    if !agent_tools(app).iter().any(|d| d["name"] == name) {
        return CallResult::Done(Err(format!("{name} is not one of this app's tools (see app_tools)")));
    }
    run_tool(host, app, name, args)
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
            App::Graph => {
                let r = vishual_mcp::call_tool(&mut b, name, args);
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

/// Web search and page reading go through Ollama's hosted API, so they need the Ollama Cloud
/// provider and its key.
fn web_enabled(cfg: &Config) -> bool {
    cfg.key.is_some() && cfg.base_url.starts_with("https://ollama.com")
}

fn web_tools() -> Vec<Value> {
    vec![
        json!({"type": "function", "function": {
            "name": "web_search",
            "description": "Search the web. Returns titles, URLs and text excerpts. Use for current or outside facts.",
            "parameters": {"type": "object", "properties": {
                "query": {"type": "string"},
                "max_results": {"type": "integer", "description": "1 to 10 (default 5)"}},
                "required": ["query"]}}}),
        json!({"type": "function", "function": {
            "name": "web_fetch",
            "description": "Read the text of one web page by URL (for example one from web_search).",
            "parameters": {"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}}}),
    ]
}

/// Runs a web tool off the UI thread; the answer arrives on the returned channel.
fn web_call(cfg: &Config, name: &str, args: &Value, ctx: &egui::Context) -> Receiver<Result<Value, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let key = cfg.key.clone();
    let ctx = ctx.clone();
    let (url, body) = if name == "web_search" {
        let n = args.get("max_results").and_then(Value::as_u64).unwrap_or(5).clamp(1, 10);
        ("https://ollama.com/api/web_search", json!({"query": args.get("query").and_then(Value::as_str).unwrap_or(""), "max_results": n}))
    } else {
        ("https://ollama.com/api/web_fetch", json!({"url": args.get("url").and_then(Value::as_str).unwrap_or("")}))
    };
    spawn_task(async move {
        let r = match post_json(url, key.as_deref(), &body, 45).await {
            Ok((200..=299, text)) => serde_json::from_str::<Value>(&text).map_err(|e| format!("unreadable reply: {e}")),
            Ok((status, text)) => Err(format!("web request failed ({status}): {}", text.chars().take(200).collect::<String>())),
            Err(e) => Err(e),
        };
        let _ = tx.send(r);
        ctx.request_repaint();
    });
    rx
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

/// Why a ping failed, in words a free-tier user can act on.
fn explain_status(status: u16, body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let msg = v.pointer("/error/message").and_then(Value::as_str).or_else(|| v.get("error").and_then(Value::as_str)).unwrap_or("").trim();
    let why = match status {
        401 | 403 => "not available on this account",
        402 => "needs credits or a paid plan",
        404 => "model not found",
        429 => "rate limited, try again shortly",
        _ => "request failed",
    };
    if msg.is_empty() { format!("{why} (HTTP {status})") } else { format!("{why}: {msg} (HTTP {status})") }
}

/// One tiny, non-streamed chat completion: `Ok(seconds)` when the model answered.
async fn ping_model(cfg: &Config, model: &str) -> Result<f32, String> {
    let url = format!("{}/chat/completions", cfg.base_url);
    let body = json!({"model": model, "messages": [{"role": "user", "content": "Reply with the single word: ok"}], "max_tokens": 16, "stream": false});
    let t0 = Instant::now();
    let (status, text) = post_json(&url, cfg.key.as_deref(), &body, 45).await?;
    if !(200..300).contains(&status) {
        return Err(explain_status(status, &text));
    }
    // Some gateways answer 200 and put the refusal in the body.
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if let Some(m) = v.pointer("/error/message").and_then(Value::as_str) {
        return Err(m.to_string());
    }
    Ok(t0.elapsed().as_secs_f32())
}

#[cfg(not(target_arch = "wasm32"))]
async fn post_json(url: &str, key: Option<&str>, body: &Value, timeout_secs: u64) -> Result<(u16, String), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(timeout_secs))).http_status_as_error(false).build().into();
    let mut req = agent.post(url);
    if let Some(k) = key {
        req = req.header("Authorization", &format!("Bearer {k}"));
    }
    let mut resp = req.header("Content-Type", "application/json").send(body.to_string()).map_err(|e| format!("Can't reach {url}: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    Ok((status, text))
}

#[cfg(target_arch = "wasm32")]
async fn post_json(url: &str, key: Option<&str>, body: &Value, timeout_secs: u64) -> Result<(u16, String), String> {
    let resp = fetch(url, "POST", key, Some(body.to_string()), timeout_secs).await?;
    let status = resp.status();
    Ok((status, response_text(&resp).await.unwrap_or_default()))
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
    let body = json!({"model": cfg.model, "messages": messages, "tools": tools, "stream": true}).to_string();
    // A hosted endpoint now and then answers 429 or 5xx for no reason of the request's: nothing has
    // streamed yet, so asking again is safe.
    let mut attempt = 0;
    let mut resp = loop {
        let mut req = agent.post(&url);
        if let Some(k) = &cfg.key {
            req = req.header("Authorization", &format!("Bearer {k}"));
        }
        let mut resp = req.header("Content-Type", "application/json").send(body.clone()).map_err(|e| format!("Can't reach {url}: {e}"))?;
        let status = resp.status().as_u16();
        if (200..300).contains(&status) {
            break resp;
        }
        if (status == 429 || status >= 500) && attempt < 3 {
            attempt += 1;
            std::thread::sleep(Duration::from_millis(1500 * attempt as u64));
            continue;
        }
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        return Err(error_message(&url, status, &text));
    };
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
    if cfg.provider == Provider::ClaudeCode {
        // Claude Code's own aliases; its default when none is chosen.
        return ["sonnet", "opus", "haiku"].map(String::from).to_vec();
    }
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

// ---- Claude Code ----

/// What `claude --version` said the last time Settings tested it.
static CLAUDE_VERSION: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// A Claude Code run's events, as the chat shows them.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum ClaudeEv {
    Text(String),
    Tool { id: String, label: String, edit: bool },
    ToolDone { id: String, ok: bool },
    Session(String),
    /// The run ended: `Err` with what went wrong.
    Done(Result<(), String>),
}

/// A running `claude -p`: its events, and how to stop it.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
struct ClaudeRun {
    rx: Receiver<ClaudeEv>,
    #[cfg(not(target_arch = "wasm32"))]
    child: std::sync::Arc<std::sync::Mutex<Option<std::process::Child>>>,
    /// The MCP token this run was given, revoked when it ends.
    token: String,
    /// Over ssh: the run's stdin, held open until the run ends (its closing stops the remote side).
    #[cfg(not(target_arch = "wasm32"))]
    stdin: std::sync::Arc<std::sync::Mutex<Option<std::process::ChildStdin>>>,
}

impl Drop for ClaudeRun {
    /// However a run ends (Stop, its answer, its tab closed, the app quitting), its process is
    /// stopped and its token stops working.
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Closing stdin first lets the far end of an ssh run stop Claude Code there.
            drop(self.stdin.lock().unwrap_or_else(|e| e.into_inner()).take());
            let child = self.child.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(mut c) = child {
                let _ = c.kill();
                let _ = c.wait();
            }
            crate::mcp::revoke(&self.token);
        }
    }
}

/// `cmd` split into words the way a shell would for simple cases: spaces separate, and single
/// or double quotes keep a word together.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn split_words(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in cmd.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                any = true;
            }
            None if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            None => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `s` as one word for a POSIX shell (the remote end of ssh runs the command through one).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Put before the remote command over ssh: a non-interactive login often lacks the folders
/// Claude Code installs into (`~/.local/bin` for the native installer), and the remote shell
/// expands `$HOME` and `$PATH` itself.
const REMOTE_PATH: &str = "PATH=\"$HOME/.local/bin:$HOME/.claude/local:$HOME/.npm-global/bin:/usr/local/bin:$PATH\"";

/// How one Claude Code turn is started.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
struct Launch {
    program: String,
    args: Vec<String>,
    /// Over ssh: the token goes in as the first line on stdin, before the prompt.
    ssh: bool,
}

/// The program and arguments that run Claude Code for one turn. `cmd` is Settings' command
/// (`claude`, or `ssh host claude`). Locally Claude Code reaches OfficeCraft at `local_port`;
/// over ssh `-R remote_port:127.0.0.1:local_port` tunnels it back, and the remote side runs
/// under `sh -c` with Claude Code's install folders on its PATH. The token is never on a command
/// line: the MCP config names `${OFFICECRAFT_MCP_TOKEN}`, set in the environment locally and read
/// from stdin's first line over ssh. The prompt goes in on stdin.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn claude_argv(cmd: &str, local_port: u16, remote_port: u16, system: &str, model: &str, resume: Option<&str>) -> Result<Launch, String> {
    let words = split_words(cmd);
    let Some(first) = words.first() else { return Err("Settings has no command for Claude Code".into()) };
    let base = first.rsplit(['/', '\\']).next().unwrap_or(first);
    let ssh = base == "ssh" || base == "ssh.exe";
    if ssh && words.len() < 3 {
        return Err("write the ssh command in full, with the host and the command: ssh my-nas claude".into());
    }
    let port = if ssh { remote_port } else { local_port };
    let mut claude: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        // No shell, no file tools: only OfficeCraft's.
        "--tools".into(),
        "".into(),
        "--strict-mcp-config".into(),
        "--mcp-config".into(),
        mcp_config_text(port),
        "--allowedTools".into(),
        "mcp__officecraft".into(),
        "--append-system-prompt".into(),
        system.into(),
    ];
    if !model.trim().is_empty() {
        claude.push("--model".into());
        claude.push(model.trim().into());
    }
    if let Some(id) = resume {
        claude.push("--resume".into());
        claude.push(id.into());
    }
    if ssh {
        let program: Vec<String> = words[2..].to_vec();
        // stdin carries the token's line, the prompt, an end line made from the token, and
        // then stays open: when OfficeCraft stops the run or the connection drops, it closes, and
        // the watcher stops Claude Code on that machine too (without a terminal nothing else would).
        // The watcher reads fd 3, a copy of stdin (sh gives a background job /dev/null), with the
        // shell's own `read`, so killing the watcher leaves no reader behind.
        let script = format!(
            "exec 3<&0; IFS= read -r {env}; export {env}; end=\"END-${env}\"; tmp=$(mktemp) || exit 1; trap 'rm -f \"$tmp\"' EXIT; \
             while IFS= read -r line; do [ \"$line\" = \"$end\" ] && break; printf '%s\\n' \"$line\"; done > \"$tmp\"; \
             {REMOTE_PATH}; export PATH; {} {} < \"$tmp\" & pid=$!; \
             {{ while IFS= read -r x <&3; do :; done; kill \"$pid\" 2> /dev/null; }} > /dev/null 2>&1 & watch=$!; \
             wait \"$pid\"; status=$?; kill \"$watch\" 2> /dev/null; exit $status",
            program.join(" "),
            claude.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" "),
            env = TOKEN_ENV,
        );
        let args: Vec<String> = vec![
            "-T".into(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ExitOnForwardFailure=yes".into(),
            "-R".into(),
            format!("{remote_port}:127.0.0.1:{local_port}"),
            words[1].clone(),
            "sh".into(),
            "-c".into(),
            shell_quote(&script),
        ];
        return Ok(Launch { program: first.clone(), args, ssh: true });
    }
    let mut args: Vec<String> = words[1..].to_vec();
    args.extend(claude);
    Ok(Launch { program: first.clone(), args, ssh: false })
}

/// The environment variable the token travels in: Claude Code expands `${…}` in its MCP config.
const TOKEN_ENV: &str = "OFFICECRAFT_MCP_TOKEN";

/// Claude Code's `--mcp-config` for OfficeCraft's server at `port`. The token is not in it:
/// Claude Code fills in `${OFFICECRAFT_MCP_TOKEN}` from its environment.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn mcp_config_text(port: u16) -> String {
    json!({"mcpServers": {"officecraft": {"type": "http", "url": format!("http://127.0.0.1:{port}/mcp"), "headers": {"Authorization": format!("Bearer ${{{TOKEN_ENV}}}")}}}}).to_string()
}

/// What Claude Code's tool call says in the activity card, and whether it changes a document.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn describe_claude(name: &str, input: &Value) -> (String, bool) {
    let Some(tool) = name.strip_prefix("mcp__officecraft__") else { return (name.to_string(), false) };
    let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    match tool {
        "list_tabs" => ("Looked at the open tabs".into(), false),
        "app_tools" => ("Looked up the app's tools".into(), false),
        "show_tab" => ("Showed a tab".into(), false),
        "new_tab" => (format!("Opened a new {} tab", s("app")), true),
        "open_file" => (format!("Opened {}", s("path").rsplit(['/', '\\']).next().unwrap_or("")), true),
        "send_to" => (format!("Sent a tab to {}", s("to")), true),
        "save" => ("Saved".into(), true),
        "use_tool" => {
            let inner = s("tool");
            let args = input.get("arguments").cloned().unwrap_or(Value::Null);
            (describe(&inner, &args), !is_read_only(&inner, &args))
        }
        other => (other.to_string(), false),
    }
}

/// One line of `claude -p --output-format stream-json`, as chat events.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn claude_events(line: &str) -> Vec<ClaudeEv> {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return Vec::new() };
    let mut out = Vec::new();
    match v.get("type").and_then(Value::as_str).unwrap_or("") {
        "system" => {
            if let Some(id) = v.get("session_id").and_then(Value::as_str) {
                out.push(ClaudeEv::Session(id.to_string()));
            }
            // The tunnel or the token failed: say so now, not after a confused answer.
            if let Some(servers) = v.get("mcp_servers").and_then(Value::as_array)
                && let Some(st) = servers.iter().find(|x| x.get("name").and_then(Value::as_str) == Some("officecraft")).and_then(|x| x.get("status").and_then(Value::as_str))
                && st != "connected"
            {
                out.push(ClaudeEv::Done(Err(format!("Claude Code could not reach OfficeCraft ({st}). Over SSH, check that the command reaches the other computer and that port forwarding is allowed there."))));
            }
        }
        "assistant" => {
            for b in v.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                match b.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let t = b.get("text").and_then(Value::as_str).unwrap_or("").trim();
                        if !t.is_empty() {
                            out.push(ClaudeEv::Text(t.to_string()));
                        }
                    }
                    Some("tool_use") => {
                        let (label, edit) = describe_claude(b.get("name").and_then(Value::as_str).unwrap_or(""), b.get("input").unwrap_or(&Value::Null));
                        out.push(ClaudeEv::Tool { id: b.get("id").and_then(Value::as_str).unwrap_or("").to_string(), label, edit });
                    }
                    _ => {}
                }
            }
        }
        "user" => {
            for b in v.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                    let ok = !b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                    out.push(ClaudeEv::ToolDone { id: b.get("tool_use_id").and_then(Value::as_str).unwrap_or("").to_string(), ok });
                }
            }
        }
        "result" => {
            if let Some(id) = v.get("session_id").and_then(Value::as_str) {
                out.push(ClaudeEv::Session(id.to_string()));
            }
            let err = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            if err {
                let why = v.get("result").and_then(Value::as_str).or_else(|| v.get("subtype").and_then(Value::as_str)).unwrap_or("Claude Code stopped with an error");
                out.push(ClaudeEv::Done(Err(why.to_string())));
            } else {
                out.push(ClaudeEv::Done(Ok(())));
            }
        }
        _ => {}
    }
    out
}

/// `claude --version` through Settings' command (over ssh too): the version, or why not.
#[cfg(not(target_arch = "wasm32"))]
fn claude_version(cmd: &str) -> Result<String, String> {
    let words = split_words(cmd);
    let Some(first) = words.first() else { return Err("no command".into()) };
    let base = first.rsplit(['/', '\\']).next().unwrap_or(first);
    let args: Vec<String> = if base == "ssh" || base == "ssh.exe" {
        if words.len() < 3 {
            return Err("write the ssh command in full, with the host and the command: ssh my-nas claude".into());
        }
        let script = format!("{REMOTE_PATH}; export PATH; exec {} --version", words[2..].join(" "));
        vec!["-T".into(), "-o".into(), "BatchMode=yes".into(), "-o".into(), "ConnectTimeout=10".into(), words[1].clone(), "sh".into(), "-c".into(), shell_quote(&script)]
    } else {
        let mut a = words[1..].to_vec();
        a.push("--version".into());
        a
    };
    let mut child = std::process::Command::new(first)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run {first}: {e}"))?;
    // A host that takes the connection and then hangs must not leave the test spinning.
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > std::time::Duration::from_secs(25) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{first} did not answer within 25 seconds"));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(e) => return Err(e.to_string()),
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && !text.is_empty() {
        Ok(text.lines().next().unwrap_or("").to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { format!("{first} exited with {}", out.status) } else { err.lines().last().unwrap_or("").to_string() })
    }
}

/// Starts one Claude Code turn: `prompt` in, events out on the returned run.
#[cfg(not(target_arch = "wasm32"))]
fn start_claude(cfg: &Config, prompt: &str, system: &str, resume: Option<&str>, ctx: &egui::Context) -> Result<ClaudeRun, String> {
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};
    let port = crate::mcp::start(ctx)?;
    let launch = claude_argv(&cfg.claude_command(), port, crate::mcp::random_port(), system, &cfg.model, resume)?;
    let token = crate::mcp::issue_token()?;
    let program = launch.program.clone();
    let mut command = Command::new(&launch.program);
    command.args(&launch.args);
    // Locally Claude Code reads the token from its environment; over ssh it travels on stdin, and
    // the local ssh process has no use for it.
    if !launch.ssh {
        command.env(TOKEN_ENV, &token);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            crate::mcp::revoke(&token);
            if e.kind() == std::io::ErrorKind::NotFound { format!("{program} was not found: install Claude Code, or set the command in Settings") } else { format!("could not start {program}: {e}") }
        })?;
    let held = std::sync::Arc::new(std::sync::Mutex::new(None));
    if let Some(mut stdin) = child.stdin.take() {
        // Over ssh: the token's line, the prompt, the end line, and stdin stays open for the run.
        // Locally: the prompt, then end of input.
        let ssh = launch.ssh;
        let p = if ssh { format!("{token}\n{prompt}\nEND-{token}\n") } else { prompt.to_string() };
        let held = held.clone();
        std::thread::spawn(move || {
            let _ = stdin.write_all(p.as_bytes());
            let _ = stdin.flush();
            if ssh {
                *held.lock().unwrap_or_else(|e| e.into_inner()) = Some(stdin);
            }
        });
    }
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let child = std::sync::Arc::new(std::sync::Mutex::new(Some(child)));
    let (tx, rx) = channel();
    // What Claude Code (or ssh) wrote to stderr: the reason, when it stops without a result.
    let errors = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    if let Some(err) = stderr {
        let errors = errors.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
                let mut e = errors.lock().unwrap_or_else(|e| e.into_inner());
                if e.len() < 4000 {
                    e.push_str(&line);
                    e.push('\n');
                }
            }
        });
    }
    let waiter = child.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let mut finished = false;
        if let Some(out) = stdout {
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                for ev in claude_events(&line) {
                    if matches!(ev, ClaudeEv::Done(_)) {
                        finished = true;
                    }
                    let _ = tx.send(ev);
                }
                ctx.request_repaint();
            }
        }
        // Wait for the process without holding the lock, so Stop (which kills it) never blocks.
        let mut status = None;
        loop {
            let mut slot = waiter.lock().unwrap_or_else(|e| e.into_inner());
            let Some(c) = slot.as_mut() else { break };
            match c.try_wait() {
                Ok(None) => {}
                Ok(Some(s)) => {
                    status = Some(s);
                    *slot = None;
                    break;
                }
                Err(_) => {
                    let _ = c.kill();
                    *slot = None;
                    break;
                }
            }
            drop(slot);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if !finished {
            // Give the stderr reader a moment to collect the last lines.
            std::thread::sleep(std::time::Duration::from_millis(100));
            let e = errors.lock().unwrap_or_else(|e| e.into_inner()).trim().to_string();
            let why = match (e.lines().last(), status) {
                (Some(l), _) if !l.is_empty() => l.to_string(),
                (_, Some(s)) if !s.success() => format!("Claude Code exited with {s}"),
                _ => "Claude Code stopped without an answer".to_string(),
            };
            let _ = tx.send(ClaudeEv::Done(Err(why)));
        }
        ctx.request_repaint();
    });
    Ok(ClaudeRun { rx, child, token, stdin: held })
}

/// What Claude Code is told about where it is, appended to its own system prompt.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn claude_system(app: App, tab: u64) -> String {
    format!(
        "You are the assistant inside OfficeCraft, an office suite open on the user's screen, and you work in their \
         documents live through the officecraft tools; there are no other tools. The user is looking at tab {tab}, a \
         {noun}: \"this document\" means that tab unless they say otherwise. Call app_tools for a tab before use_tool. \
         Read before you write, change only what was asked, keep what is already there, and save only when asked. \
         When you are done, say what you changed in one or two plain sentences.",
        noun = app.noun()
    )
}

#[cfg(not(target_arch = "wasm32"))]
impl Settings {
    /// Claude Code sessions the user starts themselves: the MCP server stays on, and this is the
    /// command that connects one.
    fn mcp_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.checkbox(&mut self.draft.mcp_listen, "Also let Claude Code sessions I start myself work in OfficeCraft");
        if !self.draft.mcp_listen {
            return;
        }
        let port = crate::mcp::port().unwrap_or(crate::mcp::BASE_PORT);
        if self.mcp_token.is_none() {
            self.mcp_token = Some(crate::mcp::saved_token().unwrap_or_default());
        }
        let token = self.mcp_token.clone().unwrap_or_default();
        let command = |t: &str| format!("claude mcp add --transport http officecraft {} --header \"Authorization: Bearer {t}\"", crate::mcp::url(port));
        ui.label(
            egui::RichText::new(format!(
                "Run this once where Claude Code runs. On another computer, connect with ssh -R {port}:127.0.0.1:{port} first, so it can reach OfficeCraft here."
            ))
            .size(12.0)
            .weak(),
        );
        let shown = if self.reveal { command(&token) } else { command("…") };
        ui.add(egui::Label::new(egui::RichText::new(shown).monospace().size(11.5)).wrap());
        ui.horizontal(|ui| {
            if ui.button("Copy command").clicked() {
                ui.ctx().copy_text(command(&token));
                self.saved_note = "Copied. The command holds the token: treat it like a password.".into();
            }
            ui.checkbox(&mut self.reveal, "Show token");
            if ui.button("New token").on_hover_text("The old token stops working; run the new command again").clicked() {
                self.saved_note = match crate::mcp::new_saved_token() {
                    Ok(t) => {
                        self.mcp_token = Some(t);
                        "Made a new token: run the new command again.".into()
                    }
                    Err(e) => format!("Could not make a new token: {e}"),
                };
            }
        });
    }
}

#[cfg(test)]
mod claude_tests {
    use super::*;

    #[test]
    fn commands_split_like_a_shell() {
        assert_eq!(split_words("ssh my-nas claude"), ["ssh", "my-nas", "claude"]);
        assert_eq!(split_words("  '/opt/my tools/claude'  --x \"a b\" ''"), ["/opt/my tools/claude", "--x", "a b", ""]);
        assert!(split_words("   ").is_empty());
    }

    #[test]
    fn remote_arguments_are_quoted_for_the_shell() {
        assert_eq!(shell_quote("claude"), "claude");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote(r#"{"a": 1}"#), r#"'{"a": 1}'"#);
        assert_eq!(shell_quote("$(rm -rf ~)"), "'$(rm -rf ~)'");
    }

    #[test]
    fn a_local_command_runs_claude_with_only_officecraft_tools() {
        let l = claude_argv("claude", 7990, 31000, "sys", "", None).unwrap();
        assert_eq!(l.program, "claude");
        assert!(!l.ssh);
        let joined = l.args.join(" ");
        assert!(joined.starts_with("-p --output-format stream-json --verbose --tools  --strict-mcp-config --mcp-config "), "{joined}");
        assert!(joined.contains("127.0.0.1:7990/mcp") && joined.contains("Bearer ${OFFICECRAFT_MCP_TOKEN}"), "the local port, and no token: {joined}");
        assert!(l.args.windows(2).any(|w| w == ["--allowedTools", "mcp__officecraft"]));
        assert!(!l.args.contains(&"--model".to_string()) && !l.args.contains(&"--resume".to_string()));
        let l = claude_argv("claude", 7990, 31000, "sys", "opus", Some("abc")).unwrap();
        assert!(l.args.windows(2).any(|w| w == ["--model", "opus"]) && l.args.windows(2).any(|w| w == ["--resume", "abc"]));
    }

    #[test]
    fn over_ssh_the_port_comes_back_and_the_remote_script_is_quoted() {
        let l = claude_argv("ssh my-nas claude", 7991, 31234, "be 'good'", "", None).unwrap();
        assert_eq!(l.program, "ssh");
        assert!(l.ssh);
        assert_eq!(&l.args[..7], ["-T", "-o", "BatchMode=yes", "-o", "ExitOnForwardFailure=yes", "-R", "31234:127.0.0.1:7991"]);
        assert_eq!(&l.args[7..10], ["my-nas", "sh", "-c"]);
        assert_eq!(l.args.len(), 11, "the whole remote side is one quoted word for the login shell");
        let script = &l.args[10];
        assert!(script.starts_with("'exec 3<&0; IFS= read -r OFFICECRAFT_MCP_TOKEN; export OFFICECRAFT_MCP_TOKEN;"), "{script}");
        assert!(script.contains("127.0.0.1:31234/mcp"), "the far end uses the tunnel's port: {script}");
        assert!(!script.contains("Bearer ") || script.contains("Bearer ${OFFICECRAFT_MCP_TOKEN}"), "no token in the script");
        assert!(claude_argv("ssh my-nas", 7991, 31234, "s", "", None).is_err());
        assert!(claude_argv("", 7991, 31234, "s", "", None).is_err());
    }

    /// Runs the remote side of an ssh launch with `sh`, a fake `claude` in a temporary HOME, and
    /// the stdin OfficeCraft sends. `hold`: keep stdin open this long before closing it.
    #[cfg(unix)]
    fn run_remote_script(fake_claude: &str, input: &str, hold: std::time::Duration) -> (String, std::time::Duration) {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let home = std::env::temp_dir().join(format!("oc-remote-test-{}-{}", std::process::id(), crate::ai::claude_tests::COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        let bin = home.join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let fake = bin.join("claude");
        std::fs::write(&fake, fake_claude).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let l = claude_argv("ssh host claude", 7990, 31000, "sys", "", None).unwrap();
        // What sshd does: the words after the host, joined, run by the login shell.
        let remote = l.args[8..].join(" ");
        let started = std::time::Instant::now();
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(&remote)
            .env("HOME", &home)
            .env("PATH", "/usr/bin:/bin")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input.as_bytes()).unwrap();
        // stdin closes after `hold` on its own thread; the time measured is the script's.
        let holder = std::thread::spawn(move || {
            std::thread::sleep(hold);
            drop(stdin);
        });
        let out = child.wait_with_output().unwrap();
        let took = started.elapsed();
        holder.join().unwrap();
        let _ = std::fs::remove_dir_all(&home);
        (String::from_utf8_lossy(&out.stdout).into_owned(), took)
    }

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    #[cfg(unix)]
    #[test]
    fn the_remote_script_passes_the_prompt_and_token_and_stops_when_stdin_closes() {
        // A claude that echoes its prompt and the token it sees, then would run for a long time.
        // (One process, like Claude Code with its tools off: `exec`, so the kill reaches the sleep.)
        let fake = "#!/bin/sh\necho \"token=$OFFICECRAFT_MCP_TOKEN\"\ncat\necho done-reading\nexec sleep 30\n";
        let input = "tok123\nline one\nEND-not-it\nline 'two' $HOME\nEND-tok123\n";
        let (out, took) = run_remote_script(fake, input, std::time::Duration::from_millis(800));
        assert!(took >= std::time::Duration::from_millis(700), "it waited for stdin to close: {took:?}");
        assert!(out.contains("token=tok123"), "{out}");
        assert!(out.contains("line one\nEND-not-it\nline 'two' $HOME\ndone-reading"), "the prompt, taken literally: {out}");
        assert!(!out.contains("END-tok123"), "the end line is not part of the prompt: {out}");
        assert!(took < std::time::Duration::from_secs(10), "stopped promptly, not after the 30 s sleep: {took:?}");
    }

    #[cfg(unix)]
    #[test]
    fn the_remote_script_ends_with_claude_while_stdin_is_still_open() {
        let fake = "#!/bin/sh\ncat > /dev/null\necho answered\n";
        let (out, took) = run_remote_script(fake, "t\nhi\nEND-t\n", std::time::Duration::from_secs(5));
        assert!(out.contains("answered"), "{out}");
        // It ended with claude, while stdin was still open (closed only after 5 s).
        assert!(took < std::time::Duration::from_secs(3), "{took:?}");
    }

    #[test]
    fn stream_json_lines_become_chat_events() {
        let init = r#"{"type":"system","subtype":"init","session_id":"s1","mcp_servers":[{"name":"officecraft","status":"connected"}]}"#;
        assert!(matches!(claude_events(init).as_slice(), [ClaudeEv::Session(s)] if s == "s1"));
        let failed = r#"{"type":"system","subtype":"init","session_id":"s1","mcp_servers":[{"name":"officecraft","status":"failed"}]}"#;
        assert!(matches!(claude_events(failed).last(), Some(ClaudeEv::Done(Err(e))) if e.contains("failed")));
        let turn = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"On it."},{"type":"tool_use","id":"t1","name":"mcp__officecraft__use_tool","input":{"tab":1,"tool":"insert_blocks","arguments":{"blocks":[{"text":"a"},{"text":"b"}]}}}]}}"#;
        match claude_events(turn).as_slice() {
            [ClaudeEv::Text(t), ClaudeEv::Tool { id, label, edit }] => {
                assert_eq!(t, "On it.");
                assert_eq!(id, "t1");
                assert_eq!(label, "Wrote 2 paragraphs");
                assert!(*edit);
            }
            _ => panic!("text and a tool call"),
        }
        let result = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true}]}}"#;
        assert!(matches!(claude_events(result).as_slice(), [ClaudeEv::ToolDone { id, ok: false }] if id == "t1"));
        let done = r#"{"type":"result","subtype":"success","is_error":false,"result":"Done.","session_id":"s1"}"#;
        assert!(matches!(claude_events(done).last(), Some(ClaudeEv::Done(Ok(())))));
        let err = r#"{"type":"result","subtype":"error_max_turns","is_error":true,"session_id":"s1"}"#;
        assert!(matches!(claude_events(err).last(), Some(ClaudeEv::Done(Err(e))) if e == "error_max_turns"));
        assert!(claude_events("not json").is_empty());
    }

    #[test]
    fn reading_tools_are_not_edits() {
        assert_eq!(describe_claude("mcp__officecraft__list_tabs", &json!({})), ("Looked at the open tabs".to_string(), false));
        let (l, edit) = describe_claude("mcp__officecraft__use_tool", &json!({"tool": "read_range", "arguments": {"range": "A1:B2"}}));
        assert_eq!(l, "Read A1:B2");
        assert!(!edit);
        assert!(describe_claude("mcp__officecraft__save", &json!({})).1);
    }
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

#[cfg(test)]
mod endpoint_tests {
    use super::{insecure_remote, url_host};

    #[test]
    fn hosts_and_plain_http() {
        assert_eq!(url_host("https://API.DeepSeek.com/v1"), "api.deepseek.com");
        assert_eq!(url_host("http://nas.local:8317/v1"), "nas.local:8317");
        assert!(insecure_remote("http://nas.local:8317/v1"));
        assert!(!insecure_remote("http://127.0.0.1:11434/v1"));
        assert!(!insecure_remote("https://api.groq.com/openai/v1"));
    }
}
