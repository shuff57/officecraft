//! OfficeCraft: WordCraft, GridCraft, DeckCraft, PdfCraft and viSHual (graphing) as tabs in one
//! window.
//!
//! Each tab is the real app's UI struct, created the first time its tab is opened. Only the
//! active tab sees input and runs its frame logic, so shortcuts never leak between apps.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use eframe::egui;

mod ai;
mod graph;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Word,
    Grid,
    Deck,
    Pdf,
    Graph,
}

impl Kind {
    const ALL: [Kind; 5] = [Kind::Word, Kind::Grid, Kind::Deck, Kind::Pdf, Kind::Graph];
    fn label(self) -> &'static str {
        match self {
            Kind::Word => "Docs",
            Kind::Grid => "Sheets",
            Kind::Deck => "Slides",
            Kind::Pdf => "PDF",
            Kind::Graph => "Graph",
        }
    }
}

/// One live app. Boxed so the tab strip does not care which it is.
enum Tab {
    Word(Box<wordcraft_ui_egui::WordApp>),
    Grid(Box<gridcraft_ui_egui::SheetApp>),
    Deck(Box<deckcraft_ui_egui::SlideApp>),
    Pdf(Box<pdfcraft_ui_egui::PdfCraftApp>),
    Graph(Box<graph::GraphApp>),
}

fn word_services() -> wordcraft_ui_egui::Services {
    wordcraft_ui_egui::Services {
        pick_open: Some(Box::new(|purpose: &str| {
            let d = rfd::FileDialog::new();
            let d = if purpose == "picture" {
                d.add_filter("Pictures", &["png", "jpg", "jpeg", "gif", "webp", "bmp"])
            } else {
                d.add_filter("Documents", &["docx", "docm", "dotx", "odt", "rtf", "txt", "md", "html", "htm", "json"])
            };
            d.pick_file().map(|p| p.to_string_lossy().to_string())
        })),
        pick_save: Some(Box::new(|name: &str| rfd::FileDialog::new().set_file_name(name).save_file().map(|p| p.to_string_lossy().to_string()))),
        ..Default::default()
    }
}

fn grid_services() -> gridcraft_ui_egui::Services {
    gridcraft_ui_egui::Services {
        pick_open: Some(Box::new(|| {
            rfd::FileDialog::new()
                .add_filter("Spreadsheets", &["xlsx", "xlsm", "csv", "tsv", "txt", "json"])
                .pick_file()
                .and_then(|p| p.to_str().map(str::to_string))
        })),
        pick_save: Some(Box::new(|suggested: &str| {
            let stem = std::path::Path::new(suggested).file_stem().and_then(|s| s.to_str()).unwrap_or("Book1").to_string();
            rfd::FileDialog::new()
                .set_file_name(format!("{stem}.xlsx"))
                .add_filter("Excel Workbook (.xlsx)", &["xlsx"])
                .add_filter("CSV UTF-8 (.csv)", &["csv"])
                .save_file()
                .and_then(|p| p.to_str().map(str::to_string))
        })),
        ..Default::default()
    }
}

fn deck_services() -> deckcraft_ui_egui::Services {
    deckcraft_ui_egui::Services {
        pick_open: Some(Box::new(|_purpose: &str| {
            rfd::FileDialog::new()
                .add_filter("Presentations", &["deckcraft", "pptx", "potx", "ppsx"])
                .pick_file()
                .map(|p| p.to_string_lossy().to_string())
        })),
        pick_save: Some(Box::new(|name: &str| rfd::FileDialog::new().set_file_name(name).save_file().map(|p| p.to_string_lossy().to_string()))),
        read: Some(Box::new(|p: &str| std::fs::read(p).map_err(|e| e.to_string()))),
        write: Some(Box::new(|p: &str, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string()))),
        ..Default::default()
    }
}

impl Tab {
    fn create(kind: Kind) -> Tab {
        match kind {
            Kind::Word => {
                let doc = wordcraft_doc::Document::new();
                Tab::Word(Box::new(wordcraft_ui_egui::WordApp::new(wordcraft_engine::Session::new(doc), word_services())))
            }
            Kind::Grid => {
                let mut session = gridcraft_engine::Session::new();
                session.new_workbook();
                Tab::Grid(Box::new(gridcraft_ui_egui::SheetApp::new(session, grid_services())))
            }
            Kind::Deck => Tab::Deck(Box::new(deckcraft_ui_egui::SlideApp::new(deckcraft_engine::Session::new(), deck_services()))),
            Kind::Pdf => Tab::Pdf(Box::new(pdfcraft_ui_egui::PdfCraftApp::new())),
            Kind::Graph => Tab::Graph(Box::new(graph::GraphApp::new())),
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        match self {
            Tab::Word(a) => a.logic(ctx),
            Tab::Grid(a) => a.logic(ctx),
            Tab::Deck(a) => a.logic(ctx),
            Tab::Pdf(a) => eframe::App::logic(a.as_mut(), ctx, frame),
            Tab::Graph(a) => a.logic(ctx),
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        match self {
            Tab::Word(a) => a.raw_input_hook(raw),
            Tab::Grid(a) => a.raw_input_hook(raw),
            Tab::Deck(a) => a.raw_input_hook(raw),
            Tab::Pdf(a) => eframe::App::raw_input_hook(a.as_mut(), ctx, raw),
            Tab::Graph(a) => a.raw_input_hook(raw),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        match self {
            Tab::Word(a) => a.ui(ui),
            Tab::Grid(a) => a.ui(ui),
            Tab::Deck(a) => a.ui(ui),
            Tab::Pdf(a) => eframe::App::ui(a.as_mut(), ui, frame),
            Tab::Graph(a) => a.ui(ui),
        }
    }
}

/// What the window shows: the Home page or one open tab (by id).
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Home,
    Tab(u64),
}

/// Which app opens a file, by extension.
fn kind_for(path: &str) -> Option<Kind> {
    let ext = std::path::Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "docx" | "docm" | "dotx" | "odt" | "rtf" | "txt" | "md" | "html" | "htm" => Some(Kind::Word),
        "xlsx" | "xlsm" | "csv" | "tsv" => Some(Kind::Grid),
        "pptx" | "potx" | "ppsx" | "deckcraft" => Some(Kind::Deck),
        "pdf" => Some(Kind::Pdf),
        "vishual" => Some(Kind::Graph),
        _ => None,
    }
}

const OPEN_EXTS: &[&str] = &[
    "docx", "docm", "dotx", "odt", "rtf", "txt", "md", "html", "htm", "xlsx", "xlsm", "csv", "tsv", "pptx", "potx", "ppsx", "deckcraft", "pdf", "vishual",
];
const MAX_RECENT: usize = 12;

fn recent_file() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(base.join("officecraft").join("recent.txt"))
}

fn load_recent() -> Vec<String> {
    let Some(p) = recent_file() else { return Vec::new() };
    std::fs::read_to_string(p).map(|s| s.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()).unwrap_or_default()
}

fn save_recent(list: &[String]) {
    if let Some(p) = recent_file() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, list.join("\n"));
    }
}

impl Tab {
    /// Unsaved work in any document this instance holds.
    fn is_dirty(&self) -> bool {
        match self {
            Tab::Word(a) => a.session.dirty,
            Tab::Grid(a) => a.session.documents().iter().any(|d| d.is_dirty()),
            Tab::Deck(a) => a.session.documents().iter().any(|d| d.is_dirty()),
            Tab::Pdf(a) => a.first_dirty().is_some(),
            Tab::Graph(a) => a.changed,
        }
    }

    /// The apps share one egui context and each installs its own fonts and visuals into it.
    /// After another app has drawn, this one must install them again.
    fn context_changed(&mut self) {
        match self {
            Tab::Word(a) => a.context_changed(),
            Tab::Grid(a) => a.context_changed(),
            Tab::Deck(a) => a.context_changed(),
            Tab::Pdf(a) => a.context_changed(),
            Tab::Graph(a) => a.context_changed(),
        }
    }

    fn open(&mut self, path: &str) -> Result<(), String> {
        match self {
            Tab::Word(a) => a.run("file.open", serde_json::json!({"path": path})).map(|_| ()),
            Tab::Grid(a) => {
                a.open_path(path);
                Ok(())
            }
            Tab::Deck(a) => a.open_path(path),
            Tab::Pdf(a) => {
                a.open_path(path);
                Ok(())
            }
            Tab::Graph(a) => a.open_path(path),
        }
    }
}

/// One open window of an app. Several of the same kind can be open at once.
struct Entry {
    id: u64,
    kind: Kind,
    name: String,
    path: Option<String>,
    tab: Tab,
    chat: ai::Chat,
    /// PDF is driven over its control channel; the client is made the first time it is needed.
    pdf_client: Option<pdfcraft_ui_egui::control::ControlClient>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Only(Kind),
}

/// One file in the Home table.
#[derive(Clone)]
struct Row {
    path: String,
    name: String,
    folder: String,
    kind: Option<Kind>,
    modified: Option<std::time::SystemTime>,
    size: u64,
}

fn row_for(path: &str) -> Row {
    let p = std::path::Path::new(path);
    let meta = std::fs::metadata(p).ok();
    Row {
        path: path.to_string(),
        name: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string()),
        folder: p.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        kind: kind_for(path),
        modified: meta.as_ref().and_then(|m| m.modified().ok()),
        size: meta.map(|m| m.len()).unwrap_or(0),
    }
}

/// Supported files directly inside `dir`, newest first.
fn list_folder(dir: &str) -> Vec<Row> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut rows: Vec<Row> = read
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| e.path().to_str().map(str::to_string))
        .filter(|p| kind_for(p).is_some())
        .map(|p| row_for(&p))
        .collect();
    rows.sort_by(|a, b| b.modified.cmp(&a.modified));
    rows.truncate(500);
    rows
}

fn folders_file() -> Option<std::path::PathBuf> {
    recent_file().map(|p| p.with_file_name("folders.txt"))
}

fn load_folders() -> Vec<String> {
    let Some(p) = folders_file() else { return Vec::new() };
    std::fs::read_to_string(p).map(|s| s.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()).unwrap_or_default()
}

fn save_folders(list: &[String]) {
    if let Some(p) = folders_file() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, list.join("\n"));
    }
}

/// A rounded square in the app's colour with its initial: the file-type mark used on tabs and Home.
fn badge(ui: &mut egui::Ui, kind: Option<Kind>, size: f32) {
    let (color, text) = match kind {
        Some(Kind::Word) => (egui::Color32::from_rgb(46, 110, 200), "D"),
        Some(Kind::Grid) => (egui::Color32::from_rgb(36, 140, 84), "S"),
        Some(Kind::Deck) => (egui::Color32::from_rgb(214, 104, 38), "P"),
        Some(Kind::Pdf) => (egui::Color32::from_rgb(196, 48, 48), "PDF"),
        Some(Kind::Graph) => (egui::Color32::from_rgb(110, 70, 190), "G"),
        None => (egui::Color32::GRAY, "?"),
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter().rect_filled(rect, size * 0.22, color);
    let font = if text.len() > 1 { size * 0.3 } else { size * 0.52 };
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(font), egui::Color32::WHITE);
}

fn ago(t: std::time::SystemTime) -> String {
    let secs = std::time::SystemTime::now().duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86399 => format!("{} h ago", secs / 3600),
        86400..=2591999 => format!("{} d ago", secs / 86400),
        _ => format!("{} mo ago", secs / 2592000),
    }
}

fn human_size(n: u64) -> String {
    match n {
        0 => String::new(),
        1..=1023 => format!("{n} B"),
        1024..=1048575 => format!("{} KB", n / 1024),
        _ => format!("{:.1} MB", n as f64 / 1048576.0),
    }
}

fn session_file() -> Option<std::path::PathBuf> {
    recent_file().map(|p| p.with_file_name("session.txt"))
}

/// Marks that Home, not a file, was showing when the session was saved.
const HOME_MARK: &str = "#home";

/// Files that were open last time (the active one last), and whether Home was showing.
fn load_session() -> (Vec<String>, bool) {
    let Some(p) = session_file() else { return (Vec::new(), false) };
    let text = std::fs::read_to_string(p).unwrap_or_default();
    let home = text.lines().any(|l| l == HOME_MARK);
    let files = text.lines().filter(|l| !l.is_empty() && *l != HOME_MARK && std::path::Path::new(l).is_file()).map(str::to_string).collect();
    (files, home)
}

fn save_session(lines: &[String]) {
    if let Some(p) = session_file() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, lines.join("\n"));
    }
}

struct Suite {
    view: View,
    entries: Vec<Entry>,
    next_id: u64,
    recent: Vec<String>,
    /// Paths dropped on the window or named on the command line, opened on the next frame.
    pending: Vec<String>,
    notice: Option<String>,
    /// A tab with unsaved work whose close is waiting for confirmation.
    confirm_close: Option<u64>,
    ai_cfg: ai::Config,
    /// Which app last installed its fonts and visuals into the one shared egui context.
    ctx_owner: Option<Kind>,
    /// `OFFICECRAFT_SELFTEST=1`: cycle through every app tab and Home, then exit 0.
    selftest: Option<u64>,
    /// Open files are saved as they change and reopened next launch. Off in self-test runs.
    persist: bool,
    last_session: Vec<String>,
    restore_home: bool,
    /// `OFFICECRAFT_SELFTEST_AI=<prompt>`: send the prompt through the Docs assistant, print the
    /// transcript and the document afterwards, then exit (0 when the model answered without an error).
    ai_selftest: Option<(String, Option<std::time::Instant>)>,
    filter: Filter,
    search: String,
    folders: Vec<String>,
    folder_sel: Option<usize>,
    folder_rows: Vec<Row>,
}

impl Suite {
    fn new() -> Self {
        Suite { view: View::Home, entries: Vec::new(), next_id: 1, recent: load_recent(), pending: Vec::new(), notice: None, confirm_close: None, ai_cfg: ai::Config::load(), ctx_owner: None, selftest: None, ai_selftest: None, persist: true, last_session: Vec::new(), restore_home: false, filter: Filter::All, search: String::new(), folders: load_folders(), folder_sel: None, folder_rows: Vec::new() }
    }

    /// Starts a fresh, empty instance of `kind` and shows it.
    fn new_tab(&mut self, kind: Kind) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let n = self.entries.iter().filter(|e| e.kind == kind).count() + 1;
        self.entries.push(Entry { id, kind, name: format!("{} {n}", kind.label()), path: None, tab: Tab::create(kind), chat: ai::Chat::new(), pdf_client: None });
        self.view = View::Tab(id);
        id
    }

    fn entry_mut(&mut self, id: u64) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    /// Opens `path` in its own tab, or switches to the tab that already has it.
    fn open_file(&mut self, ctx: &egui::Context, path: &str) {
        if let Some(e) = self.entries.iter().find(|e| e.path.as_deref() == Some(path)) {
            self.view = View::Tab(e.id);
            ctx.request_repaint();
            return;
        }
        let Some(kind) = kind_for(path) else {
            self.notice = Some(format!("No Craft app opens {path}"));
            return;
        };
        let prev = self.view;
        let id = self.new_tab(kind);
        let result = self.entry_mut(id).map(|e| e.tab.open(path)).unwrap_or(Err("tab missing".into()));
        match result {
            Ok(()) => {
                if let Some(e) = self.entry_mut(id) {
                    e.path = Some(path.to_string());
                    e.name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
                }
                self.notice = None;
                self.recent.retain(|p| p != path);
                self.recent.insert(0, path.to_string());
                self.recent.truncate(MAX_RECENT);
                save_recent(&self.recent);
            }
            Err(e) => {
                self.entries.retain(|x| x.id != id);
                self.view = prev;
                self.notice = Some(format!("{path}: {e}"));
            }
        }
        ctx.request_repaint();
    }

    fn close(&mut self, id: u64) {
        self.entries.retain(|e| e.id != id);
        self.confirm_close = None;
        if self.view == View::Tab(id) {
            self.view = self.entries.last().map(|e| View::Tab(e.id)).unwrap_or(View::Home);
        }
    }

    fn drive_ai_selftest(&mut self, ctx: &egui::Context, prompt: String, started: Option<std::time::Instant>) {
        ctx.request_repaint();
        let Some(t0) = started else {
            let id = self.new_tab(Kind::Word);
            let cfg = self.ai_cfg.clone();
            if let Some(e) = self.entry_mut(id) {
                e.chat.open = true;
                e.chat.ask_text(prompt.clone(), ai::App::Docs, &cfg, ctx);
            }
            eprintln!("ai-selftest: model {} at {}", cfg.model, cfg.base_url);
            self.ai_selftest = Some((prompt, Some(std::time::Instant::now())));
            return;
        };
        let timed_out = t0.elapsed().as_secs() > 240;
        let ctx2 = ctx.clone();
        let Some(e) = self.entries.last_mut() else { return };
        if e.chat.busy() && !timed_out {
            return;
        }
        eprintln!("---- transcript ----\n{}", e.chat.transcript());
        let mut host = TabHost { tab: &mut e.tab, client: &mut e.pdf_client, ctx: &ctx2 };
        if let ai::CallResult::Done(r) = ai::Host::call(&mut host, "document.inspect", serde_json::json!({"text": true})) {
            let text = match r {
                Ok(v) => v.to_string(),
                Err(e) => format!("inspect failed: {e}"),
            };
            eprintln!("---- document ----\n{}", text.chars().take(1500).collect::<String>());
        }
        let bad = timed_out || e.chat.has_error();
        eprintln!("ai-selftest: {} in {:.1}s", if bad { "FAILED" } else { "answered" }, t0.elapsed().as_secs_f32());
        std::process::exit(if bad { 1 } else { 0 });
    }

    /// Writes the open files (active one last) when they change. Skipped while a restore is
    /// still opening files, so a half-restored window never overwrites the full list.
    fn save_session_if_changed(&mut self) {
        if !self.persist || !self.pending.is_empty() || self.restore_home {
            return;
        }
        let active = match self.view {
            View::Tab(id) => Some(id),
            View::Home => None,
        };
        let mut lines: Vec<String> = self.entries.iter().filter(|e| Some(e.id) != active).filter_map(|e| e.path.clone()).collect();
        if let Some(p) = self.entries.iter().find(|e| Some(e.id) == active).and_then(|e| e.path.clone()) {
            lines.push(p);
        }
        if active.is_none() {
            lines.push(HOME_MARK.to_string());
        }
        if lines != self.last_session {
            save_session(&lines);
            self.last_session = lines;
        }
    }

    fn request_close(&mut self, id: u64) {
        if self.entries.iter().find(|e| e.id == id).is_some_and(|e| e.tab.is_dirty()) {
            self.confirm_close = Some(id);
        } else {
            self.close(id);
        }
    }

    fn home(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut open: Option<String> = None;
        let mut new: Option<Kind> = None;
        let mut pick_files = false;
        let mut add_folder = false;
        let mut remove_folder: Option<usize> = None;
        let mut select: Option<Option<usize>> = None;
        let mut want_filter = self.filter;

        let source: Vec<Row> = match self.folder_sel {
            None => self.recent.iter().map(|p| row_for(p)).collect(),
            Some(_) => self.folder_rows.clone(),
        };
        let q = self.search.to_lowercase();
        let rows: Vec<Row> = source
            .into_iter()
            .filter(|r| match want_filter {
                Filter::All => true,
                Filter::Only(k) => r.kind == Some(k),
            })
            .filter(|r| q.is_empty() || r.name.to_lowercase().contains(&q) || r.folder.to_lowercase().contains(&q))
            .collect();

        egui::Panel::left("home_sidebar").exact_size(232.0).show(ui, |ui| {
            ui.add_space(16.0);
            ui.heading("OfficeCraft");
            ui.add_space(14.0);
            if ui.selectable_label(self.folder_sel.is_none(), format!("Recent   {}", self.recent.len())).clicked() {
                select = Some(None);
            }
            ui.add_space(10.0);
            ui.separator();
            ui.strong("Folders");
            for (i, f) in self.folders.iter().enumerate() {
                let name = std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.clone());
                ui.horizontal(|ui| {
                    if ui.selectable_label(self.folder_sel == Some(i), name).on_hover_text(f).clicked() {
                        select = Some(Some(i));
                    }
                    if ui.small_button("×").on_hover_text("Remove from list (files stay on disk)").clicked() {
                        remove_folder = Some(i);
                    }
                });
            }
            if ui.button("Add folder…").clicked() {
                add_folder = true;
            }
        });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(14.0);
            ui.strong("QUICK START");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for (kind, title, sub) in [
                    (Kind::Word, "New document", ".docx"),
                    (Kind::Grid, "New spreadsheet", ".xlsx"),
                    (Kind::Deck, "New presentation", ".pptx"),
                    (Kind::Pdf, "PDF tools", ".pdf"),
                    (Kind::Graph, "New graph", ".vishual"),
                ] {
                    let card = egui::Frame::group(ui.style()).corner_radius(10.0).show(ui, |ui| {
                        ui.set_width(150.0);
                        badge(ui, Some(kind), 30.0);
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(title).strong());
                        ui.weak(sub);
                    });
                    let r = card.response.interact(egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                    if r.clicked() {
                        new = Some(kind);
                    }
                }
                let card = egui::Frame::group(ui.style()).corner_radius(10.0).show(ui, |ui| {
                    ui.set_width(150.0);
                    ui.label(egui::RichText::new("📂").size(26.0));
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Open file…").strong());
                    ui.weak("docx xlsx pptx pdf md html csv");
                });
                if card.response.interact(egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    pick_files = true;
                }
            });
            if let Some(n) = &self.notice {
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::LIGHT_RED, n);
            }

            ui.add_space(18.0);
            ui.horizontal(|ui| {
                for (f, label) in [
                    (Filter::All, "All"),
                    (Filter::Only(Kind::Word), "Docs"),
                    (Filter::Only(Kind::Grid), "Sheets"),
                    (Filter::Only(Kind::Deck), "Slides"),
                    (Filter::Only(Kind::Pdf), "PDF"),
                    (Filter::Only(Kind::Graph), "Graphs"),
                ] {
                    if ui.selectable_label(want_filter == f, label).clicked() {
                        want_filter = f;
                    }
                }
                ui.add_space(12.0);
                ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search files").desired_width(240.0));
            });
            ui.add_space(8.0);

            if rows.is_empty() {
                ui.weak(if self.folder_sel.is_some() { "No supported files in this folder." } else { "Files you open appear here." });
            } else {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    egui::Grid::new("home_files").num_columns(5).striped(true).spacing([18.0, 8.0]).min_col_width(24.0).show(ui, |ui| {
                        ui.label("");
                        ui.weak("Name");
                        ui.weak("Folder");
                        ui.weak("Modified");
                        ui.weak("Size");
                        ui.end_row();
                        for r in &rows {
                            badge(ui, r.kind, 20.0);
                            let name = ui.add(egui::Label::new(egui::RichText::new(&r.name)).sense(egui::Sense::click()).truncate());
                            if name.on_hover_text(&r.path).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                open = Some(r.path.clone());
                            }
                            ui.weak(&r.folder);
                            ui.weak(r.modified.map(ago).unwrap_or_default());
                            ui.weak(human_size(r.size));
                            ui.end_row();
                        }
                    });
                });
            }
        });

        self.filter = want_filter;
        if let Some(s) = select {
            self.folder_sel = s;
            self.folder_rows = s.and_then(|i| self.folders.get(i)).map(|d| list_folder(d)).unwrap_or_default();
        }
        if let Some(i) = remove_folder {
            self.folders.remove(i);
            save_folders(&self.folders);
            self.folder_sel = None;
            self.folder_rows.clear();
        }
        if add_folder && let Some(dir) = rfd::FileDialog::new().pick_folder() {
            let dir = dir.to_string_lossy().to_string();
            if !self.folders.contains(&dir) {
                self.folders.push(dir.clone());
                save_folders(&self.folders);
            }
            let i = self.folders.iter().position(|f| *f == dir);
            self.folder_sel = i;
            self.folder_rows = list_folder(&dir);
        }
        if pick_files && let Some(p) = rfd::FileDialog::new().add_filter("Office files", OPEN_EXTS).pick_files() {
            // Several files at once each get their own tab.
            self.pending.extend(p.iter().map(|f| f.to_string_lossy().to_string()));
            ctx.request_repaint();
        }
        if let Some(k) = new {
            self.new_tab(k);
            ctx.request_repaint();
        }
        if let Some(p) = open {
            self.open_file(&ctx, &p);
        }
    }
}

/// The open tab as the assistant's tools see it: each app's own control surface.
struct TabHost<'a> {
    tab: &'a mut Tab,
    client: &'a mut Option<pdfcraft_ui_egui::control::ControlClient>,
    ctx: &'a egui::Context,
}

/// Word, Grid and Deck answer `{"ok": …, "result"|"error": …}`; unwrap it.
fn envelope(v: serde_json::Value) -> Result<serde_json::Value, String> {
    match v.get("ok").and_then(serde_json::Value::as_bool) {
        Some(false) => Err(v.get("error").and_then(serde_json::Value::as_str).unwrap_or("command failed").to_string()),
        Some(true) => Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null)),
        None => Ok(v),
    }
}

const NO_SCREENSHOTS: &str = "screenshots are not available to the assistant";

impl ai::Host for TabHost<'_> {
    fn call(&mut self, method: &str, params: serde_json::Value) -> ai::CallResult {
        use ai::CallResult::{Done, Pending};
        match self.tab {
            Tab::Word(a) => {
                let (req, _rx) = wordcraft_ui_egui::control::ControlRequest::new(method, params);
                match wordcraft_ui_egui::control::handle(a.as_mut(), self.ctx, &req) {
                    wordcraft_ui_egui::control::Outcome::Done(v) => Done(envelope(v)),
                    _ => Done(Err(NO_SCREENSHOTS.into())),
                }
            }
            Tab::Deck(a) => {
                let (req, _rx) = deckcraft_ui_egui::control::ControlRequest::new(method, params);
                match deckcraft_ui_egui::control::handle(a.as_mut(), self.ctx, &req) {
                    deckcraft_ui_egui::control::Outcome::Done(v) => Done(envelope(v)),
                    _ => Done(Err(NO_SCREENSHOTS.into())),
                }
            }
            Tab::Grid(a) => match gridcraft_ui_egui::control::handle(a.as_mut(), self.ctx, method, &params) {
                gridcraft_ui_egui::control::Outcome::Done(v) => Done(envelope(v)),
                _ => Done(Err(NO_SCREENSHOTS.into())),
            },
            Tab::Pdf(a) => {
                let ctx = self.ctx;
                let client = self.client.get_or_insert_with(|| a.attach_control(ctx));
                Pending(client.send(method, params))
            }
            Tab::Graph(a) => Done(a.control(method, params)),
        }
    }
}

fn ai_app(kind: Kind) -> ai::App {
    match kind {
        Kind::Word => ai::App::Docs,
        Kind::Grid => ai::App::Sheets,
        Kind::Deck => ai::App::Slides,
        Kind::Pdf => ai::App::Pdf,
        Kind::Graph => ai::App::Graph,
    }
}

impl eframe::App for Suite {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Some(n) = self.selftest {
            const EVERY: u64 = 40;
            if n == 0 {
                for k in Kind::ALL {
                    self.new_tab(k);
                }
            }
            let step = (n / EVERY) as usize;
            if step >= 12 {
                eprintln!("selftest: cycled {step} views without a panic");
                std::process::exit(0);
            }
            if n % EVERY == 0 {
                let ids: Vec<u64> = self.entries.iter().map(|e| e.id).collect();
                // Word, Grid, Deck, Pdf, Home, then the same again in another order.
                let order = [0usize, 1, 2, 3, 4, 2, 0, 3, 1, 4, 0, 2];
                let i = order[step % order.len()];
                self.view = ids.get(i).map(|id| View::Tab(*id)).unwrap_or(View::Home);
            }
            self.selftest = Some(n + 1);
            ctx.request_repaint();
        }
        for p in std::mem::take(&mut self.pending) {
            self.open_file(ctx, &p);
        }
        if self.restore_home && self.pending.is_empty() {
            self.view = View::Home;
            self.restore_home = false;
        }
        self.save_session_if_changed();
        if let View::Tab(id) = self.view
            && let Some(e) = self.entries.iter_mut().find(|e| e.id == id)
        {
            if self.ctx_owner != Some(e.kind) {
                e.tab.context_changed();
                self.ctx_owner = Some(e.kind);
            }
            e.tab.logic(ctx, frame);
        }
        if let View::Tab(id) = self.view
            && let Some(e) = self.entries.iter_mut().find(|e| e.id == id)
        {
            let app = ai_app(e.kind);
            let mut host = TabHost { tab: &mut e.tab, client: &mut e.pdf_client, ctx };
            e.chat.poll(&mut host, app, &self.ai_cfg, ctx);
        }
        if let Some((prompt, started)) = self.ai_selftest.clone() {
            self.drive_ai_selftest(ctx, prompt, started);
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // Files of a type the suite opens get their own tab, whichever tab is showing; anything
        // else (a picture dropped on a document) stays for the active app.
        let mut keep = Vec::new();
        for f in std::mem::take(&mut raw.dropped_files) {
            match f.path().to_str().filter(|p| kind_for(p).is_some()) {
                Some(p) => self.pending.push(p.to_string()),
                None => keep.push(f),
            }
        }
        raw.dropped_files = keep;
        if !self.pending.is_empty() {
            ctx.request_repaint();
        }
        if let View::Tab(id) = self.view
            && let Some(e) = self.entry_mut(id)
        {
            e.tab.raw_input_hook(ctx, raw);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let mut select: Option<View> = None;
        let mut close: Option<u64> = None;
        let mut new: Option<Kind> = None;
        let mut toggle_chat: Option<u64> = None;
        egui::Panel::top("officecraft_tabs").exact_size(32.0).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                if ui.selectable_label(self.view == View::Home, "Home").clicked() {
                    select = Some(View::Home);
                }
                for e in &self.entries {
                    let title = if e.tab.is_dirty() { format!("{} •", e.name) } else { e.name.clone() };
                    badge(ui, Some(e.kind), 16.0);
                    let shown = title;
                    if ui.selectable_label(self.view == View::Tab(e.id), shown).on_hover_text(e.path.as_deref().unwrap_or("Not saved yet")).clicked() {
                        select = Some(View::Tab(e.id));
                    }
                    if ui.small_button("×").on_hover_text("Close tab").clicked() {
                        close = Some(e.id);
                    }
                    ui.separator();
                }
                ui.menu_button("+", |ui| {
                    for kind in Kind::ALL {
                        if ui.button(format!("New {}", kind.label())).clicked() {
                            new = Some(kind);
                            ui.close();
                        }
                    }
                });
                if let View::Tab(id) = self.view {
                    let open = self.entries.iter().find(|e| e.id == id).is_some_and(|e| e.chat.open);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.selectable_label(open, "✨ Assistant").clicked() {
                            toggle_chat = Some(id);
                        }
                    });
                }
            });
        });
        if let Some(id) = toggle_chat
            && let Some(e) = self.entry_mut(id)
        {
            e.chat.open = !e.chat.open;
        }
        if let Some(v) = select {
            self.view = v;
        }
        if let Some(k) = new {
            self.new_tab(k);
        }
        if let Some(id) = close {
            self.request_close(id);
        }
        if let Some(id) = self.confirm_close {
            let name = self.entries.iter().find(|e| e.id == id).map(|e| e.name.clone()).unwrap_or_default();
            let mut discard = false;
            let mut cancel = false;
            egui::Window::new("Close without saving?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
                ui.label(format!("{name} has unsaved changes. Go back and save it first, or close it and lose them."));
                ui.horizontal(|ui| {
                    if ui.button("Go back").clicked() {
                        cancel = true;
                    }
                    if ui.button("Close and discard").clicked() {
                        discard = true;
                    }
                });
            });
            if cancel {
                self.confirm_close = None;
                self.view = View::Tab(id);
            }
            if discard {
                self.close(id);
            }
        }
        if let View::Tab(id) = self.view
            && let Some(e) = self.entries.iter_mut().find(|e| e.id == id)
            && e.chat.open
        {
            let app = ai_app(e.kind);
            let mut submit = false;
            egui::Panel::right("officecraft_ai").default_size(380.0).min_size(280.0).resizable(true).show(ui, |ui| {
                submit = e.chat.ui(ui, app, &self.ai_cfg);
            });
            if submit {
                e.chat.send(app, &self.ai_cfg, ui.ctx());
            }
        }
        match self.view {
            View::Home => self.home(ui),
            View::Tab(id) => {
                if let Some(e) = self.entry_mut(id) {
                    e.tab.ui(ui, frame);
                } else {
                    self.view = View::Home;
                }
            }
        }
        // While a file is dragged over the window, say where it will go.
        let hovering: Vec<String> = ui.ctx().input(|i| i.raw.hovered_files.iter().filter_map(|f| f.path.as_ref().map(|p| p.to_string_lossy().to_string())).collect());
        if !hovering.is_empty() {
            let screen = ui.ctx().content_rect();
            let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("officecraft_drop")));
            painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
            let n = hovering.len();
            let text = if n == 1 {
                match kind_for(&hovering[0]) {
                    Some(k) => format!("Drop to open in {}", k.label()),
                    None => "This file type is not supported here".to_string(),
                }
            } else {
                format!("Drop to open {n} files")
            };
            painter.text(screen.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(28.0), egui::Color32::WHITE);
        }
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("OfficeCraft")
            .with_app_id("ai.storyteller.officecraft")
            .with_inner_size([1440.0, 920.0])
            .with_min_inner_size([760.0, 480.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    let mut suite = Suite::new();
    if let Ok(prompt) = std::env::var("OFFICECRAFT_SELFTEST_AI") {
        suite.ai_selftest = Some((prompt, None));
    }
    if std::env::var_os("OFFICECRAFT_SELFTEST").is_some() {
        suite.selftest = Some(0);
    }
    let selftest = suite.selftest.is_some() || suite.ai_selftest.is_some();
    if selftest {
        suite.persist = false;
    } else {
        let (files, home) = load_session();
        suite.pending = files;
        suite.restore_home = home;
    }
    // Files named on the command line each open in their own tab, after the restored ones, and
    // stay in front.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        suite.restore_home = false;
        suite.pending.extend(args);
    }
    eframe::run_native("OfficeCraft", options, Box::new(move |_cc| Ok(Box::new(suite))))
}
