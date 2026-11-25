//! OfficeCraft: WordCraft, GridCraft, DeckCraft and PdfCraft as tabs in one
//! window.
//!
//! Each tab is the real app's UI struct, created the first time its tab is opened. Only the
//! active tab sees input and runs its frame logic, so shortcuts never leak between apps.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use eframe::egui;
#[cfg(target_arch = "wasm32")]
use web::{deck_services, grid_services, word_services};

/// Files read asynchronously in a browser page (picker, drag and drop): (name, bytes).
type Inbox = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<u8>)>>>;

mod ai;
mod convert;
mod equation;
mod graphpic;
mod platform;
#[cfg(not(target_arch = "wasm32"))]
mod updater;
#[cfg(not(target_arch = "wasm32"))]
mod clip;
#[cfg(not(target_arch = "wasm32"))]
mod files;
#[cfg(not(target_arch = "wasm32"))]
mod mcp;
#[cfg(target_arch = "wasm32")]
mod host;
#[cfg(target_arch = "wasm32")]
mod web;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Word,
    Grid,
    Deck,
    Pdf,
    Graph,
    /// equaSHion: the equation editor with Copy as LaTeX, MathML, OMML, text, image and SVG.
    Equation,
    /// verSHion: question banks, exam layout, and printable exports.
    Vershion,
}

impl Kind {
    const ALL: [Kind; 7] = [Kind::Word, Kind::Grid, Kind::Deck, Kind::Pdf, Kind::Graph, Kind::Equation, Kind::Vershion];
    /// Apps that bring their own fonts and layout (never tiled beside another app).
    fn tileable(self) -> bool {
        !matches!(self, Kind::Graph | Kind::Equation | Kind::Vershion)
    }
    /// Whether the suite's assistant can work in this app.
    fn has_assistant(self) -> bool {
        !matches!(self, Kind::Equation | Kind::Vershion)
    }
    fn label(self) -> &'static str {
        match self {
            Kind::Word => "Docs",
            Kind::Grid => "Sheets",
            Kind::Deck => "Slides",
            Kind::Pdf => "PDF",
            Kind::Graph => "Graph",
            Kind::Equation => "Equations",
            Kind::Vershion => "verSHion",
        }
    }
}

/// One live app. Boxed so the tab strip does not care which it is.
enum Tab {
    Word(Box<wordcraft_ui_egui::WordApp>),
    Grid(Box<gridcraft_ui_egui::SheetApp>),
    Deck(Box<deckcraft_ui_egui::SlideApp>),
    Pdf(Box<pdfcraft_ui_egui::PdfCraftApp>),
    Graph(Box<vishual_ui_egui::VishualApp>),
    Equation(Box<equation::EquationApp>),
    Vershion(Box<vershion::App>),
}

/// File dialogs for the Graph tab and where its gallery is kept (`<config>/officecraft/`).
fn graph_services() -> vishual_ui_egui::Services {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|d| d.join("officecraft"));
    vishual_ui_egui::Services {
        #[cfg(not(target_arch = "wasm32"))]
        pick_open: Some(Box::new(|_purpose: &str| rfd::FileDialog::new().add_filter("viSHual", &["json", "vshl"]).pick_file().map(|p| p.to_string_lossy().to_string()))),
        #[cfg(not(target_arch = "wasm32"))]
        pick_save: Some(Box::new(|name: &str| rfd::FileDialog::new().set_file_name(name).save_file().map(|p| p.to_string_lossy().to_string()))),
        #[cfg(target_arch = "wasm32")]
        pick_open: None,
        #[cfg(target_arch = "wasm32")]
        pick_save: None,
        gallery_store: dir.map(|d| d.join("vishual-gallery.json")),
        // links open in the system browser (the desktop default) or a new tab (web)
        ..Default::default()
    }
}

/// Screenshot hook: `OFFICECRAFT_SHOT_VISHUAL=chat,dark,seed,rows`.
fn graph_shot_options(g: &mut vishual_ui_egui::VishualApp, spec: &str) {
    for part in spec.split(',') {
        match part.trim() {
            "chat" => g.show_chat(),
            "dark" => g.set_dark(true),
            "seed" => g.seed_demo(),
            "rows" => g.seed_rows(),
            _ => {}
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn word_services() -> wordcraft_ui_egui::Services {
    wordcraft_ui_egui::Services {
        pick_open: Some(Box::new(|purpose: &str| {
            let d = rfd::FileDialog::new();
            let d = if purpose == "picture" {
                d.add_filter("Pictures", &["png", "jpg", "jpeg", "gif", "webp", "bmp"])
            } else {
                d.add_filter("WordCraft", &["docx", "docm", "dotx", "odt", "rtf", "txt", "md", "html", "htm", "json"])
            };
            d.pick_file().map(|p| p.to_string_lossy().to_string())
        })),
        pick_save: Some(Box::new(|name: &str| rfd::FileDialog::new().set_file_name(name).save_file().map(|p| p.to_string_lossy().to_string()))),
        // Formatted copy and paste across the tabs (a Sheets range pastes as a table).
        clipboard_text: Some(Box::new(clip::text)),
        clipboard_html: Some(Box::new(clip::html)),
        // (text, html, rtf, mathml): the shell's clipboard carries the HTML beside the text; a copy
        // without HTML goes through egui as text.
        set_clipboard_rich: Some(Box::new(|text: &str, html: &str, _rtf: &str, _mathml: &str| !html.is_empty() && clip::set_html(html, text))),
        ..Default::default()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn grid_services() -> gridcraft_ui_egui::Services {
    gridcraft_ui_egui::Services {
        pick_open: Some(Box::new(|| {
            rfd::FileDialog::new()
                .add_filter("GridCraft", &["xlsx", "xlsm", "csv", "tsv", "txt", "json"])
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
        copy_html: Some(Box::new(|html: &str, text: &str| if clip::set_html(html, text) { Ok(()) } else { Err("no HTML clipboard".into()) })),
        paste_html: Some(Box::new(clip::html)),
        ..Default::default()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn deck_services() -> deckcraft_ui_egui::Services {
    deckcraft_ui_egui::Services {
        pick_open: Some(Box::new(|_purpose: &str| {
            rfd::FileDialog::new()
                .add_filter("DeckCraft", &["deckcraft", "pptx", "potx", "ppsx"])
                .pick_file()
                .map(|p| p.to_string_lossy().to_string())
        })),
        pick_save: Some(Box::new(|name: &str| rfd::FileDialog::new().set_file_name(name).save_file().map(|p| p.to_string_lossy().to_string()))),
        read: Some(Box::new(|p: &str| std::fs::read(p).map_err(|e| e.to_string()))),
        write: Some(Box::new(|p: &str, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string()))),
        clipboard_html: Some(Box::new(clip::html)),
        clipboard_set_html: Some(Box::new(clip::set_html)),
        ..Default::default()
    }
}

/// Where unsaved documents are copied while they change, so a crash or a discarded quit can be
/// undone on the next launch: `<data dir>/recovery/{word,deck}`, and the context that wakes the
/// window when a copy is written. Set once at launch; unset in a page, in self-tests and with
/// `OFFICECRAFT_NO_RECOVERY`.
#[cfg(not(target_arch = "wasm32"))]
static RECOVERY: std::sync::OnceLock<(std::path::PathBuf, egui::Context)> = std::sync::OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
fn recovery_dir(app: &str) -> Option<(std::path::PathBuf, egui::Context)> {
    RECOVERY.get().map(|(d, ctx)| (d.join(app), ctx.clone()))
}

/// A DeckCraft session that keeps AutoRecover copies of its unsaved presentations.
fn deck_session(#[allow(unused_mut)] mut session: deckcraft_engine::Session) -> deckcraft_engine::Session {
    #[cfg(not(target_arch = "wasm32"))]
    {
        session.recovery_dir = recovery_dir("deck").map(|(d, _)| d);
    }
    session
}

impl Tab {
    fn create(kind: Kind) -> Tab {
        match kind {
            Kind::Word => {
                let doc = wordcraft_doc::Document::new();
                #[allow(unused_mut)]
                let mut session = wordcraft_engine::Session::new(doc);
                #[cfg(not(target_arch = "wasm32"))]
                if let Some((dir, ctx)) = recovery_dir("word")
                    && let Err(e) = session.enable_recovery(dir, std::sync::Arc::new(move || ctx.request_repaint()))
                {
                    log::warn!("Docs recovery unavailable: {e}");
                }
                Tab::Word(Box::new(wordcraft_ui_egui::WordApp::new(session, word_services())))
            }
            Kind::Grid => {
                let mut session = gridcraft_engine::Session::new();
                session.new_workbook();
                Tab::Grid(Box::new(gridcraft_ui_egui::SheetApp::new(session, grid_services())))
            }
            Kind::Deck => Tab::Deck(Box::new(deckcraft_ui_egui::SlideApp::new(deck_session(deckcraft_engine::Session::new()), deck_services()))),
            Kind::Pdf => {
                #[allow(unused_mut)]
                let mut app = pdfcraft_ui_egui::PdfCraftApp::new();
                // Inside a host page, Save hands the PDF to the page instead of downloading it.
                #[cfg(target_arch = "wasm32")]
                if host::active() {
                    app.host_save = Some(Box::new(host::save));
                }
                Tab::Pdf(Box::new(app))
            }
            Kind::Graph => Tab::Graph(Box::new(vishual_ui_egui::VishualApp::new(graph_services()))),
            Kind::Equation => Tab::Equation(Box::new(equation::EquationApp::new())),
            // The app installs its own fonts when the tab is activated (`context_changed`).
            Kind::Vershion => Tab::Vershion(Box::new(vershion::App::new_from(&egui::Context::default()))),
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        match self {
            Tab::Word(a) => a.logic(ctx),
            Tab::Grid(a) => a.logic(ctx),
            Tab::Deck(a) => a.logic(ctx),
            Tab::Pdf(a) => eframe::App::logic(a.as_mut(), ctx, frame),
            Tab::Graph(_) | Tab::Equation(_) | Tab::Vershion(_) => {}
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        match self {
            Tab::Word(a) => a.raw_input_hook(ctx, raw),
            Tab::Grid(a) => a.raw_input_hook(ctx, raw),
            Tab::Deck(a) => a.raw_input_hook(ctx, raw),
            Tab::Pdf(a) => eframe::App::raw_input_hook(a.as_mut(), ctx, raw),
            Tab::Graph(_) | Tab::Equation(_) => {}
            Tab::Vershion(a) => a.raw_input_hook(ctx, raw),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        match self {
            Tab::Word(a) => a.ui(ui),
            Tab::Grid(a) => a.ui(ui),
            Tab::Deck(a) => a.ui(ui),
            Tab::Pdf(a) => eframe::App::ui(a.as_mut(), ui, frame),
            Tab::Graph(a) => a.ui(ui),
            Tab::Equation(a) => a.ui(ui),
            Tab::Vershion(a) => a.ui(ui),
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
/// The Home empty-state line in the browser build.
#[cfg(target_arch = "wasm32")]
fn web_open_hint() -> &'static str {
    if web::can_write_in_place() {
        "Press Open file, or add a folder in the sidebar. Save writes back to the file you opened; files dropped on the page download a copy instead."
    } else {
        "Press Open file, add a folder in the sidebar, or drop a file on the page. Files stay in your browser; Save downloads a copy."
    }
}
#[cfg(not(target_arch = "wasm32"))]
fn web_open_hint() -> &'static str {
    ""
}

/// Whether the browser build can show the folder sidebar.
#[cfg(target_arch = "wasm32")]
fn web_can_browse() -> bool {
    web::can_browse_folders()
}
#[cfg(not(target_arch = "wasm32"))]
fn web_can_browse() -> bool {
    false
}

fn kind_for(path: &str) -> Option<Kind> {
    // verSHion claims compound suffixes before the final-extension match (and never claims
    // generic .json/.zip/.csv, which other tabs own).
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".parrot.json") || lower.ends_with(".ditto.json") || lower.ends_with(".question-bank.json") {
        return Some(Kind::Vershion);
    }
    let ext = std::path::Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "docx" | "docm" | "dotx" | "odt" | "rtf" | "txt" | "md" | "html" | "htm" => Some(Kind::Word),
        "xlsx" | "xlsm" | "csv" | "tsv" => Some(Kind::Grid),
        "pptx" | "potx" | "ppsx" | "deckcraft" => Some(Kind::Deck),
        "pdf" => Some(Kind::Pdf),
        "vshl" | "vishual" => Some(Kind::Graph),
        "gift" | "qti" => Some(Kind::Vershion),
        _ => None,
    }
}

const OPEN_EXTS: &[&str] = &[
    "docx", "docm", "dotx", "odt", "rtf", "txt", "md", "html", "htm", "xlsx", "xlsm", "csv", "tsv", "pptx", "potx", "ppsx", "deckcraft", "pdf", "vshl", "vishual",
    "gift", "qti",
];
const MAX_RECENT: usize = 12;

fn load_recent() -> Vec<String> {
    platform::read_lines("recent.txt")
}

fn save_recent(list: &[String]) {
    platform::write_lines("recent.txt", list);
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
            Tab::Equation(_) => false,
            // The store saves on every mutation; there is no separate dirty flag.
            Tab::Vershion(_) => false,
        }
    }

    /// The file the app now saves to, which Save As changes. `None` when the app has no file or
    /// does not report one (PDF keeps its own per-document paths).
    fn current_path(&self) -> Option<String> {
        match self {
            Tab::Word(a) => a.session.path.as_ref().map(|p| p.to_string_lossy().to_string()),
            Tab::Grid(a) => a.session.active().and_then(|d| d.path.clone()),
            Tab::Deck(a) => a.session.active().and_then(|d| d.path.clone()),
            _ => None,
        }
    }

    /// Points the app at `path` after its file was renamed or moved on Home, so the next Save
    /// writes there. `false`: this app can't be re-pointed (close the tab first).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn set_path(&mut self, path: &str) -> bool {
        match self {
            Tab::Word(a) => {
                a.session.path = Some(std::path::PathBuf::from(path));
                true
            }
            Tab::Grid(a) => a.session.active_mut().map(|d| d.path = Some(path.to_string())).is_some(),
            Tab::Deck(a) => a.session.active_mut().map(|d| d.path = Some(path.to_string())).is_some(),
            _ => false,
        }
    }

    /// The apps share one egui context and each installs its own fonts and visuals into it.
    /// After another app has drawn, this one must install them again.
    fn context_changed(&mut self, ctx: &egui::Context) {
        match self {
            Tab::Graph(a) => a.context_changed(ctx),
            Tab::Equation(a) => a.context_changed(ctx),
            Tab::Vershion(a) => a.context_changed(ctx),
            // The official releases keep their font state private: the shell puts their fonts
            // and style back itself (`Look`).
            Tab::Word(_) | Tab::Deck(_) | Tab::Grid(_) | Tab::Pdf(_) => {}
        }
    }

    /// Web: hands a picked or dropped file's bytes to the app, which opens them on its next frame.
    #[cfg(target_arch = "wasm32")]
    fn deliver(&mut self, name: &str, bytes: Vec<u8>) {
        let push = |inbox: Option<&Inbox>| {
            if let Some(i) = inbox {
                i.lock().unwrap_or_else(|e| e.into_inner()).push((name.to_string(), bytes.clone()));
            }
        };
        match self {
            Tab::Word(a) => push(a.services.inbox.as_ref()),
            Tab::Grid(a) => push(a.services.inbox.as_ref()),
            // DeckCraft numbers its file requests: a file handed over here is a new one.
            Tab::Deck(a) => a.open_or_insert_bytes(name, &bytes),
            Tab::Pdf(a) => push(Some(&a.startup_inbox)),
            Tab::Graph(_) | Tab::Equation(_) => {}
            Tab::Vershion(a) => a.open_import_bytes(name, bytes.clone()),
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
            #[cfg(not(target_arch = "wasm32"))]
            Tab::Pdf(a) => {
                a.open_path(path);
                Ok(())
            }
            // A page has no paths; files arrive through `deliver`.
            #[cfg(target_arch = "wasm32")]
            Tab::Pdf(_) => Err("opening by path is not available in the browser".into()),
            Tab::Graph(a) => a.open_path(path),
            Tab::Equation(_) => Err("equations are not files".into()),
            Tab::Vershion(a) => {
                a.open_path(std::path::Path::new(path));
                Ok(())
            }
        }
    }
}

/// One open window of an app. Several of the same kind can be open at once.
/// An app's fonts and style as it installed them into the shared egui context, kept so they can
/// be put back when the app comes to the front again.
struct Look {
    fonts: egui::FontDefinitions,
    light: std::sync::Arc<egui::Style>,
    dark: std::sync::Arc<egui::Style>,
}

impl Look {
    fn capture(ctx: &egui::Context) -> Look {
        Look { fonts: ctx.fonts(|f| f.definitions().clone()), light: ctx.style_of(egui::Theme::Light), dark: ctx.style_of(egui::Theme::Dark) }
    }

    /// Fonts activate on the next pass but a style applies at once, and a style naming a font
    /// family the active set lacks panics wherever text is drawn. So the fonts go first, under a
    /// plain style that only uses the built-in families; `apply_style` follows a frame later.
    fn apply_fonts(&self, ctx: &egui::Context) {
        ctx.set_fonts(self.fonts.clone());
        let plain = std::sync::Arc::new(egui::Style::default());
        ctx.set_style_of(egui::Theme::Light, plain.clone());
        ctx.set_style_of(egui::Theme::Dark, plain);
    }

    fn apply_style(&self, ctx: &egui::Context) {
        ctx.set_style_of(egui::Theme::Light, self.light.clone());
        ctx.set_style_of(egui::Theme::Dark, self.dark.clone());
    }
}

/// What happens when a second document is opened: ask, tile straight away, or keep tabs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TilePref {
    Ask,
    Always,
    Never,
}

impl TilePref {
    fn load() -> Self {
        match platform::read("tiling.txt").as_deref().map(str::trim) {
            Some("always") => TilePref::Always,
            Some("never") => TilePref::Never,
            _ => TilePref::Ask,
        }
    }

    fn save(self) {
        let text = match self {
            TilePref::Ask => "ask",
            TilePref::Always => "always",
            TilePref::Never => "never",
        };
        let _ = platform::write("tiling.txt", text);
    }
}

/// Most tabs shown side by side.
/// One column of the tiled view: its share of the width and the tiles stacked in it with their
/// share of the height.
#[derive(Clone)]
struct TileCol {
    w: f32,
    rows: Vec<(u64, f32)>,
}

/// Where tab `dragged` goes when dropped at `x` on the tab bar: its index among the tabs, counted
/// before it is taken out (`None` with no other tabs).
fn tab_insert_index(rects: &[(u64, egui::Rect)], dragged: u64, x: f32) -> Option<usize> {
    let mut seen_other = false;
    let mut index = 0;
    let mut before_others = 0;
    for (k, (id, r)) in rects.iter().enumerate() {
        if *id == dragged {
            index = k;
            continue;
        }
        seen_other = true;
        if r.center().x < x {
            before_others += 1;
        }
    }
    if !seen_other {
        return None;
    }
    // `before_others` counts the other tabs left of the drop; in the full list (dragged tab
    // still in it) that is the same index if the tab came from the right, one more if from the left.
    Some(if before_others >= index { before_others + 1 } else { before_others })
}

/// Where a dragged tile lands relative to the tile under the pointer.
#[derive(Clone, Copy, PartialEq)]
enum DropZone {
    Swap,
    Above,
    Below,
    ColBefore,
    ColAfter,
}

/// Columns the window has room for: two on an ordinary screen, up to four on a super wide one.
fn max_tile_cols(width: f32) -> usize {
    ((width / 860.0).floor() as usize).clamp(2, 4)
}

fn equalize_rows(c: &mut TileCol) {
    let n = c.rows.len().max(1) as f32;
    for r in &mut c.rows {
        r.1 = 1.0 / n;
    }
}

fn renorm_rows(c: &mut TileCol) {
    let sum: f32 = c.rows.iter().map(|r| r.1).sum();
    if sum <= 0.01 {
        equalize_rows(c);
    } else {
        for r in &mut c.rows {
            r.1 /= sum;
        }
    }
}

fn equalize_cols(l: &mut [TileCol]) {
    let n = l.len().max(1) as f32;
    for c in l {
        c.w = 1.0 / n;
    }
}

fn renorm_cols(l: &mut [TileCol]) {
    let sum: f32 = l.iter().map(|c| c.w).sum();
    if sum <= 0.01 {
        equalize_cols(l);
    } else {
        for c in l {
            c.w /= sum;
        }
    }
}

/// The tile under `p` (other than the dragged one) and which part of it `p` is in.
fn drop_target(rects: &[(u64, egui::Rect)], dragged: u64, p: egui::Pos2, room_for_column: bool) -> Option<(u64, DropZone, egui::Rect)> {
    let (id, r) = rects.iter().find(|(id, r)| *id != dragged && r.contains(p))?;
    let (u, v) = ((p.x - r.min.x) / r.width().max(1.0), (p.y - r.min.y) / r.height().max(1.0));
    let zone = if room_for_column && u < 0.22 {
        DropZone::ColBefore
    } else if room_for_column && u > 0.78 {
        DropZone::ColAfter
    } else if v < 0.28 {
        DropZone::Above
    } else if v > 0.72 {
        DropZone::Below
    } else {
        DropZone::Swap
    };
    Some((*id, zone, *r))
}

fn drop_hint_rect(r: egui::Rect, zone: DropZone) -> egui::Rect {
    let (w, h) = (r.width(), r.height());
    match zone {
        DropZone::ColBefore => egui::Rect::from_min_size(r.min, egui::vec2(w * 0.22, h)),
        DropZone::ColAfter => egui::Rect::from_min_size(egui::pos2(r.max.x - w * 0.22, r.min.y), egui::vec2(w * 0.22, h)),
        DropZone::Above => egui::Rect::from_min_size(r.min, egui::vec2(w, h * 0.28)),
        DropZone::Below => egui::Rect::from_min_size(egui::pos2(r.min.x, r.max.y - h * 0.28), egui::vec2(w, h * 0.28)),
        DropZone::Swap => r,
    }
}

const MAX_TILES: usize = 4;
/// Frames an app has to run alone, installing its fonts and style, before the next one starts.
const TILE_WARM_FRAMES: u8 = 3;

/// Adds to `into` every font and family `from` has that it lacks; true if anything was added.
/// Apps replace the whole font set when they install, so tiled apps keep a union alive.
fn merge_fonts(into: &mut egui::FontDefinitions, from: &egui::FontDefinitions) -> bool {
    let mut changed = false;
    for (name, data) in &from.font_data {
        if !into.font_data.contains_key(name) {
            into.font_data.insert(name.clone(), data.clone());
            changed = true;
        }
    }
    for (family, names) in &from.families {
        let have = into.families.entry(family.clone()).or_insert_with(|| {
            changed = true;
            Vec::new()
        });
        for n in names {
            if !have.contains(n) {
                have.push(n.clone());
                changed = true;
            }
        }
    }
    changed
}

/// Whether `have` binds every font and family `want` does, so any tiled app's text can be laid out.
fn fonts_cover(have: &egui::FontDefinitions, want: &egui::FontDefinitions) -> bool {
    want.font_data.keys().all(|k| have.font_data.contains_key(k))
        && want.families.iter().all(|(fam, names)| have.families.get(fam).is_some_and(|h| names.iter().all(|n| h.contains(n))))
}

/// Runs `f`, with the frame's key, text and clipboard events held back when `withhold` (so a
/// tile that is not in front does not act on the front tile's typing and shortcuts).
fn with_events_withheld<R>(ctx: &egui::Context, withhold: bool, f: impl FnOnce() -> R) -> R {
    if !withhold {
        return f();
    }
    let held = ctx.input_mut(|i| std::mem::take(&mut i.events));
    let out = f();
    ctx.input_mut(|i| {
        let mut all = held;
        all.append(&mut i.events);
        i.events = all;
    });
    out
}

/// The selftest's stand-in for a double-click on the selected picture.
static FORCE_PICTURE_EDIT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The encoded bytes of the picture selected in a document (the fork had this in the engine).
fn selected_picture_bytes(s: &wordcraft_engine::Session) -> Option<std::sync::Arc<Vec<u8>>> {
    let (_, o) = wordcraft_engine::cmd::objects::selected(s)?;
    let wordcraft_doc::para::InlineObject::Image { media, .. } = o else { return None };
    s.doc.media.get(&media).cloned()
}

/// A picture double-clicked in a document this frame, and its bytes (the fork's
/// `WordApp::take_picture_edit`): the primary button's double-click with a picture selected.
fn take_picture_edit(a: &wordcraft_ui_egui::WordApp, ctx: &egui::Context) -> Option<std::sync::Arc<Vec<u8>>> {
    let forced = FORCE_PICTURE_EDIT.swap(false, std::sync::atomic::Ordering::Relaxed);
    let dbl = ctx.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary));
    if !(forced || dbl) {
        return None;
    }
    selected_picture_bytes(&a.session)
}

/// Apps whose own `context_changed` is not available to the shell; it restores their `Look`.
fn restores_look(kind: Kind) -> bool {
    matches!(kind, Kind::Word | Kind::Deck | Kind::Grid | Kind::Pdf)
}

/// Where Send to… puts a document.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SendTo {
    /// A new tab of this app.
    New(Kind),
    /// This open tab, at its cursor (Docs) or on a new slide (Slides).
    Into(u64),
    /// A PDF the user names, opened in a PDF tab.
    Pdf,
}

struct Entry {
    id: u64,
    kind: Kind,
    name: String,
    path: Option<String>,
    tab: Tab,
    chat: ai::Chat,
    /// A Graph tab opened from a graph picture in a document: the document's tab id.
    graph_for: Option<u64>,
    /// PDF is driven over its control channel; the client is made the first time it is needed.
    pdf_client: Option<pdfcraft_ui_egui::control::ControlClient>,
    /// The path the app reported last frame, as it spelled it (`sync_paths`).
    seen_path: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Only(Kind),
}

/// A folder listing the browser delivered: a folder the visitor just added (`added`) or a
/// fresh listing of one already in the sidebar.
/// (Each build reads only some of the fields.)
#[allow(dead_code)]
struct FolderScan {
    key: String,
    rows: Vec<Row>,
    added: bool,
    /// Desktop: which request this answers (older answers for the same folder are dropped).
    seq: u64,
    /// Desktop: the folder could not be read.
    error: Option<String>,
}
type FolderScans = std::sync::Arc<std::sync::Mutex<Vec<FolderScan>>>;

/// One file in the Home table.
#[derive(Clone)]
struct Row {
    path: String,
    name: String,
    folder: String,
    kind: Option<Kind>,
    modified: Option<std::time::SystemTime>,
    size: u64,
    /// A subfolder (desktop folder browsing): clicking it goes in.
    is_dir: bool,
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
        size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
        is_dir: meta.is_some_and(|m| m.is_dir()),
    }
}

/// `path` resolved to its one canonical spelling (symlinks, `.` and `..` removed). Unchanged in a
/// browser page, or when the file cannot be resolved.
fn canonical_path(path: &str) -> String {
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(p) = dunce::canonicalize(path)
        && let Some(s) = p.to_str()
    {
        return s.to_string();
    }
    path.to_string()
}

fn file_name_of(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
}

/// `n` and its noun: "1 slide", "3 slides".
fn plural(n: u64, noun: &str) -> String {
    if n == 1 { format!("1 {noun}") } else { format!("{n} {noun}s") }
}

/// One level of folder `dir` (desktop): its subfolders, then the files a Craft app opens,
/// newest first. Read on a worker thread: a NAS or network folder can be slow.
#[cfg(not(target_arch = "wasm32"))]
fn list_level(dir: &str) -> Result<Vec<Row>, String> {
    files::list_dir(std::path::Path::new(dir), |p| kind_for(p).is_some()).map(|v| v.into_iter().filter_map(|p| p.to_str().map(row_for)).collect())
}

/// Folders the visitor added. A page can't reopen a folder without a fresh pick, so the browser
/// build starts with none and keeps them for the session only.
fn load_folders() -> Vec<String> {
    if cfg!(target_arch = "wasm32") {
        return Vec::new();
    }
    // Not resolved here: a folder on a share that is down would block the launch. Folders are
    // stored resolved when added, and open-tab lookups resolve the path they are given.
    platform::read_lines("folders.txt")
}

fn save_folders(list: &[String]) {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    platform::write_lines("folders.txt", list);
}

/// The last listing of folder `dir` held for Home (fresh ones arrive through `folder_scans`).
fn folder_listing(scans: &std::collections::HashMap<String, Vec<Row>>, dir: &str) -> Vec<Row> {
    scans.get(dir).cloned().unwrap_or_default()
}

fn kind_color(kind: Option<Kind>) -> egui::Color32 {
    match kind {
        Some(Kind::Word) => egui::Color32::from_rgb(47, 99, 198),
        Some(Kind::Grid) => egui::Color32::from_rgb(31, 143, 92),
        Some(Kind::Deck) => egui::Color32::from_rgb(212, 98, 46),
        Some(Kind::Pdf) => egui::Color32::from_rgb(194, 59, 69),
        Some(Kind::Graph) => egui::Color32::from_rgb(183, 121, 31),
        Some(Kind::Equation) => egui::Color32::from_rgb(98, 76, 168),
        Some(Kind::Vershion) => egui::Color32::from_rgb(47, 97, 68),
        None => egui::Color32::GRAY,
    }
}

/// The contact page. A page opens the copy next to it (so a Pages preview stays on the preview);
/// the desktop build opens the published site.
const CONTACT_URL: &str = if cfg!(target_arch = "wasm32") { "contact.html" } else { "https://officecraft.lefthanddev.com/contact.html" };

/// The one chrome accent on Home: selection, primary button, active toggle.
const ACCENT: egui::Color32 = egui::Color32::from_rgb(11, 122, 117);

/// Home's own look: the light, cool-grey surface with a white sidebar and one teal accent.
fn home_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::light();
    v.panel_fill = egui::Color32::from_rgb(237, 241, 245);
    v.window_fill = egui::Color32::WHITE;
    v.extreme_bg_color = egui::Color32::WHITE;
    let line = egui::Stroke::new(1.0, egui::Color32::from_rgb(207, 214, 223));
    v.widgets.noninteractive.bg_stroke = line;
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, INK);
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, INK);
    v.widgets.noninteractive.weak_bg_fill = egui::Color32::WHITE;
    v.widgets.inactive.bg_stroke = line;
    v.widgets.inactive.weak_bg_fill = egui::Color32::WHITE;
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
    v.hyperlink_color = ACCENT;
    let r = egui::CornerRadius::same(8);
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.corner_radius = r;
    v.widgets.open.corner_radius = r;
    v
}


const INK: egui::Color32 = egui::Color32::from_rgb(23, 32, 43);
const MUTED: egui::Color32 = egui::Color32::from_rgb(116, 128, 142);
const SOFT: egui::Color32 = egui::Color32::from_rgb(69, 81, 95);
const SEL_BG: egui::Color32 = egui::Color32::from_rgb(221, 241, 239);
const HAIRLINE: egui::Color32 = egui::Color32::from_rgb(238, 241, 244);
const LINE: egui::Color32 = egui::Color32::from_rgb(220, 226, 234);

/// egui applies new fonts on the frame after `set_fonts`, so the bold family is only named once
/// that frame has come, and never while another app's fonts are installed.
static BOLD_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn bold() -> egui::FontFamily {
    if BOLD_READY.load(std::sync::atomic::Ordering::Relaxed) {
        egui::FontFamily::Name("home_bold".into())
    } else {
        egui::FontFamily::Proportional
    }
}

/// The OfficeCraft mark (assets/officecraft-logo.svg) drawn with the painter, scaled from its 512-unit box to `rect`.
fn paint_logo(painter: &egui::Painter, rect: egui::Rect) {
    let k = rect.width() / 512.0;
    let at = |x: f32, y: f32| rect.min + egui::vec2(x, y) * k;
    let r = |x: f32, y: f32, w: f32, h: f32| egui::Rect::from_min_size(at(x, y), egui::vec2(w, h) * k);
    let white = egui::Color32::WHITE;
    let soft = egui::Color32::from_white_alpha(217);
    painter.rect_filled(r(16.0, 16.0, 480.0, 480.0), 112.0 * k, egui::Color32::from_rgb(0x2d, 0x4f, 0x6e));
    let tiles = [
        (91.0, 91.0, egui::Color32::from_rgb(0x3f, 0x5f, 0xe0)),
        (271.0, 91.0, egui::Color32::from_rgb(0xf0, 0x7a, 0x1e)),
        (91.0, 271.0, egui::Color32::from_rgb(0x1f, 0x9d, 0x55)),
        (271.0, 271.0, egui::Color32::from_rgb(0x12, 0xa5, 0x8a)),
    ];
    for (x, y, c) in tiles {
        painter.rect_filled(r(x, y, 150.0, 150.0), 36.0 * k, c);
    }
    // Glyphs: (tile origin, x, y, w, h, radius, colour), in tile units.
    let mut g: Vec<(f32, f32, f32, f32, f32, f32, f32, egui::Color32)> = Vec::new();
    // Docs: text lines.
    for (y, w, c) in [(34.0, 86.0, white), (56.0, 62.0, soft), (78.0, 74.0, soft), (100.0, 40.0, soft)] {
        g.push((91.0, 91.0, 32.0, y, w, 10.0, 5.0, c));
    }
    // Slides: title bar and three bars.
    g.push((271.0, 91.0, 34.0, 28.0, 50.0, 9.0, 4.5, white));
    g.push((271.0, 91.0, 34.0, 76.0, 26.0, 36.0, 6.0, soft));
    g.push((271.0, 91.0, 62.0, 50.0, 26.0, 62.0, 6.0, white));
    g.push((271.0, 91.0, 90.0, 64.0, 26.0, 48.0, 6.0, soft));
    // Sheets: 3x3 cells.
    for row in 0..3 {
        for col in 0..3 {
            let c = if row == 0 { white } else { egui::Color32::from_white_alpha(178) };
            g.push((91.0, 271.0, 32.0 + 32.0 * col as f32, 34.0 + 28.0 * row as f32, 26.0, 22.0, 5.0, c));
        }
    }
    for (ox, oy, x, y, w, h, rad, c) in g {
        painter.rect_filled(r(ox + x, oy + y, w, h), rad * k, c);
    }
    // PDF: page outline with a folded corner and two text lines.
    let stroke = |w: f32, c: egui::Color32| egui::Stroke::new(w * k, c);
    let p = |x: f32, y: f32| at(271.0 + x, 271.0 + y);
    painter.add(egui::Shape::closed_line(
        vec![p(46.0, 28.0), p(86.0, 28.0), p(106.0, 48.0), p(106.0, 122.0), p(46.0, 122.0)],
        stroke(8.0, white),
    ));
    painter.add(egui::Shape::line(vec![p(86.0, 28.0), p(86.0, 48.0), p(106.0, 48.0)], stroke(8.0, white)));
    painter.line_segment([p(60.0, 78.0), p(92.0, 78.0)], stroke(7.0, soft));
    painter.line_segment([p(60.0, 96.0), p(80.0, 96.0)], stroke(7.0, soft));
}

fn b(text: &str, size: f32) -> egui::RichText {
    egui::RichText::new(text).font(egui::FontId::new(size, bold()))
}

/// Noto Sans for Home when the system has it; the apps install their own fonts again on return.
fn install_home_fonts(ctx: &egui::Context) {
    let load = |names: &[&str]| names.iter().find_map(|p| std::fs::read(p).ok());
    let regular = load(&[
        "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/open-sans/OpenSans-Regular.ttf",
    ]);
    let semi = load(&[
        "/usr/share/fonts/google-noto/NotoSans-SemiBold.ttf",
        "/usr/share/fonts/noto/NotoSans-SemiBold.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf",
        "/usr/share/fonts/google-noto/NotoSans-Bold.ttf",
        "/usr/share/fonts/open-sans/OpenSans-Semibold.ttf",
    ]);
    let mut fonts = egui::FontDefinitions::default();
    if let Some(bytes) = regular {
        fonts.font_data.insert("home_regular".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
        if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            list.insert(0, "home_regular".into());
        }
    }
    let mut bold_list = fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
    if let Some(bytes) = semi {
        fonts.font_data.insert("home_semibold".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
        bold_list.insert(0, "home_semibold".into());
    }
    fonts.families.insert(bold(), bold_list);
    ctx.set_fonts(fonts);
}

fn kind_abbr(kind: Option<Kind>) -> &'static str {
    match kind {
        Some(Kind::Word) => "Dc",
        Some(Kind::Grid) => "Sh",
        Some(Kind::Deck) => "Sl",
        Some(Kind::Pdf) => "Pd",
        Some(Kind::Graph) => "Gr",
        Some(Kind::Equation) => "Eq",
        Some(Kind::Vershion) => "Vs",
        None => "?",
    }
}

fn paint_folder(p: &egui::Painter, c: egui::Pos2, col: egui::Color32) {
    let st = egui::Stroke::new(1.4, col);
    let r = egui::Rect::from_center_size(c + egui::vec2(0.0, 1.0), egui::vec2(14.0, 10.0));
    p.rect_stroke(r, 2.0, st, egui::StrokeKind::Inside);
    p.line_segment([r.left_top() + egui::vec2(1.0, -2.0), r.left_top() + egui::vec2(5.5, -2.0)], st);
}

/// The folder picture on a Home card.
fn paint_folder_big(p: &egui::Painter, c: egui::Pos2, col: egui::Color32) {
    let body = egui::Rect::from_center_size(c + egui::vec2(0.0, 4.0), egui::vec2(54.0, 38.0));
    let tab = egui::Rect::from_min_size(body.left_top() + egui::vec2(0.0, -7.0), egui::vec2(22.0, 9.0));
    p.rect_filled(tab, 3.0, col.gamma_multiply(0.55));
    p.rect_filled(body, 5.0, col.gamma_multiply(0.75));
}

fn paint_search(p: &egui::Painter, c: egui::Pos2, col: egui::Color32) {
    let st = egui::Stroke::new(1.6, col);
    p.circle_stroke(c + egui::vec2(-1.5, -1.5), 5.0, st);
    p.line_segment([c + egui::vec2(2.0, 2.0), c + egui::vec2(6.0, 6.0)], st);
}

/// One sidebar row: optional folder icon, label, count on the right (or a remove cross on hover).
fn side_row(ui: &mut egui::Ui, selected: bool, icon: bool, label: &str, count: Option<usize>, removable: bool) -> (egui::Response, bool) {
    // Draggable too: sidebar folders reorder by dragging (a click still selects).
    let sense = if removable { egui::Sense::click_and_drag() } else { egui::Sense::click() };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), sense);
    let hovered = resp.hovered();
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 8.0, SEL_BG);
    } else if hovered {
        p.rect_filled(rect, 8.0, egui::Color32::from_gray(244));
    }
    let ink = if selected { ACCENT } else { SOFT };
    let mut x = rect.left() + 10.0;
    if icon {
        paint_folder(p, egui::pos2(x + 8.0, rect.center().y), ink);
        x += 26.0;
    }
    let font = if selected { egui::FontId::new(14.0, bold()) } else { egui::FontId::proportional(14.0) };
    p.text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, label, font, ink);
    let mut removed = false;
    let mut over_x = false;
    if removable && hovered {
        let xr = egui::Rect::from_center_size(egui::pos2(rect.right() - 18.0, rect.center().y), egui::vec2(20.0, 20.0));
        p.text(xr.center(), egui::Align2::CENTER_CENTER, "×", egui::FontId::proportional(17.0), MUTED);
        removed = resp.clicked() && resp.interact_pointer_pos().is_some_and(|q| xr.expand(6.0).contains(q));
        over_x = resp.hover_pos().is_some_and(|q| xr.expand(6.0).contains(q));
    } else if let Some(n) = count {
        let col = if selected { ACCENT } else { MUTED };
        p.text(egui::pos2(rect.right() - 12.0, rect.center().y), egui::Align2::RIGHT_CENTER, n.to_string(), egui::FontId::proportional(13.0), col);
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    // The × only takes the folder off the sidebar; say so, since it looks like a delete.
    let resp = if over_x { resp.on_hover_text("Remove from sidebar. The folder and its files stay on disk.") } else { resp };
    (resp, removed)
}

/// What the Home rows asked for this frame, applied once drawing is done.
#[derive(Default)]
struct HomeActions {
    open: Option<String>,
    /// Go into this folder (desktop browsing).
    enter: Option<String>,
    op: Option<FileOp>,
    /// (item, folder): move the item into the folder.
    moves: Vec<(String, String)>,
    reveal: Option<String>,
    forget_recent: Option<String>,
}

/// Whether Home can change files on disk (rename, trash, move): the desktop build.
const FILE_OPS: bool = cfg!(not(target_arch = "wasm32"));

/// A Home row's clicks, right-click menu and drag: a click opens a file or goes into a folder; a
/// row can be dragged onto a folder (a folder row, the path above the list, a sidebar folder) to
/// move it there. `in_recent`: the row is in the Recent list, which can forget it.
fn row_interact(resp: &egui::Response, r: &Row, in_recent: bool, acts: &mut HomeActions) {
    if resp.clicked() {
        if r.is_dir {
            acts.enter = Some(r.path.clone());
        } else {
            acts.open = Some(r.path.clone());
        }
    }
    if FILE_OPS {
        resp.dnd_set_drag_payload(MovePayload(r.path.clone()));
        if r.is_dir
            && let Some(p) = dropped::<MovePayload>(&resp)
            && p.0 != r.path
        {
            acts.moves.push((p.0.clone(), r.path.clone()));
        }
    }
    resp.context_menu(|ui| {
        menu_visuals(ui);
        if ui.button(if r.is_dir { "Open folder" } else { "Open" }).clicked() {
            if r.is_dir {
                acts.enter = Some(r.path.clone());
            } else {
                acts.open = Some(r.path.clone());
            }
            ui.close();
        }
        if FILE_OPS {
            if ui.button("Rename…").clicked() {
                acts.op = Some(FileOp::Rename { path: r.path.clone(), name: r.name.clone(), error: None });
                ui.close();
            }
            if ui.button("Show in file manager").clicked() {
                acts.reveal = Some(r.path.clone());
                ui.close();
            }
        }
        if ui.button("Copy path").clicked() {
            ui.ctx().copy_text(r.path.clone());
            ui.close();
        }
        if in_recent && ui.button("Remove from Recent").on_hover_text("The file stays where it is").clicked() {
            acts.forget_recent = Some(r.path.clone());
            ui.close();
        }
        if FILE_OPS {
            ui.separator();
            if ui.button(egui::RichText::new("Move to Trash").color(egui::Color32::from_rgb(180, 40, 50))).clicked() {
                acts.op = Some(FileOp::Trash { path: r.path.clone(), open: Vec::new(), is_dir: r.is_dir });
                ui.close();
            }
        }
    });
}

/// Opens the system file manager at `path` (its folder, for a file).
#[cfg(not(target_arch = "wasm32"))]
fn reveal_in_file_manager(path: &str) {
    let p = std::path::Path::new(path);
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg("-R").arg(p).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer").arg(format!("/select,{path}")).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let r = std::process::Command::new("xdg-open").arg(if p.is_dir() { p } else { p.parent().unwrap_or(p) }).spawn();
    if let Err(e) = r {
        log::warn!("file manager: {e}");
    }
}

/// Home's look for a right-click menu: plain rows that light up on hover, no boxes.
fn menu_visuals(ui: &mut egui::Ui) {
    *ui.visuals_mut() = home_visuals();
    let w = &mut ui.visuals_mut().widgets;
    w.inactive.bg_stroke = egui::Stroke::NONE;
    w.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    w.hovered.bg_stroke = egui::Stroke::NONE;
    w.hovered.weak_bg_fill = SEL_BG;
    ui.spacing_mut().button_padding = egui::vec2(10.0, 5.0);
}

/// What was dropped on `resp` this frame, if it is a `P`. egui's own `dnd_release_payload` takes
/// the payload before checking its type, so asking for the wrong type would lose the drag.
fn dropped<P: std::any::Any + Send + Sync>(resp: &egui::Response) -> Option<std::sync::Arc<P>> {
    if egui::DragAndDrop::has_payload_of_type::<P>(&resp.ctx) { resp.dnd_release_payload::<P>() } else { None }
}

/// A drop target highlight: a teal outline while a dragged row is held over `rect`.
fn paint_drop_hint(ui: &egui::Ui, resp: &egui::Response, rect: egui::Rect) {
    if resp.dnd_hover_payload::<MovePayload>().is_some() {
        ui.painter().rect_stroke(rect, 8.0, egui::Stroke::new(2.0, ACCENT), egui::StrokeKind::Inside);
    }
}

/// A segment in the white filter bar: dark fill when selected.
fn pill(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let w = label.chars().count() as f32 * 7.8 + 28.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 30.0), egui::Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 8.0, INK);
    }
    let (font, col) = if selected { (egui::FontId::new(13.0, bold()), egui::Color32::WHITE) } else { (egui::FontId::proportional(13.0), SOFT) };
    p.text(rect.center(), egui::Align2::CENTER_CENTER, label, font, col);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn framed() -> egui::Frame {
    egui::Frame::new().fill(egui::Color32::WHITE).stroke(egui::Stroke::new(1.0, LINE)).corner_radius(10.0)
}

fn load_home_grid() -> bool {
    platform::read("homeview.txt").map(|s| s.trim() != "list").unwrap_or(true)
}

fn save_home_grid(grid: bool) {
    let _ = platform::write("homeview.txt", if grid { "grid" } else { "list" });
}

/// A small drawn preview of what the file is, centred in `area`. No file is opened to make it.
fn thumb(painter: &egui::Painter, kind: Option<Kind>, area: egui::Rect) {
    let c = kind_color(kind);
    let paper = egui::Color32::from_gray(250);
    let line = egui::Color32::from_gray(205);
    let r = |x: f32, y: f32, w: f32, h: f32, col: egui::Color32, round: f32, base: egui::Rect| {
        painter.rect_filled(egui::Rect::from_min_size(base.min + egui::vec2(x, y), egui::vec2(w, h)), round, col);
    };
    let center = area.center();
    match kind {
        Some(Kind::Word) | Some(Kind::Pdf) => {
            let page = egui::Rect::from_center_size(center, egui::vec2(58.0, 74.0));
            painter.rect_filled(page, 3.0, paper);
            if kind == Some(Kind::Pdf) {
                r(9.0, 10.0, 40.0, 3.0, line, 1.0, page);
                r(9.0, 22.0, 40.0, 22.0, egui::Color32::from_gray(232), 2.0, page);
                painter.text(page.left_bottom() + egui::vec2(9.0, -9.0), egui::Align2::LEFT_CENTER, "PDF", egui::FontId::proportional(9.0), c);
            } else {
                r(9.0, 10.0, 30.0, 5.0, c, 1.0, page);
                for (i, w) in [40.0, 40.0, 30.0, 40.0].into_iter().enumerate() {
                    r(9.0, 24.0 + i as f32 * 9.0, w, 3.0, line, 1.0, page);
                }
            }
        }
        Some(Kind::Grid) => {
            let sheet = egui::Rect::from_center_size(center, egui::vec2(96.0, 60.0));
            painter.rect_filled(sheet, 3.0, line);
            let (cw, ch) = (23.5, 14.0);
            for row in 0..4 {
                for col in 0..4 {
                    let fill = if row == 0 { c } else { paper };
                    r(0.5 + col as f32 * (cw + 0.5), 0.5 + row as f32 * (ch + 0.5), cw, ch, fill, 0.0, sheet);
                }
            }
        }
        Some(Kind::Deck) => {
            let slide = egui::Rect::from_center_size(center, egui::vec2(100.0, 62.0));
            painter.rect_filled(slide, 3.0, paper);
            r(12.0, 22.0, 62.0, 7.0, c, 2.0, slide);
            r(12.0, 36.0, 40.0, 3.0, line, 1.0, slide);
        }
        Some(Kind::Graph) => {
            let plot = egui::Rect::from_center_size(center, egui::vec2(100.0, 66.0));
            painter.rect_filled(plot, 3.0, paper);
            painter.hline(plot.x_range(), plot.center().y, egui::Stroke::new(1.0, line));
            painter.vline(plot.center().x, plot.y_range(), egui::Stroke::new(1.0, line));
            let pts: Vec<egui::Pos2> = (0..=24)
                .map(|i| {
                    let t = i as f32 / 24.0 * 2.0 - 1.0;
                    egui::pos2(plot.center().x + t * 42.0, plot.center().y + 26.0 - t * t * 46.0)
                })
                .collect();
            painter.add(egui::Shape::line(pts, egui::Stroke::new(2.5, c)));
        }
        Some(Kind::Equation) => {
            // A fraction: numerator, bar, denominator.
            let card = egui::Rect::from_center_size(center, egui::vec2(100.0, 62.0));
            painter.rect_filled(card, 3.0, paper);
            r(36.0, 14.0, 28.0, 6.0, c, 2.0, card);
            r(30.0, 29.0, 40.0, 3.0, line, 1.0, card);
            r(36.0, 40.0, 28.0, 6.0, c, 2.0, card);
        }
        Some(Kind::Vershion) => {
            // A numbered test page with an answer-key stripe.
            let page = egui::Rect::from_center_size(center, egui::vec2(58.0, 74.0));
            painter.rect_filled(page, 3.0, paper);
            r(9.0, 9.0, 34.0, 4.0, c, 1.0, page);
            for i in 0..5 {
                r(9.0, 22.0 + i as f32 * 9.0, 40.0, 3.0, line, 1.0, page);
            }
            r(9.0, 66.0, 40.0, 3.0, c, 1.0, page);
        }
        None => {}
    }
}

fn ago(t: std::time::SystemTime) -> String {
    // `SystemTime::now()` panics on wasm32; web_time's is std's natively and the browser's in a page.
    let now = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let then = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let secs = now.saturating_sub(then);
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

/// Marks that Home, not a file, was showing when the session was saved.
const HOME_MARK: &str = "#home";

/// Files that were open last time (the active one last), and whether Home was showing.
fn load_session() -> (Vec<String>, bool) {
    let text = platform::read("session.txt").unwrap_or_default();
    let home = text.lines().any(|l| l == HOME_MARK);
    let files = text.lines().filter(|l| !l.is_empty() && *l != HOME_MARK && std::path::Path::new(l).is_file()).map(str::to_string).collect();
    (files, home)
}

fn save_session(lines: &[String]) {
    platform::write_lines("session.txt", lines);
}

/// Whether Home has the shown folder's files yet.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum Listing {
    Ready,
    Loading,
    Failed(String),
}

/// A Home file operation waiting for the user's answer in a dialog.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum FileOp {
    Rename { path: String, name: String, error: Option<String> },
    NewFolder { dir: String, name: String, error: Option<String> },
    /// Move to Trash, confirmed first when the item is a folder or open in a tab.
    Trash { path: String, open: Vec<u64>, is_dir: bool },
}

/// A Home row being dragged onto a folder to move it there.
#[derive(Clone)]
struct MovePayload(String);

/// A sidebar folder being dragged to another place in the list (its index).
#[derive(Clone, Copy)]
struct FolderDrag(usize);

/// Why the window is about to close.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Quit {
    Close,
    /// Updates: start the new build, then close.
    Restart,
}

// The quit guard, Home browsing state and recovery probe are desktop-only.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
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
    /// Leaving the app (window close or an update's restart) with unsaved tabs, waiting for an answer.
    confirm_quit: Option<Quit>,
    /// The user chose to leave and discard: the next close request goes through.
    quit_ok: bool,
    /// Desktop: the folder shown on Home, inside the selected sidebar folder (`None`: its top).
    cwd: Option<String>,
    /// A rename, new folder or trash waiting in its dialog.
    file_op: Option<FileOp>,
    /// Home shows OfficeCraft's own trash (items from places with no system trash).
    show_trash: bool,
    /// Desktop: the newest listing request per folder (older answers are dropped).
    listing_seq: std::collections::HashMap<String, u64>,
    /// Whether the folder shown has been read yet, or could not be.
    listing_state: Listing,
    /// The rename / new folder field has been focused once (it isn't grabbed back every frame).
    file_op_focused: bool,
    /// "Delete forever" asked for this OfficeCraft Trash item: waiting for the confirm.
    #[cfg(not(target_arch = "wasm32"))]
    purge_confirm: Option<files::OwnItem>,
    /// Quit and discard was chosen: keep asking the window to close until it does.
    leaving: bool,
    /// Launch: a WordCraft session reading the recovery folder (see `poll_word_recovery`).
    #[cfg(not(target_arch = "wasm32"))]
    word_probe: Option<wordcraft_engine::Session>,
    /// The tab last scrolled into view in the tab strip.
    tab_in_view: Option<u64>,
    /// The tab strip is wider than the bar: show the ⋯ list of all tabs.
    tabs_overflow: bool,
    /// The last Home file operation that went through ("Moved … to the Trash"), shown in teal.
    status: Option<String>,
    /// A short message over any view (Send to… results): text, whether it is an error, and when
    /// it was shown (egui time).
    toast: Option<(String, bool, f64)>,
    /// New from prompt on Home: the description being typed, and the app chosen for it (`None`:
    /// worked out from the words, `kind_for_prompt`).
    make_prompt: String,
    make_kind: Option<Kind>,
    /// The window had keyboard focus last frame (desktop: refresh Home when it comes back).
    focused: bool,
    /// Recent paths and their rows, read again only when the list changes.
    recent_rows: (Vec<String>, Vec<Row>),
    /// What is in OfficeCraft's own trash (desktop), refreshed after each trash operation.
    #[cfg(not(target_arch = "wasm32"))]
    own_trash: Vec<files::OwnItem>,
    ai_cfg: ai::Config,
    ai_settings: Option<ai::Settings>,
    #[cfg(not(target_arch = "wasm32"))]
    updater: updater::Updater,
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
    /// Home shows thumbnails (true) or a table (false). Remembered in `homeview.txt`.
    home_grid: bool,
    /// `OFFICECRAFT_SHOT=<file.ppm>`: draw Home for a few frames, save the window as a PPM, exit.
    shot: Option<(String, u32)>,
    shot_sent: bool,
    home_styled: bool,
    folder_counts: Vec<usize>,
    /// Browser build: listings delivered by the folder picker, and the last one per folder.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    folder_scans: FolderScans,
    scans: std::collections::HashMap<String, Vec<Row>>,
    /// Files the browser delivered (Open file, drag and drop), each opened in its own tab.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    inbox: Inbox,
    /// Each `restores_look` app's fonts and style, as last seen installed.
    looks: std::collections::HashMap<Kind, Look>,
    /// Frames the current context owner has been showing (its own install takes a couple).
    owner_frames: u32,
    /// Frames left to skip drawing a tab whose fonts were just put back (they apply next pass).
    settle: u8,
    /// An app whose style goes back once its fonts are active (see `Look::apply_fonts`).
    restyle: Option<Kind>,
    /// Tabs shown side by side, left to right. Fewer than two means not tiled.
    tiles: Vec<u64>,
    tile_layout: Vec<TileCol>,
    /// Where each tile was drawn last frame, and how many columns fit: drop targets for tab drags.
    tile_rects: Vec<(u64, egui::Rect)>,
    tile_max_cols: usize,
    /// A tab being dragged out of the tab bar.
    tab_drag: Option<u64>,
    tile_test_out: Option<u64>,
    tile_pref: TilePref,
    /// A document was opened beside others and the visitor has not said whether to tile.
    tile_prompt: bool,
    tile_remember: bool,
    /// Every font any tiled app installed, kept installed so none of them loses a family.
    fonts_union: Option<egui::FontDefinitions>,
    /// Frames still to skip drawing tiles after the font set was replaced (it applies next pass).
    tile_settle: u8,
    /// The tabs allowed to run this frame: warmed-up apps plus at most one still installing.
    tile_running: Vec<u64>,
    /// Frames each kind has run in a tile; installing its fonts and style takes a few.
    tile_warm: std::collections::HashMap<Kind, u8>,
    /// Each app's own style as it last installed it, applied inside its tile.
    tile_styles: std::collections::HashMap<Kind, std::sync::Arc<egui::Style>>,
    /// Frame numbers of recent font-set restorations: an app that replaces the set every frame
    /// cannot share the window.
    tile_restores: Vec<u64>,
    tile_frame: u64,
    /// `OFFICECRAFT_TILES=1`: open one tab of each app, tile them, run, exit 0 (or take the
    /// `OFFICECRAFT_SHOT` picture once they have warmed up).
    tiles_test: Option<u64>,
}

impl Suite {
    fn new() -> Self {
        // The verSHion tab shares the standalone app's store; install its backend once.
        #[cfg(not(target_arch = "wasm32"))]
        {
            vershion::store::migrate_legacy_dir();
            vershion::store::init(Box::new(vershion::store::FsBackend::from_env()));
        }
        let mut folders = load_folders();
        // First run: show ~/Documents so Home is not an empty page.
        if folders.is_empty()
            && let Some(d) = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Documents")).filter(|d| d.is_dir())
        {
            folders.push(d.to_string_lossy().to_string());
        }
        let recent = load_recent();
        // The folder's rows arrive from a worker on the first frames (`recount`).
        let (folder_sel, folder_rows) = match folders.first() {
            Some(_) if recent.is_empty() => (Some(0), Vec::new()),
            _ => (None, Vec::new()),
        };
        let suite = Suite { view: View::Home, entries: Vec::new(), next_id: 1, recent, pending: Vec::new(), notice: None, confirm_close: None, confirm_quit: None, quit_ok: false, cwd: None, file_op: None, show_trash: false, listing_seq: std::collections::HashMap::new(), listing_state: Listing::Ready, file_op_focused: false, #[cfg(not(target_arch = "wasm32"))] purge_confirm: None, leaving: false, #[cfg(not(target_arch = "wasm32"))] word_probe: None, tab_in_view: None, tabs_overflow: false, status: None, toast: None, make_prompt: String::new(), make_kind: None, focused: true, recent_rows: (Vec::new(), Vec::new()), #[cfg(not(target_arch = "wasm32"))] own_trash: files::own_trash_list(), ai_cfg: ai::Config::load(), ai_settings: None, #[cfg(not(target_arch = "wasm32"))] updater: updater::Updater::new(), ctx_owner: None, selftest: None, ai_selftest: None, persist: true, last_session: Vec::new(), restore_home: false, filter: Filter::All, search: String::new(), folders, folder_sel, folder_rows, home_grid: load_home_grid(), shot: None, shot_sent: false, home_styled: false, folder_counts: Vec::new(), folder_scans: FolderScans::default(), scans: std::collections::HashMap::new(), inbox: Inbox::default(), looks: std::collections::HashMap::new(), owner_frames: 0, settle: 0, restyle: None, tiles: Vec::new(), tile_layout: Vec::new(), tile_rects: Vec::new(), tile_max_cols: 2, tab_drag: None, tile_test_out: None, tile_pref: TilePref::load(), tile_prompt: false, tile_remember: false, fonts_union: None, tile_settle: 0, tile_running: Vec::new(), tile_warm: std::collections::HashMap::new(), tile_styles: std::collections::HashMap::new(), tile_restores: Vec::new(), tile_frame: 0, tiles_test: None };
        suite
    }

    /// Sidebar counts and the shown folder: read again in the background (desktop), or from the
    /// browser's last listings (page).
    fn recount(&mut self, ctx: &egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.folder_counts.resize(self.folders.len(), 0);
            for f in self.folders.clone() {
                self.request_listing(ctx, &f);
            }
            if let Some(d) = self.cwd.clone()
                && !self.folders.contains(&d)
            {
                self.request_listing(ctx, &d);
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = ctx;
            self.folder_counts = self.folders.iter().map(|f| folder_listing(&self.scans, f).len()).collect();
        }
    }

    /// The folder Home shows: a subfolder being browsed, else the selected sidebar folder.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn shown_dir(&self) -> Option<String> {
        if self.folder_sel.is_none() {
            return None;
        }
        self.cwd.clone().or_else(|| self.folder_sel.and_then(|i| self.folders.get(i).cloned()))
    }

    /// Lists `dir` on a worker thread; the rows arrive through `folder_scans`.
    #[cfg(not(target_arch = "wasm32"))]
    fn request_listing(&mut self, ctx: &egui::Context, dir: &str) {
        let seq = {
            let n = self.listing_seq.entry(dir.to_string()).or_insert(0);
            *n += 1;
            *n
        };
        let scans = self.folder_scans.clone();
        let dir = dir.to_string();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let (rows, error) = match list_level(&dir) {
                Ok(rows) => (rows, None),
                Err(e) => (Vec::new(), Some(e)),
            };
            scans.lock().unwrap_or_else(|e| e.into_inner()).push(FolderScan { key: dir, rows, added: false, seq, error });
            ctx.request_repaint();
        });
    }

    /// Shows `dir` (`None`: the top of the selected sidebar folder) and lists it again.
    #[cfg(not(target_arch = "wasm32"))]
    fn browse(&mut self, ctx: &egui::Context, dir: Option<String>) {
        let top = self.folder_sel.and_then(|i| self.folders.get(i).cloned());
        self.cwd = dir.filter(|d| Some(d) != top.as_ref());
        self.status = None;
        if let Some(d) = self.shown_dir() {
            self.folder_rows = folder_listing(&self.scans, &d);
            self.listing_state = if self.scans.contains_key(&d) { Listing::Ready } else { Listing::Loading };
            self.request_listing(ctx, &d);
        }
    }

    /// Takes in folder listings the workers (desktop) delivered.
    #[cfg(not(target_arch = "wasm32"))]
    fn take_listings(&mut self) {
        let scanned = std::mem::take(&mut *self.folder_scans.lock().unwrap_or_else(|e| e.into_inner()));
        let shown = self.shown_dir();
        for FolderScan { key, rows, seq, error, .. } in scanned {
            // A later request for this folder is out: this answer is stale (a file just moved).
            if self.listing_seq.get(&key).is_some_and(|latest| seq < *latest) {
                continue;
            }
            if shown.as_deref() == Some(key.as_str()) {
                self.listing_state = match error {
                    Some(e) => Listing::Failed(e),
                    None => Listing::Ready,
                };
            }
            if let Some(i) = self.folders.iter().position(|f| *f == key) {
                self.folder_counts.resize(self.folders.len(), 0);
                self.folder_counts[i] = rows.len();
            }
            if shown.as_deref() == Some(key.as_str()) {
                self.folder_rows = rows.clone();
            }
            self.scans.insert(key, rows);
        }
    }

    /// Starts a fresh, empty instance of `kind` and shows it.
    /// A tab with a new, blank file in it (what New file makes).
    fn new_tab(&mut self, kind: Kind) -> u64 {
        // One verSHion workspace tab: reopening focuses it rather than making a second store.
        if kind == Kind::Vershion
            && let Some(id) = self.entries.iter().find(|e| e.kind == kind).map(|e| e.id)
        {
            self.view = View::Tab(id);
            return id;
        }
        let tab = match kind {
            // A presentation needs a slide to work on: an assistant (or a first click) has nothing
            // to act on in an empty session.
            Kind::Deck => Tab::Deck(Box::new(deckcraft_ui_egui::SlideApp::new(deck_session(deckcraft_engine::Session::with_new()), deck_services()))),
            _ => Tab::create(kind),
        };
        self.push_tab(kind, tab)
    }

    fn push_tab(&mut self, kind: Kind, tab: Tab) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let n = self.entries.iter().filter(|e| e.kind == kind).count() + 1;
        self.entries.push(Entry { id, kind, name: format!("{} {n}", kind.label()), path: None, tab, chat: { let mut c = ai::Chat::new(); c.tab = id; c }, graph_for: None, pdf_client: None, seen_path: None });
        self.view = View::Tab(id);
        if !kind.tileable() {
            // The Graph and Equations tabs bring their own fonts and layout; they are never tiled.
        } else if self.tiles.len() >= 2 {
            // Already tiled: the new document joins, the oldest tile steps back to its tab.
            self.tiles.push(id);
            while self.tiles.len() > MAX_TILES {
                self.tiles.remove(0);
            }
        } else if self.entries.iter().filter(|e| e.kind.tileable()).count() >= 2 {
            match self.tile_pref {
                TilePref::Ask => self.tile_prompt = true,
                TilePref::Always => self.tile_all(),
                TilePref::Never => {}
            }
        }
        id
    }

    /// The tabs drawn side by side right now: only while the front tab is one of them.
    fn tiled_ids(&self) -> Option<Vec<u64>> {
        let View::Tab(id) = self.view else { return None };
        (self.tiles.len() >= 2 && self.tiles.contains(&id)).then(|| self.tiles.clone())
    }

    /// Tiles the most recently opened tabs (up to `MAX_TILES`).
    fn tile_all(&mut self) {
        let tileable: Vec<u64> = self.entries.iter().filter(|e| e.kind.tileable()).map(|e| e.id).collect();
        self.tiles = tileable[tileable.len().saturating_sub(MAX_TILES)..].to_vec();
        if self.tiles.len() < 2 {
            self.tiles.clear();
            return;
        }
        if !matches!(self.view, View::Tab(id) if self.tiles.contains(&id)) {
            self.view = View::Tab(*self.tiles.last().unwrap_or(&0));
        }
        // Each app installs its fonts and style again, one after another.
        self.tile_warm.clear();
        self.tile_restores.clear();
        self.tile_prompt = false;
    }

    /// Removes a tile from the tiled view; its document stays open in its tab.
    fn take_out_tile(&mut self, id: u64) {
        self.tiles.retain(|t| *t != id);
        if self.tiles.len() < 2 {
            self.untile();
        }
    }

    /// Brings tab `id` into the tiles (starting a tiled view with the front tab if there is none),
    /// optionally at a drop position next to `at.0`.
    fn tile_with(&mut self, id: u64, at: Option<(u64, DropZone)>) {
        if self.entries.iter().find(|e| e.id == id).is_none_or(|e| !e.kind.tileable()) {
            return;
        }
        if self.tiles.len() < 2 {
            let tileable = |entries: &[Entry], t: u64| entries.iter().any(|e| e.id == t && e.kind.tileable());
            let front = match self.view {
                View::Tab(v) if v != id && tileable(&self.entries, v) => Some(v),
                _ => None,
            };
            let partner = front.or_else(|| self.entries.iter().rev().find(|e| e.id != id && e.kind.tileable()).map(|e| e.id));
            let Some(p) = partner else { return };
            self.tiles = vec![p, id];
            self.tile_layout.clear();
            self.tile_warm.clear();
            self.tile_restores.clear();
            self.tile_prompt = false;
        } else if !self.tiles.contains(&id) {
            self.tiles.push(id);
            while self.tiles.len() > MAX_TILES {
                self.tiles.remove(0);
            }
            if let Some((target, zone)) = at {
                self.reflow_tiles(self.tile_max_cols);
                self.move_tile(id, target, zone);
            }
        }
        self.view = View::Tab(id);
    }

    fn untile(&mut self) {
        self.tiles.clear();
        self.tile_running.clear();
        self.tile_prompt = false;
        // The front app must install its own fonts and style alone again.
        self.ctx_owner = None;
    }

    fn prune_tiles(&mut self) {
        let have: Vec<u64> = self.entries.iter().map(|e| e.id).collect();
        self.tiles.retain(|t| have.contains(t));
        if self.tiles.len() < 2 && !self.tiles.is_empty() {
            self.tiles.clear();
        }
    }

    /// Per-frame work for tiled apps: keep every app's fonts installed, let one app at a time
    /// warm up, then run each app's `logic` with only the front tile receiving key events.
    fn tile_logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame, ids: &[u64]) {
        self.tile_frame += 1;
        let active = ctx.fonts(|f| f.definitions().clone());
        let union = self.fonts_union.get_or_insert_with(|| active.clone());
        merge_fonts(union, &active);
        for id in ids {
            if let Some(kind) = self.entries.iter().find(|e| e.id == *id).map(|e| e.kind)
                && let Some(look) = self.looks.get(&kind)
            {
                // GridCraft and PdfCraft: what they installed when they last had the window.
                merge_fonts(union, &look.fonts);
                self.tile_styles.entry(kind).or_insert_with(|| look.light.clone());
            }
        }
        if !fonts_cover(&active, union) {
            ctx.set_fonts(union.clone());
            self.tile_settle = 2;
            let now = self.tile_frame;
            self.tile_restores.retain(|f| now.saturating_sub(*f) < 180);
            self.tile_restores.push(now);
            ctx.request_repaint();
            if self.tile_restores.len() > 10 {
                self.notice = Some("These apps keep replacing each other's fonts, so they cannot be shown side by side. They stay open as tabs.".into());
                self.untile();
                return;
            }
        }

        // Warm-up: every kind that has not yet installed waits its turn, one per frame.
        self.tile_running.clear();
        let mut installing = false;
        for id in ids {
            let Some(kind) = self.entries.iter().find(|e| e.id == *id).map(|e| e.kind) else { continue };
            let seen = self.looks.contains_key(&kind) && restores_look(kind);
            let warm = self.tile_warm.get(&kind).copied().unwrap_or(if seen { TILE_WARM_FRAMES } else { 0 });
            if warm < TILE_WARM_FRAMES {
                if installing {
                    continue;
                }
                installing = true;
                if warm == 0
                    && let Some(e) = self.entries.iter_mut().find(|e| e.id == *id)
                {
                    // Docs and Slides put their fonts and style back when told the context changed.
                    e.tab.context_changed(ctx);
                }
                ctx.request_repaint();
            }
            self.tile_warm.insert(kind, warm.saturating_add(1));
            self.tile_running.push(*id);
        }

        let front = self.view;
        let running = self.tile_running.clone();
        let theme_style_before = ctx.global_style();
        for id in running {
            let withhold = front != View::Tab(id);
            let Some(e) = self.entries.iter_mut().find(|e| e.id == id) else { continue };
            with_events_withheld(ctx, withhold, || e.tab.logic(ctx, frame));
            let after = ctx.global_style();
            if !std::sync::Arc::ptr_eq(&theme_style_before, &after) {
                self.tile_styles.insert(e.kind, after);
            }
        }
    }

    /// Keeps `tile_layout` in step with `tiles`: drops closed tiles, places new ones, and folds
    /// columns together when the window no longer has room for them.
    fn reflow_tiles(&mut self, max_cols: usize) {
        let want = self.tiles.clone();
        let lay = &mut self.tile_layout;
        let mut cols_changed = false;
        for c in lay.iter_mut() {
            let n = c.rows.len();
            c.rows.retain(|(id, _)| want.contains(id));
            if c.rows.len() != n {
                renorm_rows(c);
            }
        }
        if lay.iter().any(|c| c.rows.is_empty()) {
            lay.retain(|c| !c.rows.is_empty());
            cols_changed = true;
        }
        while lay.len() > max_cols.max(1) {
            if let Some(last) = lay.pop() {
                if let Some(c) = lay.last_mut() {
                    c.rows.extend(last.rows);
                    equalize_rows(c);
                }
            }
            cols_changed = true;
        }
        for id in want {
            if lay.iter().any(|c| c.rows.iter().any(|(r, _)| *r == id)) {
                continue;
            }
            if lay.len() < max_cols {
                lay.push(TileCol { w: 0.0, rows: vec![(id, 1.0)] });
                equalize_cols(lay);
            } else if let Some(i) = (0..lay.len()).min_by_key(|i| lay[*i].rows.len()) {
                lay[i].rows.push((id, 0.0));
                equalize_rows(&mut lay[i]);
            }
            cols_changed = false;
        }
        if cols_changed {
            renorm_cols(lay);
        }
        self.tiles = lay.iter().flat_map(|c| c.rows.iter().map(|(id, _)| *id)).collect();
    }

    /// Moves tile `id` next to `target` (or swaps them) and gives the touched row or column an even split.
    fn move_tile(&mut self, id: u64, target: u64, zone: DropZone) {
        let pos = |lay: &[TileCol], t: u64| lay.iter().enumerate().find_map(|(ci, c)| c.rows.iter().position(|(r, _)| *r == t).map(|ri| (ci, ri)));
        let lay = &mut self.tile_layout;
        let (Some((ci, ri)), Some((tc, tr))) = (pos(lay, id), pos(lay, target)) else { return };
        if zone == DropZone::Swap {
            let (a, b) = (lay[ci].rows[ri].0, lay[tc].rows[tr].0);
            lay[ci].rows[ri].0 = b;
            lay[tc].rows[tr].0 = a;
        } else {
            lay[ci].rows.remove(ri);
            if lay[ci].rows.is_empty() {
                lay.remove(ci);
                renorm_cols(lay);
            } else {
                renorm_rows(&mut lay[ci]);
            }
            let Some((tc, tr)) = pos(lay, target) else { return };
            match zone {
                DropZone::Above | DropZone::Below => {
                    let at = if zone == DropZone::Above { tr } else { tr + 1 };
                    lay[tc].rows.insert(at, (id, 0.0));
                    equalize_rows(&mut lay[tc]);
                }
                _ => {
                    let at = if zone == DropZone::ColBefore { tc } else { tc + 1 };
                    lay.insert(at, TileCol { w: 0.0, rows: vec![(id, 1.0)] });
                    equalize_cols(lay);
                }
            }
        }
        self.tiles = lay.iter().flat_map(|c| c.rows.iter().map(|(r, _)| *r)).collect();
    }

    /// Draws the tiled tabs. Each tile has a title strip to drag it by; the gaps between tiles
    /// resize them. Columns are capped by the window's width (`max_tile_cols`).
    fn tile_ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame, _ids: &[u64]) {
        let ctx = ui.ctx().clone();
        let settling = self.tile_settle > 0;
        self.tile_settle = self.tile_settle.saturating_sub(1);
        let area = ui.available_rect_before_wrap();
        let max_cols = max_tile_cols(area.width());
        self.reflow_tiles(max_cols);
        let gap = 6.0;
        let head = 20.0;
        self.tile_max_cols = max_cols;
        let ncols = self.tile_layout.len().max(1);
        let avail_w = (area.width() - gap * (ncols as f32 - 1.0)).max(1.0);
        let min_w = (280.0 / avail_w).min(1.0 / ncols as f32);

        // Geometry of every tile, and the splitters between them.
        let mut rects: Vec<(u64, egui::Rect)> = Vec::new();
        let mut col_adj: Option<(usize, f32)> = None;
        let mut row_adj: Option<(usize, usize, f32)> = None;
        let mut x = area.min.x;
        for ci in 0..self.tile_layout.len() {
            let col = &self.tile_layout[ci];
            let w = col.w * avail_w;
            let nrows = col.rows.len();
            let avail_h = (area.height() - gap * (nrows as f32 - 1.0)).max(1.0);
            let min_h = (150.0 / avail_h).min(1.0 / nrows as f32);
            let mut y = area.min.y;
            for (ri, (id, h)) in col.rows.iter().enumerate() {
                let hh = h * avail_h;
                let r = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, hh));
                rects.push((*id, r));
                if ri + 1 < nrows {
                    let bar = egui::Rect::from_min_size(egui::pos2(x, y + hh), egui::vec2(w, gap)).expand2(egui::vec2(0.0, 2.0));
                    let resp = ui.interact(bar, egui::Id::new(("tile-rsplit", ci, ri)), egui::Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeVertical);
                    if resp.dragged() {
                        let d = resp.drag_delta().y / avail_h;
                        let (a, b) = (col.rows[ri].1, col.rows[ri + 1].1);
                        let d = d.clamp(min_h - a, b - min_h);
                        row_adj = Some((ci, ri, d));
                    }
                }
                y += hh + gap;
            }
            if ci + 1 < self.tile_layout.len() {
                let bar = egui::Rect::from_min_size(egui::pos2(x + w, area.min.y), egui::vec2(gap, area.height())).expand2(egui::vec2(2.0, 0.0));
                let resp = ui.interact(bar, egui::Id::new(("tile-csplit", ci)), egui::Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                if resp.dragged() {
                    let d = resp.drag_delta().x / avail_w;
                    let (a, b) = (col.w, self.tile_layout[ci + 1].w);
                    col_adj = Some((ci, d.clamp(min_w - a, b - min_w)));
                }
            }
            x += w + gap;
        }
        if let Some((ci, d)) = col_adj {
            self.tile_layout[ci].w += d;
            self.tile_layout[ci + 1].w -= d;
        }
        if let Some((ci, ri, d)) = row_adj {
            self.tile_layout[ci].rows[ri].1 += d;
            self.tile_layout[ci].rows[ri + 1].1 -= d;
        }

        self.tile_rects = rects.clone();
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let pressed = ctx.input(|i| if i.pointer.any_pressed() { i.pointer.interact_pos() } else { None });
        let mut focus = None;
        let mut dragging: Option<u64> = None;
        let mut dropped: Option<u64> = None;
        let mut close_req: Option<u64> = None;
        let mut take_out: Option<u64> = None;
        let mut new_kind: Option<Kind> = None;
        let mut add_tab: Option<u64> = None;
        // Open documents that are not in the tiles: they can be added from a tile's "+" menu.
        let others: Vec<(u64, String)> = self.entries.iter().filter(|e| e.kind.tileable() && !self.tiles.contains(&e.id)).map(|e| (e.id, e.name.clone())).collect();
        for (tid, rect) in rects.iter().copied() {
            let focused = self.view == View::Tab(tid);
            if !focused && pressed.is_some_and(|p| rect.contains(p)) {
                focus = Some(tid);
            }
            let running = self.tile_running.contains(&tid) && !settling;
            let Some(e) = self.entries.iter_mut().find(|e| e.id == tid) else { continue };
            let kind_col = kind_color(Some(e.kind));
            // Title strip: the handle for dragging the tile to another place.
            let strip = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), head));
            let handle = ui.interact(strip, egui::Id::new(("tile-head", tid)), egui::Sense::click_and_drag());
            if handle.hovered() || handle.dragged() {
                ctx.set_cursor_icon(if handle.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
            }
            if handle.dragged() {
                dragging = Some(tid);
            }
            if handle.drag_stopped() {
                dropped = Some(tid);
            }
            if handle.clicked() || handle.drag_started() {
                focus = Some(tid);
            }
            handle.context_menu(|ui| {
                if ui.button("Remove from the tiles").clicked() {
                    take_out = Some(tid);
                    ui.close();
                }
                if ui.button("Close document").clicked() {
                    close_req = Some(tid);
                    ui.close();
                }
            });
            let painter = ui.painter();
            // Solid colours: the apps' own visuals (light or dark) must not decide legibility.
            let fill = if focused { kind_col } else { kind_col.lerp_to_gamma(egui::Color32::from_gray(110), 0.45) };
            painter.rect_filled(strip, 0.0, fill);
            for i in 0..3 {
                painter.circle_filled(strip.left_center() + egui::vec2(9.0 + i as f32 * 5.0, 0.0), 1.4, egui::Color32::WHITE);
            }
            let title = painter.layout_no_wrap(e.name.clone(), egui::FontId::proportional(11.5), egui::Color32::WHITE);
            let btns = egui::Rect::from_min_max(egui::pos2((strip.max.x - 56.0).max(strip.min.x + 28.0), strip.min.y), strip.max);
            let clip = egui::Rect::from_min_max(strip.min + egui::vec2(28.0, 0.0), egui::pos2(btns.min.x, strip.max.y));
            painter.with_clip_rect(clip).galley(egui::pos2(clip.min.x, strip.center().y - title.size().y / 2.0), title, egui::Color32::WHITE);

            // Close, take out of the tiles (it stays a tab), and add another document.
            let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(btns.shrink2(egui::vec2(4.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
            bui.spacing_mut().item_spacing.x = 2.0;
            bui.spacing_mut().button_padding = egui::vec2(4.0, 0.0);
            bui.spacing_mut().interact_size.y = head - 2.0;
            {
                let w = &mut bui.visuals_mut().widgets;
                for st in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
                    st.weak_bg_fill = egui::Color32::TRANSPARENT;
                    st.bg_fill = egui::Color32::TRANSPARENT;
                    st.bg_stroke = egui::Stroke::NONE;
                }
                w.hovered.weak_bg_fill = egui::Color32::from_white_alpha(46);
                w.active.weak_bg_fill = egui::Color32::from_white_alpha(70);
                w.open.weak_bg_fill = egui::Color32::from_white_alpha(46);
            }
            let white = |t: &str| egui::RichText::new(t.to_string()).color(egui::Color32::WHITE).size(12.5);
            if bui.add(egui::Button::new(white("×")).frame(false)).on_hover_text("Remove from the tiles; the document stays open in its tab").clicked() {
                take_out = Some(tid);
            }
            bui.menu_button(white("+"), |ui| {
                for k in [Kind::Word, Kind::Grid, Kind::Deck, Kind::Pdf] {
                    if ui.button(format!("New {}", k.label())).clicked() {
                        new_kind = Some(k);
                        ui.close();
                    }
                }
                if !others.is_empty() {
                    ui.separator();
                    ui.label("Add an open tab");
                    for (id, name) in &others {
                        if ui.button(name).clicked() {
                            add_tab = Some(*id);
                            ui.close();
                        }
                    }
                }
            });

            let body = egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.min.y + head), rect.max);
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
            child.set_clip_rect(body.intersect(ui.clip_rect()));
            if running {
                if let Some(style) = self.tile_styles.get(&e.kind) {
                    child.set_style(style.clone());
                }
                let before = ctx.global_style();
                with_events_withheld(&ctx, !focused, || {
                    child.push_id(("tile", tid), |ui| e.tab.ui(ui, frame));
                });
                let after = ctx.global_style();
                if !std::sync::Arc::ptr_eq(&before, &after) {
                    self.tile_styles.insert(e.kind, after);
                }
            } else {
                child.centered_and_justified(|ui| {
                    ui.label(format!("Loading {}…", e.name));
                });
                ctx.request_repaint();
            }
            let ring = if focused { egui::Stroke::new(2.0, kind_col) } else { egui::Stroke::new(1.0, LINE) };
            ui.painter().rect_stroke(rect, 0.0, ring, egui::StrokeKind::Inside);
        }
        ui.allocate_rect(area, egui::Sense::hover());

        // While a tile is being dragged, show where it would land; apply it on release.
        if let (Some(tid), Some(p)) = (dragging.or(dropped).or(self.tab_drag), pointer) {
            let alone = self.tile_layout.iter().any(|c| c.rows.len() == 1 && c.rows[0].0 == tid);
            let room = self.tile_layout.len() - usize::from(alone) < max_cols;
            if let Some((target, zone, trect)) = drop_target(&rects, tid, p, room) {
                if dropped.is_none() {
                    let hint = drop_hint_rect(trect, zone);
                    let layer = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("tile-drop")));
                    layer.rect_filled(hint, 4.0, ACCENT.gamma_multiply(0.28));
                    layer.rect_stroke(hint, 4.0, egui::Stroke::new(2.0, ACCENT), egui::StrokeKind::Inside);
                } else {
                    self.move_tile(tid, target, zone);
                    focus = Some(tid);
                }
            }
        }
        if let Some(t) = focus {
            self.view = View::Tab(t);
        }
        if let Some(id) = take_out {
            self.take_out_tile(id);
        }
        if let Some(id) = add_tab {
            self.tiles.push(id);
            while self.tiles.len() > MAX_TILES {
                self.tiles.remove(0);
            }
            self.view = View::Tab(id);
        }
        if let Some(k) = new_kind {
            // Already tiled, so `push_tab` puts the new document into the tiles.
            self.new_tab(k);
        }
        if let Some(id) = close_req {
            self.request_close(id);
        }
    }

    fn entry_mut(&mut self, id: u64) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    /// Opens `path` in its own tab, or switches to the tab that already has it.
    fn open_file(&mut self, ctx: &egui::Context, path: &str) {
        // Browser build: a row of an added folder is read through its handle and arrives via the inbox.
        #[cfg(target_arch = "wasm32")]
        if web::is_folder_path(path) {
            web::open_from_folder(path, self.inbox.clone());
            ctx.request_repaint();
            return;
        }
        // One spelling per file, so a symlink or `..` path finds the tab that already has it
        // instead of opening a second copy that would overwrite the first on save.
        let canonical = canonical_path(path);
        let path = canonical.as_str();
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
        // An empty app to open the file into (not a blank new file, which would sit beside it).
        let id = self.push_tab(kind, Tab::create(kind));
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

    /// Web: opens a file the browser delivered in its own tab. There is no path, so no recent entry.
    #[cfg(target_arch = "wasm32")]
    fn open_bytes(&mut self, ctx: &egui::Context, name: String, bytes: Vec<u8>) {
        let Some(kind) = kind_for(&name) else {
            self.notice = Some(format!("No Craft app opens {name}"));
            return;
        };
        let id = self.push_tab(kind, Tab::create(kind));
        if let Some(e) = self.entry_mut(id) {
            e.tab.deliver(&name, bytes);
            e.name = name;
        }
        self.notice = None;
        ctx.request_repaint();
    }

    fn close(&mut self, id: u64) {
        // DeckCraft forgets a presentation's recovery copy only when the document is closed, not
        // when the session is dropped: close them, or a discarded deck comes back next launch.
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id)
            && let Tab::Deck(a) = &mut e.tab
        {
            while !a.session.documents().is_empty() {
                a.session.close_document(0);
            }
        }
        self.entries.retain(|e| e.id != id);
        self.confirm_close = None;
        self.prune_tiles();
        if self.view == View::Tab(id) {
            self.view = self.entries.last().map(|e| View::Tab(e.id)).unwrap_or(View::Home);
        }
    }

    /// Draws `args`' graph and inserts it as a picture after the caret of document tab `doc`, with
    /// the graph stored in the PNG. The caret then moves past the picture, so the next graph
    /// follows it instead of replacing it.
    fn insert_graph_picture(&mut self, doc: u64, args: &serde_json::Value, ctx: &egui::Context) -> Result<(), String> {
        let (png, json) = render_graph(args)?;
        let alt = args.get("title").and_then(serde_json::Value::as_str).filter(|t| !t.trim().is_empty()).map(|t| format!("Graph: {t}")).unwrap_or_else(|| "Graph".into());
        let data = graphpic::base64_encode(&graphpic::embed(&png, &json).unwrap_or(png));
        let e = self.entry_mut(doc).ok_or_else(|| "the document was closed".to_string())?;
        let mut host = TabHost { tab: &mut e.tab, client: &mut e.pdf_client, ctx };
        let call = |host: &mut TabHost, command: &str, params: serde_json::Value| match ai::Host::call(host, "engine.execute", serde_json::json!({"command": command, "params": params})) {
            ai::CallResult::Done(r) => r.map(|_| ()),
            ai::CallResult::Pending(_) => Err("the document did not answer".to_string()),
        };
        call(&mut host, "insert.picture", serde_json::json!({"data": data, "width": 430, "alt": alt}))?;
        let _ = call(&mut host, "select.collapse", serde_json::json!({"end": true}));
        Ok(())
    }

    /// Graphs a document's assistant asked for: each is drawn with viSHual (no tab, no window) and
    /// inserted as a picture at the caret, with the graph stored inside the PNG for editing later.
    fn make_requested_graphs(&mut self, ctx: &egui::Context) {
        let Some((src, graphs)) = self.entries.iter_mut().find_map(|e| {
            let g = e.chat.take_graphs();
            (!g.is_empty()).then_some((e.id, g))
        }) else {
            return;
        };
        let (mut made, mut failed) = (0, Vec::new());
        for args in graphs {
            match self.insert_graph_picture(src, &args, ctx) {
                Ok(()) => made += 1,
                Err(e) => failed.push(e),
            }
        }
        let mut note = match made {
            0 => String::new(),
            1 => "Inserted a graph. Double-click it to edit it in viSHual.".to_string(),
            n => format!("Inserted {n} graphs. Double-click one to edit it in viSHual."),
        };
        if !failed.is_empty() {
            if !note.is_empty() {
                note.push(' ');
            }
            note.push_str(&format!("{} graph(s) could not be drawn: {}", failed.len(), failed.join("; ")));
        }
        if let Some(e) = self.entry_mut(src) {
            e.chat.note(note);
        }
        ctx.request_repaint();
    }

    /// A picture double-clicked in a document: if it carries a graph, it opens in a Graph tab
    /// linked back to the document (see the bar at the top of that tab).
    fn open_double_clicked_graphs(&mut self, ctx: &egui::Context) {
        let mut found = None;
        for e in self.entries.iter_mut() {
            if let Tab::Word(a) = &mut e.tab
                && let Some(bytes) = take_picture_edit(a, ctx)
                && let Some(json) = graphpic::extract(&bytes)
            {
                found = Some((e.id, e.name.clone(), json));
                break;
            }
        }
        let Some((doc_id, doc_name, json)) = found else { return };
        // The same picture's editor is already open: show it.
        if let Some(open) = self.entries.iter().find(|e| e.graph_for == Some(doc_id)) {
            self.view = View::Tab(open.id);
            return;
        }
        let id = self.push_tab(Kind::Graph, Tab::create(Kind::Graph));
        let result = match self.entry_mut(id) {
            Some(e) => {
                e.graph_for = Some(doc_id);
                e.name = format!("Graph ({doc_name})");
                match &mut e.tab {
                    Tab::Graph(g) => g.load_json(&json),
                    _ => Err("not a graph tab".into()),
                }
            }
            None => Err("tab missing".into()),
        };
        if let Err(e) = result {
            self.entries.retain(|x| x.id != id);
            self.view = View::Tab(doc_id);
            self.notice = Some(format!("Could not open the graph: {e}"));
        }
        ctx.request_repaint();
    }

    /// The linked Graph tab's "Update picture": redraws the graph, stores it in the PNG and puts
    /// it in place of the picture in the document, then returns to the document.
    fn update_graph_picture(&mut self, graph_id: u64, ctx: &egui::Context) {
        let Some(doc_id) = self.entries.iter().find(|e| e.id == graph_id).and_then(|e| e.graph_for) else { return };
        let rendered = match self.entry_mut(graph_id).map(|e| &mut e.tab) {
            Some(Tab::Graph(g)) => g.render_png(GRAPH_PX).map(|png| (png, g.doc_json())),
            _ => Err("not a graph tab".into()),
        };
        let result = rendered.and_then(|(png, json)| {
            let data = graphpic::base64_encode(&graphpic::embed(&png, &json).unwrap_or(png));
            let e = self.entry_mut(doc_id).ok_or_else(|| "the document was closed".to_string())?;
            let mut host = TabHost { tab: &mut e.tab, client: &mut e.pdf_client, ctx };
            match ai::Host::call(&mut host, "engine.execute", serde_json::json!({"command": "insert.picture", "params": {"data": data}})) {
                ai::CallResult::Done(r) => r.map(|_| ()).map_err(|e| format!("{e} (select the picture in the document again)")),
                ai::CallResult::Pending(_) => Err("the document did not answer".into()),
            }
        });
        match result {
            Ok(()) => {
                self.close(graph_id);
                self.view = View::Tab(doc_id);
                self.notice = None;
            }
            Err(e) => self.notice = Some(format!("Could not update the picture: {e}")),
        }
        ctx.request_repaint();
    }

    /// `OFFICECRAFT_SELFTEST_SENDTO=<dir>`: every Send to… path once, checked through the apps'
    /// own inspection, with the PDFs written to `dir`. Exits 0 when all of them hold.
    #[cfg(not(target_arch = "wasm32"))]
    fn send_selftest(&mut self, ctx: &egui::Context, dir: &str) {
        let mut bad: Vec<String> = Vec::new();
        let mut check = |ok: bool, what: String| {
            eprintln!("sendto-selftest: {} {what}", if ok { "ok  " } else { "FAIL" });
            if !ok {
                bad.push(what);
            }
        };
        let blocks = |suite: &mut Suite, id: u64| -> Vec<serde_json::Value> {
            suite.tab_call(ctx, id, "document.inspect", serde_json::json!({ "text": true })).ok().and_then(|v| v.get("blocks").and_then(serde_json::Value::as_array).cloned()).unwrap_or_default()
        };
        let slide_titles = |suite: &mut Suite, id: u64| -> Vec<String> {
            suite
                .tab_call(ctx, id, "document.inspect", serde_json::json!({}))
                .ok()
                .and_then(|v| v.get("slides").and_then(serde_json::Value::as_array).map(|a| a.iter().map(|s| s.get("title").and_then(serde_json::Value::as_str).unwrap_or("").to_string()).collect()))
                .unwrap_or_default()
        };
        // Docs → Slides.
        let doc = self.new_tab(Kind::Word);
        let outline = serde_json::json!({"blocks": [
            {"text": "Field Trip", "style": "Title"},
            {"text": "Where we go", "style": "Heading 1"},
            {"text": "The science museum downtown.", "style": "Normal"},
            {"text": "Bus leaves at 8", "style": "Bullet"},
            {"text": "Cost", "style": "Heading 1"},
            {"text": "Twelve dollars a ticket.", "style": "Normal"},
        ]});
        if let Some(e) = self.entry_mut(doc) {
            let mut host = TabHost { tab: &mut e.tab, client: &mut e.pdf_client, ctx };
            if let Err(er) = ai::insert_blocks(&mut host, &outline) {
                check(false, format!("writing the test document: {er}"));
            }
        }
        let n_before = self.entries.len();
        let r = self.convert_tab(ctx, doc, SendTo::New(Kind::Deck));
        let deck = self.entries.last().map(|e| e.id).unwrap_or(0);
        let titles = slide_titles(self, deck);
        check(r.is_ok() && self.entries.len() == n_before + 1, format!("docs → new slides: {r:?}"));
        check(titles == ["Field Trip", "Where we go", "Cost"], format!("deck titles {titles:?}"));
        check(self.entries.iter().find(|e| e.id == deck).is_some_and(|e| e.name == "Field Trip"), "the new deck is named after the title".into());
        // Docs → that open deck: three more slides.
        let r = self.convert_tab(ctx, doc, SendTo::Into(deck));
        let n = slide_titles(self, deck).len();
        check(r.is_ok() && n == 5, format!("docs → open slides adds two (no second title slide): {r:?}, {n} slides"));
        // Slides → Docs.
        let r = self.convert_tab(ctx, deck, SendTo::New(Kind::Word));
        let back = self.entries.last().map(|e| e.id).unwrap_or(0);
        let b = blocks(self, back);
        let styled: Vec<(String, String)> = b.iter().map(|x| (x["style"].as_str().unwrap_or("").to_string(), x["text"].as_str().unwrap_or("").to_string())).collect();
        eprintln!("sendto-selftest: slides → docs gave {styled:?}");
        check(r.is_ok() && styled.iter().any(|(s, t)| s.starts_with("Heading") && t == "Where we go"), format!("slides → new docs has the heading: {r:?}"));
        check(styled.iter().any(|(_, t)| t == "Bus leaves at 8"), "slides → new docs has the bullet".into());
        // Sheets → Docs and Slides, as tables.
        let grid = self.push_tab(Kind::Grid, Tab::create(Kind::Grid));
        let r = self.tab_run(ctx, grid, "edit.paste", serde_json::json!({ "at": "A1", "text": "Item\tPrice\nTicket\t12\nLunch\t8" }));
        check(r.is_ok(), format!("filling the test sheet: {r:?}"));
        let r = self.convert_tab(ctx, grid, SendTo::New(Kind::Word));
        let gdoc = self.entries.last().map(|e| e.id).unwrap_or(0);
        let table = blocks(self, gdoc).into_iter().find(|x| x["type"] == "table");
        check(r.is_ok() && table.as_ref().is_some_and(|t| t["rows"] == 3 && t["cols"] == 2), format!("sheets → new docs as a 3×2 table: {r:?} {table:?}"));
        let r = self.convert_tab(ctx, grid, SendTo::New(Kind::Deck));
        let gdeck = self.entries.last().map(|e| e.id).unwrap_or(0);
        let slide = self.tab_run(ctx, gdeck, "slide.inspect", serde_json::json!({ "index": 0 })).unwrap_or_default();
        let has_table = slide.get("shapes").and_then(serde_json::Value::as_array).is_some_and(|a| a.iter().any(|s| s.get("table").is_some()));
        eprintln!("sendto-selftest: sheets → slides slide 0: {}", slide.to_string().chars().take(600).collect::<String>());
        check(r.is_ok() && has_table && slide_titles(self, gdeck).len() == 1, format!("sheets → new slides as one table slide: {r:?}"));
        // An empty sheet opens nothing.
        let empty = self.push_tab(Kind::Grid, Tab::create(Kind::Grid));
        let n_before = self.entries.len();
        let r = self.convert_tab(ctx, empty, SendTo::New(Kind::Word));
        check(r.is_err() && self.entries.len() == n_before, format!("an empty sheet is refused without a tab: {r:?}"));
        // Graph → Docs and Slides, as a picture.
        let graph = self.new_tab(Kind::Graph);
        if let Some(Tab::Graph(a)) = self.entry_mut(graph).map(|e| &mut e.tab) {
            let _ = a.control("engine.execute", serde_json::json!({"command": "addItem", "params": {"id": "a1", "kind": "equation", "latex": "y=x^2"}}));
        }
        let r = self.convert_tab(ctx, graph, SendTo::New(Kind::Word));
        let pics = match self.entries.last().map(|e| &e.tab) {
            Some(Tab::Word(a)) => a.session.doc.media.len(),
            _ => 0,
        };
        check(r.is_ok() && pics == 1, format!("graph → new docs: {r:?}, {pics} picture(s)"));
        let r = self.convert_tab(ctx, graph, SendTo::New(Kind::Deck));
        let gd = self.entries.last().map(|e| e.id).unwrap_or(0);
        let slide = self.tab_run(ctx, gd, "slide.inspect", serde_json::json!({ "index": 0 })).unwrap_or_default();
        let pic = slide.get("shapes").and_then(serde_json::Value::as_array).is_some_and(|a| a.iter().any(|s| s.get("kind").and_then(serde_json::Value::as_str).is_some_and(|k| k.to_ascii_lowercase().contains("pic"))));
        check(r.is_ok() && pic, format!("graph → new slides: {r:?} {}", slide.to_string().chars().take(300).collect::<String>()));
        // PDFs of each, opened in PDF tabs; the source keeps its own path.
        for (id, what) in [(doc, "docs"), (grid, "sheets"), (deck, "slides")] {
            let path = format!("{dir}/{what}.pdf");
            let _ = std::fs::remove_file(&path);
            let r = self.send_pdf(ctx, id, &path);
            let head = std::fs::read(&path).map(|b| b.starts_with(b"%PDF-")).unwrap_or(false);
            let opened = self.entries.iter().any(|e| e.kind == Kind::Pdf && e.path.as_deref() == Some(canonical_path(&path).as_str()));
            let kept = self.entries.iter().find(|e| e.id == id).is_some_and(|e| e.tab.current_path().is_none());
            check(r.is_ok() && head && opened && kept, format!("{what} → pdf: {r:?}, file {head}, tab {opened}, source untouched {kept}"));
            // Again onto the PDF now open: refused.
            let again = self.send_pdf(ctx, id, &path);
            check(again.is_err(), format!("{what} → the open pdf again is refused: {again:?}"));
        }
        eprintln!("sendto-selftest: {}", if bad.is_empty() { "passed".to_string() } else { format!("FAILED {}", bad.len()) });
        std::process::exit(if bad.is_empty() { 0 } else { 1 });
    }

    /// `OFFICECRAFT_SELFTEST_GRAPH=<dir>`: draw a graph into a new document, double-click the
    /// picture, edit the graph in its tab, update the picture, and write both PNGs to `dir`.
    fn graph_selftest(&mut self, ctx: &egui::Context, dir: &str) {
        let fail = |m: String| -> ! {
            eprintln!("graph-selftest: FAILED: {m}");
            std::process::exit(1)
        };
        let picture = |suite: &mut Suite, id: u64| -> Vec<u8> {
            match suite.entry_mut(id).map(|e| &mut e.tab) {
                Some(Tab::Word(a)) => selected_picture_bytes(&a.session).map(|b| b.to_vec()).unwrap_or_default(),
                _ => Vec::new(),
            }
        };
        let doc = self.new_tab(Kind::Word);
        // Two graphs in a row, the way two create_graph calls in one turn arrive: the second must
        // land after the first, not replace it.
        for args in [serde_json::json!({"expressions": ["y=x^2", "y=2x+1"]}), serde_json::json!({"expressions": ["y=sin(x)"]})] {
            if let Err(er) = self.insert_graph_picture(doc, &args, ctx) {
                fail(format!("insert: {er}"));
            }
        }
        let pictures = match self.entry_mut(doc).map(|e| &mut e.tab) {
            Some(Tab::Word(a)) => a.session.doc.media.len(),
            _ => 0,
        };
        if pictures != 2 {
            fail(format!("two graphs were inserted but the document holds {pictures} picture(s)"));
        }
        // Select the first one again, as a click on it would.
        {
            let e = self.entry_mut(doc).unwrap();
            let mut host = TabHost { tab: &mut e.tab, client: &mut e.pdf_client, ctx };
            let _ = ai::Host::call(&mut host, "engine.execute", serde_json::json!({"command": "select.range", "params": {"anchor": {"story": "body", "path": [0], "off": 0}, "focus": {"story": "body", "path": [0], "off": 3}}}));
            if let Tab::Word(a) = &mut e.tab {
                let _ = a;
                FORCE_PICTURE_EDIT.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        let first = picture(self, doc);
        let _ = std::fs::write(format!("{dir}/first.png"), &first);
        self.open_double_clicked_graphs(ctx);
        let Some(g) = self.entries.iter().find(|e| e.graph_for == Some(doc)).map(|e| e.id) else { fail("double-click opened no graph tab".into()) };
        if let Some(Tab::Graph(a)) = self.entry_mut(g).map(|e| &mut e.tab) {
            let r = a.control("engine.execute", serde_json::json!({"command": "addItem", "params": {"id": "a9", "kind": "equation", "latex": "y=-x+5"}}));
            if let Err(er) = r {
                fail(format!("edit: {er}"));
            }
        }
        self.update_graph_picture(g, ctx);
        if let Some(n) = &self.notice {
            fail(n.clone());
        }
        let second = picture(self, doc);
        let _ = std::fs::write(format!("{dir}/second.png"), &second);
        let same_tab_back = self.view == View::Tab(doc) && self.entries.iter().all(|e| e.graph_for.is_none());
        let items = |p: &[u8]| graphpic::extract(p).map(|j| j.matches("\"latex\"").count()).unwrap_or(0);
        eprintln!("graph-selftest: first {} bytes ({} items), second {} bytes ({} items), back in document: {same_tab_back}", first.len(), items(&first), second.len(), items(&second));
        std::process::exit(if second != first && items(&second) == items(&first) + 1 && same_tab_back { 0 } else { 1 });
    }

    /// Presentations a document's assistant asked for: each opens in a new Slides tab and is
    /// built there, while the document (and its chat) stays in front.
    fn open_requested_decks(&mut self, ctx: &egui::Context) {
        let Some((src, decks)) = self
            .entries
            .iter_mut()
            .find_map(|e| {
                let d = e.chat.take_decks();
                (!d.is_empty()).then_some((e.id, d))
            })
        else {
            return;
        };
        for args in decks {
            let keep = self.view;
            let id = self.new_tab(Kind::Deck);
            self.view = keep;
            let title = args.get("title").and_then(serde_json::Value::as_str).unwrap_or("").trim().to_string();
            let built = match self.entry_mut(id) {
                Some(e) => {
                    let mut host = TabHost { tab: &mut e.tab, client: &mut e.pdf_client, ctx };
                    let r = ai::build_slides(&mut host, &args);
                    if r.is_ok() && !title.is_empty() {
                        e.name = title.clone();
                    }
                    r
                }
                None => Err("tab missing".to_string()),
            };
            let note = match built {
                Ok(v) => format!("Opened a new Slides tab with {} slides.", v.get("slides_added").and_then(serde_json::Value::as_u64).unwrap_or(0)),
                Err(e) => format!("The presentation could not be built: {e}"),
            };
            if let Some(e) = self.entry_mut(src) {
                e.chat.note(note);
            }
        }
        ctx.request_repaint();
    }

    fn drive_ai_selftest(&mut self, ctx: &egui::Context, prompt: String, started: Option<std::time::Instant>) {
        ctx.request_repaint();
        let Some(t0) = started else {
            // `OFFICECRAFT_SELFTEST_AI_APP=docs|sheets|slides` picks the app (default docs).
            let kind = match std::env::var("OFFICECRAFT_SELFTEST_AI_APP").as_deref() {
                Ok("sheets") => Kind::Grid,
                Ok("slides") => Kind::Deck,
                _ => Kind::Word,
            };
            // `OFFICECRAFT_SELFTEST_AI_FILE=<path>` opens that file instead of a blank tab.
            let kind = match std::env::var("OFFICECRAFT_SELFTEST_AI_FILE") {
                Ok(path) => {
                    self.open_file(ctx, &path);
                    match self.entries.first() {
                        Some(e) => e.kind,
                        None => {
                            eprintln!("ai-selftest: could not open {path}");
                            std::process::exit(1);
                        }
                    }
                }
                Err(_) => kind,
            };
            let id = if self.entries.is_empty() { self.new_tab(kind) } else { self.entries[0].id };
            let cfg = self.ai_cfg.clone();
            if let Some(e) = self.entry_mut(id) {
                e.chat.open = true;
                e.chat.ask_text(prompt.clone(), ai_app(kind), &cfg, ctx);
            }
            eprintln!("ai-selftest: model {} at {}", cfg.model, cfg.base_url);
            self.ai_selftest = Some((prompt, Some(std::time::Instant::now())));
            return;
        };
        let timed_out = t0.elapsed().as_secs() > 240;
        let ctx2 = ctx.clone();
        let Some(e) = self.entries.first_mut() else { return };
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
        for d in self.entries.iter_mut().skip(1) {
            let mut host = TabHost { tab: &mut d.tab, client: &mut d.pdf_client, ctx: &ctx2 };
            if let ai::CallResult::Done(r) = ai::Host::call(&mut host, "document.inspect", serde_json::json!({"text": true})) {
                eprintln!("---- tab '{}' ----\n{}", d.name, r.map(|v| v.to_string()).unwrap_or_else(|e| e).chars().take(1500).collect::<String>());
            }
        }
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

    /// Follows Save As: a tab whose app now saves to another file takes that file's path and name,
    /// so the tab title, the open-file check and the saved session all point at the new file.
    fn sync_paths(&mut self) {
        let mut moved = Vec::new();
        for e in &mut self.entries {
            let Some(now) = e.tab.current_path() else { continue };
            // Only a change the app made is worth resolving (resolving touches the disk).
            if e.seen_path.as_deref() == Some(now.as_str()) {
                continue;
            }
            e.seen_path = Some(now.clone());
            // A page has no paths, only names: retitle the tab, keep it out of Recent.
            if cfg!(target_arch = "wasm32") {
                let name = file_name_of(&now);
                if e.name != name {
                    e.name = name;
                }
                continue;
            }
            if e.path.as_deref() == Some(now.as_str()) {
                continue;
            }
            let now = canonical_path(&now);
            if e.path.as_deref() != Some(now.as_str()) {
                e.name = file_name_of(&now);
                e.path = Some(now.clone());
                moved.push(now);
            }
        }
        for p in moved {
            self.recent.retain(|r| *r != p);
            self.recent.insert(0, p);
            self.recent.truncate(MAX_RECENT);
            save_recent(&self.recent);
        }
    }

    /// Launch: brings back documents a previous run left unsaved. DeckCraft reopens its copies
    /// itself; a Word tab offers WordCraft's on its recovery page.
    #[cfg(not(target_arch = "wasm32"))]
    fn reopen_recovered(&mut self) {
        // DeckCraft opens what is unclaimed (another running instance keeps its own).
        let mut session = deck_session(deckcraft_engine::Session::new());
        if let Ok(v) = session.execute("file.recovery.open", &serde_json::json!({}))
            && let Some(n) = v.get("opened").and_then(|o| o.as_array()).map(Vec::len).filter(|n| *n > 0)
        {
            let mut app = deckcraft_ui_egui::SlideApp::new(session, deck_services());
            app.set_status(format!("Recovered {n} unsaved presentation{} from the last session", if n == 1 { "" } else { "s" }));
            self.push_tab(Kind::Deck, Tab::Deck(Box::new(app)));
        }
        // WordCraft: a session reads its recovery folder on a worker; `poll_word_recovery` opens
        // a Docs tab once it reports copies no other instance holds.
        if let Some((dir, ctx)) = recovery_dir("word") {
            let mut probe = wordcraft_engine::Session::new(wordcraft_doc::Document::new());
            if probe.enable_recovery(dir, std::sync::Arc::new(move || ctx.request_repaint())).is_ok() {
                self.word_probe = Some(probe);
            }
        }
    }

    /// Launch: once WordCraft's recovery folder is read, a Docs tab offers what it holds.
    #[cfg(not(target_arch = "wasm32"))]
    fn poll_word_recovery(&mut self) {
        let Some(probe) = &mut self.word_probe else { return };
        probe.recovery_tick();
        if probe.recovery_error().is_some() {
            self.word_probe = None;
        } else if probe.recovery_ready() {
            let waiting = !probe.recovery_documents().is_empty();
            self.word_probe = None;
            if waiting {
                self.push_tab(Kind::Word, Tab::create(Kind::Word));
            }
        }
    }

/// Applies what Home's rows, path and sidebar asked for this frame (desktop).
    #[cfg(not(target_arch = "wasm32"))]
    fn apply_home(&mut self, ctx: &egui::Context, acts: HomeActions, crumb_go: Option<Option<String>>, new_folder: bool, reorder: Option<(usize, usize)>, open_trash: bool) {
        if let Some(dir) = crumb_go {
            self.browse(ctx, dir);
        }
        if let Some(d) = acts.enter
            && self.folder_sel.is_some()
        {
            self.browse(ctx, Some(d));
        }
        if new_folder && let Some(dir) = self.shown_dir() {
            self.file_op = Some(FileOp::NewFolder { dir, name: "New folder".into(), error: None });
        }
        if let Some((from, to)) = reorder
            && from < self.folders.len()
        {
            let sel = self.folder_sel.and_then(|i| self.folders.get(i).cloned());
            let f = self.folders.remove(from);
            let to = if to > from { to - 1 } else { to }.min(self.folders.len());
            self.folders.insert(to, f);
            self.folder_sel = sel.and_then(|s| self.folders.iter().position(|x| *x == s));
            save_folders(&self.folders);
            self.folder_counts.clear();
        }
        if open_trash {
            self.own_trash = files::own_trash_list();
            self.show_trash = true;
        }
        if let Some(op) = acts.op {
            match op {
                FileOp::Trash { path, is_dir, .. } => {
                    let open = self.tabs_under(&path);
                    if open.is_empty() && !is_dir {
                        self.do_trash(ctx, &path);
                    } else {
                        self.file_op = Some(FileOp::Trash { path, open, is_dir });
                    }
                }
                other => self.file_op = Some(other),
            }
        }
        for (src, dest) in acts.moves {
            if let Some(blocked) = self.cannot_follow(&src) {
                self.notice = Some(format!("Close {blocked} first: it can't follow a move."));
                continue;
            }
            match files::move_into(std::path::Path::new(&src), std::path::Path::new(&dest)) {
                Ok(new) if new.to_string_lossy() == src => {}
                Ok(new) => {
                    let new = new.to_string_lossy().to_string();
                    self.retarget(&src, &new);
                    self.notice = None;
                    self.status = Some(format!("Moved {} to {}", file_name_of(&src), file_name_of(&dest)));
                }
                Err(e) => self.notice = Some(e),
            }
            self.refresh_home(ctx);
        }
        if let Some(p) = acts.reveal {
            reveal_in_file_manager(&p);
        }
        if let Some(p) = acts.forget_recent {
            self.recent.retain(|r| *r != p);
            save_recent(&self.recent);
        }
    }

    /// Open tabs whose file is `path` or inside it.
    #[cfg(not(target_arch = "wasm32"))]
    fn tabs_under(&self, path: &str) -> Vec<u64> {
        let path = canonical_path(path);
        self.entries.iter().filter(|e| e.path.as_deref().is_some_and(|p| std::path::Path::new(p).starts_with(&path))).map(|e| e.id).collect()
    }

    /// The name of an open tab inside `path` whose app can't be re-pointed at a new path.
    #[cfg(not(target_arch = "wasm32"))]
    fn cannot_follow(&self, path: &str) -> Option<String> {
        let ids = self.tabs_under(path);
        self.entries.iter().find(|e| ids.contains(&e.id) && !matches!(e.kind, Kind::Word | Kind::Grid | Kind::Deck)).map(|e| e.name.clone())
    }

    /// `old` was renamed or moved to `new`: open tabs, Recent, sidebar folders and the shown
    /// folder that point at it (or inside it) follow.
    #[cfg(not(target_arch = "wasm32"))]
    fn retarget(&mut self, old: &str, new: &str) {
        // `old` is gone now: resolve its folder (still there) to match the open tabs' spelling.
        let resolved = |p: &str| -> String {
            let path = std::path::Path::new(p);
            match (path.parent(), path.file_name()) {
                (Some(dir), Some(name)) => std::path::Path::new(&canonical_path(&dir.to_string_lossy())).join(name).to_string_lossy().to_string(),
                _ => p.to_string(),
            }
        };
        let (old_c, new_c) = (resolved(old), canonical_path(new));
        if old_c != old {
            self.retarget(&old_c, &new_c);
        }
        let swap = |p: &str| -> Option<String> {
            let rel = std::path::Path::new(p).strip_prefix(old).ok()?;
            // `join("")` would leave a trailing separator on the item itself.
            Some(if rel.as_os_str().is_empty() { new.to_string() } else { std::path::Path::new(new).join(rel).to_string_lossy().to_string() })
        };
        for e in &mut self.entries {
            if let Some(p) = e.path.clone()
                && let Some(n) = swap(&p)
                && e.tab.set_path(&n)
            {
                e.name = file_name_of(&n);
                e.path = Some(n);
            }
        }
        for r in &mut self.recent {
            if let Some(n) = swap(r) {
                *r = n;
            }
        }
        save_recent(&self.recent);
        let mut folders_changed = false;
        for f in &mut self.folders {
            if let Some(n) = swap(f) {
                *f = n;
                folders_changed = true;
            }
        }
        if folders_changed {
            save_folders(&self.folders);
        }
        if let Some(c) = self.cwd.clone()
            && let Some(n) = swap(&c)
        {
            self.cwd = Some(n);
        }
    }

    /// Moves `path` to the trash, after closing the tabs that have it open.
    #[cfg(not(target_arch = "wasm32"))]
    fn do_trash(&mut self, ctx: &egui::Context, path: &str) {
        let open = self.tabs_under(path);
        match files::trash(std::path::Path::new(path)) {
            Ok(place) => {
                // Only now that the file is safely in a trash: its tabs close (unsaved changes
                // were confirmed away in the dialog).
                for id in open {
                    self.close(id);
                }
                let name = file_name_of(path);
                self.notice = None;
                self.status = Some(match place {
                    files::Trashed::System => format!("Moved {name} to the Trash."),
                    files::Trashed::Own => format!("Moved {name} to OfficeCraft Trash (this folder has no system trash)."),
                });
                self.recent.retain(|r| !std::path::Path::new(r).starts_with(path));
                save_recent(&self.recent);
                // A trashed sidebar folder leaves the sidebar.
                let before = self.folders.len();
                let sel = self.folder_sel.and_then(|i| self.folders.get(i).cloned());
                self.folders.retain(|f| !std::path::Path::new(f).starts_with(path));
                if self.folders.len() != before {
                    save_folders(&self.folders);
                    self.folder_sel = sel.and_then(|s| self.folders.iter().position(|x| *x == s));
                    if self.folder_sel.is_none() {
                        self.cwd = None;
                    }
                    self.folder_counts.clear();
                }
                if self.cwd.as_deref().is_some_and(|c| std::path::Path::new(c).starts_with(path)) {
                    self.cwd = std::path::Path::new(path).parent().map(|p| p.to_string_lossy().to_string());
                }
                self.own_trash = files::own_trash_list();
            }
            Err(e) => self.notice = Some(format!("Couldn't move {} to the Trash: {e}", file_name_of(path))),
        }
        self.refresh_home(ctx);
    }

    /// Lists the shown folder and the sidebar counts again after a change on disk.
    #[cfg(not(target_arch = "wasm32"))]
    fn refresh_home(&mut self, ctx: &egui::Context) {
        // `recount` lists the sidebar folders and the subfolder shown.
        self.recount(ctx);
    }

    /// The rename / new folder / trash dialog, when one is open.
    #[cfg(not(target_arch = "wasm32"))]
    fn file_op_ui(&mut self, ctx: &egui::Context) {
        let Some(op) = &mut self.file_op else { return };
        let mut done = false;
        let mut go = false;
        let title = match op {
            FileOp::Rename { .. } => "Rename",
            FileOp::NewFolder { .. } => "New folder",
            FileOp::Trash { is_dir: true, .. } => "Move folder to Trash?",
            FileOp::Trash { .. } => "Move to Trash?",
        };
        egui::Window::new(title).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            *ui.visuals_mut() = home_visuals();
            match op {
                FileOp::Rename { name, error, .. } | FileOp::NewFolder { name, error, .. } => {
                    let edit = ui.add(egui::TextEdit::singleline(name).desired_width(320.0));
                    if !self.file_op_focused {
                        edit.request_focus();
                        self.file_op_focused = true;
                    }
                    if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        go = true;
                    }
                    if let Some(e) = error {
                        ui.colored_label(egui::Color32::from_rgb(190, 40, 50), e.as_str());
                    }
                }
                FileOp::Trash { path, open, is_dir } => {
                    let name = file_name_of(path);
                    ui.label(if *is_dir { format!("Move the folder {name} and everything in it to the Trash?") } else { format!("Move {name} to the Trash?") });
                    if *is_dir {
                        ui.label(egui::RichText::new(path.as_str()).size(12.0).color(MUTED));
                    }
                    if !open.is_empty() {
                        let dirty = self.entries.iter().any(|e| open.contains(&e.id) && e.tab.is_dirty());
                        ui.label(if dirty { "It is open with unsaved changes. Its tab will close and those changes will be lost." } else { "It is open. Its tab will close." });
                    }
                    ui.label(egui::RichText::new("You can restore it from the Trash.").size(12.0).color(MUTED));
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let verb = match op {
                    FileOp::Rename { .. } => "Rename",
                    FileOp::NewFolder { .. } => "Create",
                    FileOp::Trash { .. } => "Move to Trash",
                };
                if ui.button(verb).clicked() {
                    go = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    done = true;
                }
            });
        });
        if go || done {
            self.file_op_focused = false;
        }
        if go {
            match self.file_op.take() {
                Some(FileOp::Rename { path, name, .. }) => {
                    if let Some(blocked) = self.cannot_follow(&path) {
                        self.file_op = Some(FileOp::Rename { path, name, error: Some(format!("Close {blocked} first: it can't follow a rename.")) });
                    } else {
                        match files::rename(std::path::Path::new(&path), &name) {
                            Ok(new) => {
                                self.retarget(&path, &new.to_string_lossy());
                                self.refresh_home(ctx);
                            }
                            Err(e) => self.file_op = Some(FileOp::Rename { path, name, error: Some(e) }),
                        }
                    }
                }
                Some(FileOp::NewFolder { dir, name, .. }) => match files::new_folder(std::path::Path::new(&dir), &name) {
                    Ok(_) => self.refresh_home(ctx),
                    Err(e) => self.file_op = Some(FileOp::NewFolder { dir, name, error: Some(e) }),
                },
                Some(FileOp::Trash { path, .. }) => self.do_trash(ctx, &path),
                None => {}
            }
        } else if done {
            self.file_op = None;
        }
    }

    /// OfficeCraft's own trash: what was trashed where there is no system trash, with Restore.
    #[cfg(not(target_arch = "wasm32"))]
    fn trash_list(&mut self, ui: &mut egui::Ui) {
        let mut restore: Option<files::OwnItem> = None;
        let mut purge: Option<files::OwnItem> = None;
        if self.own_trash.is_empty() {
            ui.add_space(24.0);
            ui.label(egui::RichText::new("OfficeCraft Trash is empty.").color(MUTED));
            return;
        }
        framed().show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let w = ui.available_width();
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                for item in &self.own_trash {
                    ui.horizontal(|ui| {
                        ui.set_min_height(45.0);
                        ui.add_space(16.0);
                        ui.vertical(|ui| {
                            ui.add_space(5.0);
                            ui.label(b(&file_name_of(&item.original.to_string_lossy()), 14.0));
                            let from = item.original.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                            let when = item.deleted.map(ago).unwrap_or_default();
                            let what = if item.is_dir { "Folder from" } else { "From" };
                            ui.label(egui::RichText::new(format!("{what} {from}  ·  {when}")).size(12.0).color(MUTED));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(12.0);
                            if ui.button(egui::RichText::new("Delete forever").color(egui::Color32::from_rgb(180, 40, 50))).clicked() {
                                purge = Some(item.clone());
                            }
                            if ui.button("Restore").clicked() {
                                restore = Some(item.clone());
                            }
                        });
                    });
                    let y = ui.cursor().top();
                    ui.painter().hline(ui.min_rect().left()..=ui.min_rect().left() + w, y, egui::Stroke::new(1.0, HAIRLINE));
                }
            });
        });
        if let Some(item) = restore {
            match files::restore(&item) {
                Ok(p) => self.status = Some(format!("Restored {}", p.display())),
                Err(e) => self.notice = Some(e),
            }
            self.own_trash = files::own_trash_list();
        }
        if let Some(item) = purge {
            self.purge_confirm = Some(item);
        }
        if let Some(item) = self.purge_confirm.clone() {
            let mut answer = None;
            egui::Window::new("Delete forever?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
                *ui.visuals_mut() = home_visuals();
                ui.label(format!("Delete {} for good? It can't be restored afterwards.", file_name_of(&item.original.to_string_lossy())));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(egui::RichText::new("Delete forever").color(egui::Color32::from_rgb(180, 40, 50))).clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        answer = Some(false);
                    }
                });
            });
            if let Some(yes) = answer {
                self.purge_confirm = None;
                if yes {
                    if let Err(e) = files::purge(&item) {
                        self.notice = Some(e);
                    }
                    self.own_trash = files::own_trash_list();
                }
            }
        }
        if self.own_trash.is_empty() {
            self.show_trash = false;
        }
    }

    /// The Equations tab's Insert into…: it lists the open Docs and Slides tabs, and an insert it
    /// asked for goes in here, a real equation in Docs, a picture in Slides (DeckCraft has no
    /// equation object). The document's tab comes to the front.
    /// Claude Code's tool calls (MCP), run between frames on the open tabs.
    #[cfg(not(target_arch = "wasm32"))]
    fn serve_mcp(&mut self, ctx: &egui::Context) {
        // Settings keeps the server on for sessions the user starts: start it once at launch.
        static TRIED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if self.ai_cfg.mcp_listen && mcp::port().is_none() && !TRIED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            if let Err(e) = mcp::start(ctx) {
                self.toast = Some((format!("Couldn't start the Claude Code connection: {e}"), true, ctx.input(|i| i.time)));
            }
        }
        for call in mcp::take_calls() {
            let reply = self.mcp_tool(ctx, &call.tool, &call.args);
            let _ = call.reply.send(reply);
        }
    }

    /// One MCP tool call (see `mcp::tool_definitions`).
    #[cfg(not(target_arch = "wasm32"))]
    fn mcp_tool(&mut self, ctx: &egui::Context, tool: &str, args: &serde_json::Value) -> mcp::Reply {
        use mcp::Reply::{Later, Now};
        use serde_json::json;
        let tab = args.get("tab").and_then(serde_json::Value::as_u64);
        let s = |k: &str| args.get(k).and_then(serde_json::Value::as_str).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
        let no_tab = |id: Option<u64>| match id {
            Some(id) => format!("there is no tab {id} (see list_tabs)"),
            None => "`tab` is required (see list_tabs)".to_string(),
        };
        let app_name = |k: Kind| match k {
            Kind::Word => "docs",
            Kind::Grid => "sheets",
            Kind::Deck => "slides",
            Kind::Pdf => "pdf",
            Kind::Graph => "graph",
            Kind::Equation => "equations",
            Kind::Vershion => "vershion",
        };
        match tool {
            "list_tabs" => {
                let tabs: Vec<serde_json::Value> = self
                    .entries
                    .iter()
                    .map(|e| json!({"tab": e.id, "app": app_name(e.kind), "name": e.name, "path": e.path, "unsaved": e.tab.is_dirty(), "front": self.view == View::Tab(e.id), "editable": e.kind.has_assistant()}))
                    .collect();
                Now(Ok(json!({ "tabs": tabs })))
            }
            "app_tools" => match self.entries.iter().find(|e| Some(e.id) == tab) {
                Some(e) if e.kind.has_assistant() => Now(Ok(json!(ai::agent_tools(ai_app(e.kind))))),
                Some(e) => Now(Err(format!("the {} tab has no tools", e.kind.label()))),
                None => Now(Err(no_tab(tab))),
            },
            "use_tool" => {
                let Some(name) = s("tool") else { return Now(Err("`tool` is required (see app_tools)".into())) };
                let arguments = args.get("arguments").cloned().unwrap_or_else(|| json!({}));
                let Some(e) = tab.and_then(|id| self.entry_mut(id)) else { return Now(Err(no_tab(tab))) };
                if !e.kind.has_assistant() {
                    return Now(Err(format!("the {} tab has no tools", e.kind.label())));
                }
                let app = ai_app(e.kind);
                let mut host = TabHost { tab: &mut e.tab, client: &mut e.pdf_client, ctx };
                ctx.request_repaint();
                match ai::agent_call(&mut host, app, &name, &arguments) {
                    ai::CallResult::Done(r) => Now(r),
                    ai::CallResult::Pending(rx) => Later(rx),
                }
            }
            "new_tab" => {
                let kind = match s("app").as_deref() {
                    Some("docs" | "doc" | "document" | "word") => Kind::Word,
                    Some("sheets" | "sheet" | "spreadsheet" | "workbook") => Kind::Grid,
                    Some("slides" | "slide" | "presentation" | "deck") => Kind::Deck,
                    Some("graph") => Kind::Graph,
                    other => return Now(Err(format!("app must be docs, sheets, slides or graph, not {other:?}"))),
                };
                let id = self.new_tab(kind);
                if let Some(n) = s("name")
                    && let Some(e) = self.entry_mut(id)
                {
                    e.name = n;
                }
                ctx.request_repaint();
                Now(Ok(json!({ "tab": id })))
            }
            "open_file" => {
                let Some(path) = s("path") else { return Now(Err("`path` is required".into())) };
                self.notice = None;
                self.open_file(ctx, &path);
                let canonical = canonical_path(&path);
                match self.entries.iter().find(|e| e.path.as_deref() == Some(canonical.as_str())) {
                    Some(e) => Now(Ok(json!({ "tab": e.id, "app": app_name(e.kind), "name": e.name }))),
                    None => Now(Err(self.notice.take().unwrap_or_else(|| format!("could not open {path}")))),
                }
            }
            "show_tab" => match tab.filter(|id| self.entries.iter().any(|e| e.id == *id)) {
                Some(id) => {
                    self.view = View::Tab(id);
                    ctx.request_repaint();
                    Now(Ok(json!({ "shown": id })))
                }
                None => Now(Err(no_tab(tab))),
            },
            "send_to" => {
                let Some(from) = tab.filter(|id| self.entries.iter().any(|e| e.id == *id)) else { return Now(Err(no_tab(tab))) };
                let to = s("to").unwrap_or_default();
                if to == "pdf" {
                    let Some(path) = s("pdf_path") else { return Now(Err("to=pdf needs `pdf_path`".into())) };
                    let r = self.send_pdf(ctx, from, &path);
                    let opened = self.entries.iter().find(|e| e.path.as_deref() == Some(canonical_path(&path).as_str())).map(|e| e.id);
                    return Now(r.map(|said| json!({ "said": said, "tab": opened })));
                }
                let kind = match to.as_str() {
                    "docs" => Kind::Word,
                    "slides" => Kind::Deck,
                    other => return Now(Err(format!("to must be docs, slides or pdf, not {other:?}"))),
                };
                let dest = match args.get("into").and_then(serde_json::Value::as_u64) {
                    Some(id) => SendTo::Into(id),
                    None => SendTo::New(kind),
                };
                if let SendTo::Into(id) = dest
                    && !self.entries.iter().any(|e| e.id == id && e.kind == kind)
                {
                    return Now(Err(format!("tab {id} is not an open {} tab", kind.label())));
                }
                let r = self.convert_tab(ctx, from, dest);
                let front = match self.view {
                    View::Tab(id) => Some(id),
                    View::Home => None,
                };
                Now(r.map(|said| json!({ "said": said, "tab": front })))
            }
            "save" => {
                let Some(e) = tab.and_then(|id| self.entries.iter().find(|e| e.id == id)) else { return Now(Err(no_tab(tab))) };
                let id = e.id;
                if !matches!(e.kind, Kind::Word | Kind::Grid | Kind::Deck) {
                    return Now(Err(format!("save a {} tab with its own tools", e.kind.label())));
                }
                let r = match s("path") {
                    Some(path) => {
                        let canonical = canonical_path(&path);
                        if self.entries.iter().any(|o| o.id != id && o.path.as_deref() == Some(canonical.as_str())) {
                            return Now(Err(format!("{} is open in another tab", file_name_of(&path))));
                        }
                        self.tab_run(ctx, id, "file.saveAs", json!({ "path": path }))
                    }
                    None if e.tab.current_path().is_none() => return Now(Err("this document has never been saved: give a `path`".into())),
                    None => self.tab_run(ctx, id, "file.save", json!({})),
                };
                self.sync_paths();
                ctx.request_repaint();
                Now(r.map(|_| json!({ "saved": self.entries.iter().find(|e| e.id == id).and_then(|e| e.path.clone()) })))
            }
            other => Now(Err(format!("unknown tool {other}"))),
        }
    }

    /// One control call on tab `id`, through the same surface the assistant uses.
    fn tab_call(&mut self, ctx: &egui::Context, id: u64, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
        let e = self.entry_mut(id).ok_or_else(|| "that tab was closed".to_string())?;
        let mut host = TabHost { tab: &mut e.tab, client: &mut e.pdf_client, ctx };
        match ai::Host::call(&mut host, method, params) {
            ai::CallResult::Done(r) => r,
            ai::CallResult::Pending(_) => Err("the app did not answer".to_string()),
        }
    }

    /// One app command on tab `id` (`engine.execute`).
    fn tab_run(&mut self, ctx: &egui::Context, id: u64, command: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
        self.tab_call(ctx, id, "engine.execute", serde_json::json!({ "command": command, "params": params }))
    }

    /// Where tab `from` can be sent, as (menu label, destination): a new tab of another app, an
    /// open one, or a PDF.
    fn send_targets(entries: &[Entry], from: u64) -> Vec<(String, SendTo)> {
        let Some(src) = entries.iter().find(|e| e.id == from) else { return Vec::new() };
        let into = |kind: Kind, what: &str| -> Vec<(String, SendTo)> {
            entries.iter().filter(|e| e.kind == kind && e.id != from).map(|e| (format!("{}: {what}", e.name), SendTo::Into(e.id))).collect()
        };
        let mut out: Vec<(String, SendTo)> = Vec::new();
        match src.kind {
            Kind::Word => {
                out.push(("New Slides from the outline".into(), SendTo::New(Kind::Deck)));
                out.extend(into(Kind::Deck, "add the outline as slides"));
            }
            Kind::Grid => {
                out.push(("New Docs with the table".into(), SendTo::New(Kind::Word)));
                out.extend(into(Kind::Word, "insert the table"));
                out.push(("New Slides with the table".into(), SendTo::New(Kind::Deck)));
                out.extend(into(Kind::Deck, "add a slide with the table"));
            }
            Kind::Deck => {
                out.push(("New Docs from the slides".into(), SendTo::New(Kind::Word)));
                out.extend(into(Kind::Word, "insert the slides' text"));
            }
            Kind::Graph => {
                out.push(("New Docs with the graph".into(), SendTo::New(Kind::Word)));
                out.extend(into(Kind::Word, "insert the graph"));
                out.push(("New Slides with the graph".into(), SendTo::New(Kind::Deck)));
                out.extend(into(Kind::Deck, "add a slide with the graph"));
            }
            _ => {}
        }
        #[cfg(not(target_arch = "wasm32"))]
        if matches!(src.kind, Kind::Word | Kind::Grid | Kind::Deck) {
            out.push(("PDF…".into(), SendTo::Pdf));
        }
        out
    }

    /// Send to…: makes tab `from`'s document into `to` and shows the result, or says why not.
    fn send_to(&mut self, ctx: &egui::Context, from: u64, to: SendTo) {
        let now = ctx.input(|i| i.time);
        match self.convert_tab(ctx, from, to) {
            Ok(None) => {}
            Ok(Some(said)) => self.toast = Some((said, false, now)),
            Err(e) => self.toast = Some((format!("Couldn't send: {e}"), true, now)),
        }
        ctx.request_repaint();
    }

    /// New from prompt, on Home: a description, the app for it, and Make. `Some` when Make (or
    /// Enter) was pressed with something written.
    fn make_from_prompt_ui(&mut self, ui: &mut egui::Ui) -> Option<(Kind, String)> {
        let mut go = false;
        let picked = self.make_kind.unwrap_or_else(|| kind_for_prompt(&self.make_prompt));
        framed().inner_margin(egui::Margin::symmetric(12, 6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.set_height(32.0);
                ui.label(b("Make", 14.0).color(ACCENT));
                let field_w = (ui.available_width() - 270.0).max(160.0);
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.make_prompt)
                        .hint_text("Describe something new: “a 6-slide lesson on fractions”, “a monthly budget”, “a letter to parents”…")
                        .frame(egui::Frame::NONE)
                        .desired_width(field_w),
                );
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    go = true;
                }
                let label = |k: Option<Kind>| match k {
                    None => format!("Any app ({})", picked.label()),
                    Some(k) => k.label().to_string(),
                };
                egui::ComboBox::from_id_salt("make_kind").selected_text(label(self.make_kind)).width(130.0).show_ui(ui, |ui| {
                    *ui.visuals_mut() = home_visuals();
                    for k in [None, Some(Kind::Word), Some(Kind::Grid), Some(Kind::Deck), Some(Kind::Graph)] {
                        let text = match k {
                            None => "Any app (from the words)".to_string(),
                            Some(k) => k.label().to_string(),
                        };
                        ui.selectable_value(&mut self.make_kind, k, text);
                    }
                });
                let can = !self.make_prompt.trim().is_empty();
                let btn = egui::Button::new(b("Make it", 14.0).color(egui::Color32::WHITE)).fill(if can { ACCENT } else { MUTED }).corner_radius(8.0).min_size(egui::vec2(80.0, 30.0));
                if ui.add_enabled(can, btn).on_hover_text(format!("Opens a new {} tab and asks its assistant to make it ({})", picked.label(), self.ai_cfg.model)).clicked() {
                    go = true;
                }
            });
        });
        let text = self.make_prompt.trim().to_string();
        (go && !text.is_empty()).then_some((picked, text))
    }

    /// New from prompt: a new tab of `kind` whose assistant is asked to make `text`.
    fn make_from_prompt(&mut self, ctx: &egui::Context, kind: Kind, text: &str) {
        let id = self.new_tab(kind);
        let cfg = self.ai_cfg.clone();
        let ask = if kind == Kind::Graph { vishual_mcp::generate_prompt(text) } else { prompt_for_new(kind, text) };
        if let Some(e) = self.entry_mut(id) {
            let words: String = text.split_whitespace().take(6).collect::<Vec<_>>().join(" ");
            if !words.is_empty() {
                e.name = if text.split_whitespace().count() > 6 { format!("{words}…") } else { words };
            }
            e.chat.open = true;
            e.chat.ask_text(ask, ai_app(kind), &cfg, ctx);
        }
        self.make_prompt.clear();
        self.make_kind = None;
        ctx.request_repaint();
    }

    /// The toast, at the foot of the window until it times out (an error stays longer).
    fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some((text, error, at)) = &self.toast else { return };
        let now = ctx.input(|i| i.time);
        let life = if *error { 9.0 } else { 5.0 };
        if now - at > life {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((life - (now - at)).max(0.05)));
        let (fill, ink) = if *error { (egui::Color32::from_rgb(253, 236, 236), egui::Color32::from_rgb(160, 30, 40)) } else { (egui::Color32::from_rgb(230, 245, 243), egui::Color32::from_rgb(20, 90, 85)) };
        let mut dismiss = false;
        egui::Area::new(egui::Id::new("officecraft_toast")).order(egui::Order::Foreground).anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -28.0)).show(ctx, |ui| {
            egui::Frame::new().fill(fill).stroke(egui::Stroke::new(1.0, ink.gamma_multiply(0.4))).corner_radius(8).inner_margin(egui::Margin::symmetric(14, 8)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(text.as_str()).color(ink).size(13.5));
                    if ui.small_button("×").on_hover_text("Dismiss").clicked() {
                        dismiss = true;
                    }
                });
            });
        });
        if dismiss {
            self.toast = None;
        }
    }

    /// The tab a conversion writes into: a new one of `kind` (named `name` when given), or the
    /// open one `to` names. `(id, made here)`.
    fn send_target(&mut self, to: SendTo, kind: Kind, name: &str) -> Result<(u64, bool), String> {
        // Never two tabs of the same name: an untitled source keeps its name to itself.
        let name = if self.entries.iter().any(|e| e.name == name.trim()) { "" } else { name };
        match to {
            SendTo::Into(id) => match self.entries.iter().find(|e| e.id == id) {
                Some(e) if e.kind == kind => Ok((id, false)),
                _ => Err("that tab was closed".into()),
            },
            SendTo::New(_) => {
                let id = if kind == Kind::Deck {
                    self.new_tab(Kind::Deck)
                } else {
                    self.push_tab(kind, Tab::create(kind))
                };
                if !name.trim().is_empty()
                    && let Some(e) = self.entry_mut(id)
                {
                    e.name = name.trim().to_string();
                }
                Ok((id, true))
            }
            SendTo::Pdf => Err("not a tab".into()),
        }
    }

    /// A tab a conversion made and then could not fill: gone again, with nothing to save.
    fn drop_made_tab(&mut self, id: u64, back: View) {
        self.entries.retain(|e| e.id != id);
        self.tiles.retain(|t| *t != id);
        self.view = back;
    }

    /// The conversion itself. `Ok(None)`: the user cancelled (the PDF file dialog).
    fn convert_tab(&mut self, ctx: &egui::Context, from: u64, to: SendTo) -> Result<Option<String>, String> {
        let (kind, name) = self.entries.iter().find(|e| e.id == from).map(|e| (e.kind, e.name.clone())).ok_or_else(|| "that tab was closed".to_string())?;
        if to == SendTo::Pdf {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let dir = self.entries.iter().find(|e| e.id == from).and_then(|e| e.path.as_deref()).and_then(|p| std::path::Path::new(p).parent().map(std::path::Path::to_path_buf));
                let mut dlg = rfd::FileDialog::new().set_file_name(convert::renamed(&name, "pdf")).add_filter("PDF", &["pdf"]);
                if let Some(d) = dir {
                    dlg = dlg.set_directory(d);
                }
                let Some(dest) = dlg.save_file() else { return Ok(None) };
                return self.send_pdf(ctx, from, &dest.to_string_lossy()).map(Some);
            }
            #[cfg(target_arch = "wasm32")]
            return Err("PDF copies are made in the desktop app".into());
        }
        let back = self.view;
        let target_kind = match to {
            SendTo::New(k) => k,
            SendTo::Into(id) => self.entries.iter().find(|e| e.id == id).map(|e| e.kind).ok_or_else(|| "that tab was closed".to_string())?,
            SendTo::Pdf => unreachable!(),
        };
        // Read everything first, so a document that cannot be converted opens no tab.
        let made = match (kind, target_kind) {
            (Kind::Word, Kind::Deck) => {
                let inspect = self.tab_call(ctx, from, "document.inspect", serde_json::json!({ "text": true }))?;
                let mut args = convert::doc_to_slides(&inspect, &name)?;
                // Added to a deck that has its own: no second title slide.
                if matches!(to, SendTo::Into(_))
                    && let Some(list) = args.get_mut("slides").and_then(serde_json::Value::as_array_mut)
                    && list.len() > 1
                    && list[0].get("layout").and_then(serde_json::Value::as_str) == Some("title")
                {
                    list.remove(0);
                }
                let title = args.get("title").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
                let (deck, fresh) = self.send_target(to, Kind::Deck, &title)?;
                let built = match self.entry_mut(deck) {
                    Some(e) => {
                        let mut host = TabHost { tab: &mut e.tab, client: &mut e.pdf_client, ctx };
                        ai::build_slides(&mut host, &args)
                    }
                    None => Err("that tab was closed".into()),
                };
                (deck, fresh, built.map(|v| format!("Made {} from {name}.", plural(v.get("slides_added").and_then(serde_json::Value::as_u64).unwrap_or(0), "slide"))))
            }
            (Kind::Deck, Kind::Word) => {
                let info = self.tab_call(ctx, from, "document.inspect", serde_json::json!({}))?;
                let n = info.get("slides").and_then(serde_json::Value::as_array).map_or(0, Vec::len);
                let mut slides = Vec::with_capacity(n);
                for i in 0..n {
                    slides.push(self.tab_run(ctx, from, "slide.inspect", serde_json::json!({ "index": i }))?);
                }
                let mut args = convert::slides_to_blocks(&slides)?;
                let (doc, fresh) = self.send_target(to, Kind::Word, &convert::stem(&name))?;
                if !fresh {
                    args["at"] = serde_json::json!("cursor");
                }
                let r = match self.entry_mut(doc) {
                    Some(e) => {
                        let mut host = TabHost { tab: &mut e.tab, client: &mut e.pdf_client, ctx };
                        ai::insert_blocks(&mut host, &args)
                    }
                    None => Err("that tab was closed".into()),
                };
                (doc, fresh, r.map(|_| format!("Wrote the text of {} from {name}.", plural(n as u64, "slide"))))
            }
            (Kind::Grid, Kind::Word) => {
                let (text, html, range) = self.sheet_table(ctx, from)?;
                let (dest, fresh) = self.send_target(to, Kind::Word, &convert::stem(&name))?;
                let mut paste = serde_json::json!({ "text": text });
                if let Some(h) = &html {
                    paste["html"] = serde_json::json!(h);
                }
                let r = self.tab_run(ctx, dest, "edit.paste", paste);
                let how = if html.is_some() { "" } else { " (as text: the range is too big for a table)" };
                (dest, fresh, r.map(|_| format!("Sent {range} from {name}{how}.")))
            }
            (Kind::Grid, Kind::Deck) => {
                // DeckCraft pastes HTML as text, so the table is made from the shown values.
                let read = self.tab_call(ctx, from, "sheet.read", serde_json::json!({ "formatted": true }))?;
                let range = read.get("range").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
                // The slide size is the target deck's; a new deck's is the default.
                let probe = (960.0, 540.0);
                convert::slide_table(&read, probe)?;
                let (dest, fresh) = self.send_target(to, Kind::Deck, &convert::stem(&name))?;
                let size = self
                    .tab_call(ctx, dest, "document.inspect", serde_json::json!({}))
                    .ok()
                    .and_then(|v| v.get("slideSize").and_then(serde_json::Value::as_array).and_then(|a| Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))))
                    .unwrap_or(probe);
                let r = convert::slide_table(&read, size).and_then(|(params, cut)| {
                    self.new_slide_for(ctx, dest, fresh, "titleOnly", &convert::stem(&name))?;
                    self.tab_run(ctx, dest, "insert.table", params)?;
                    let (mr, mc) = convert::SLIDE_TABLE_MAX;
                    Ok(if cut { format!("Sent {range} from {name}: the first {mr} rows and {mc} columns fit on the slide.") } else { format!("Sent {range} from {name}.") })
                });
                (dest, fresh, r)
            }
            (Kind::Graph, Kind::Word | Kind::Deck) => {
                let (png, doc_json) = match self.entry_mut(from).map(|e| &mut e.tab) {
                    Some(Tab::Graph(a)) => (a.render_png(GRAPH_PX)?, a.doc_json()),
                    _ => return Err("that tab was closed".into()),
                };
                let (dest, fresh) = self.send_target(to, target_kind, if target_kind == Kind::Word { "" } else { "Graph" })?;
                let r = if target_kind == Kind::Word {
                    // The graph rides inside the PNG, so a double-click opens it in viSHual again.
                    let data = graphpic::base64_encode(&graphpic::embed(&png, &doc_json).unwrap_or(png));
                    self.tab_run(ctx, dest, "insert.picture", serde_json::json!({ "data": data, "width": 430, "alt": "Graph" })).map(|_| {
                        let _ = self.tab_run(ctx, dest, "select.collapse", serde_json::json!({ "end": true }));
                    })
                } else {
                    self.new_slide_for(ctx, dest, fresh, "blank", "")
                        .and_then(|_| self.tab_run(ctx, dest, "insert.picture", serde_json::json!({ "data": graphpic::base64_encode(&png), "name": "graph.png" })).map(|_| ()))
                };
                (dest, fresh, r.map(|_| format!("Sent the graph from {name}.")))
            }
            _ => return Err(format!("{} can't go to {}", kind.label(), target_kind.label())),
        };
        let (dest, fresh, result) = made;
        match result {
            Ok(said) => {
                self.view = View::Tab(dest);
                Ok(Some(said))
            }
            Err(e) => {
                if fresh {
                    self.drop_made_tab(dest, back);
                }
                Err(e)
            }
        }
    }

    /// A Sheets tab's used range on its active sheet, as (tab-separated text, HTML table when
    /// small enough, the range's address). GridCraft's own copy makes the HTML; what the user
    /// had copied inside GridCraft is put back afterwards.
    fn sheet_table(&mut self, ctx: &egui::Context, id: u64) -> Result<(String, Option<String>, String), String> {
        let read = self.tab_call(ctx, id, "sheet.read", serde_json::json!({}))?;
        let range = read.get("range").and_then(serde_json::Value::as_str).filter(|r| !r.is_empty()).ok_or_else(|| "the sheet is empty".to_string())?.to_string();
        let saved = match self.entry_mut(id).map(|e| &mut e.tab) {
            Some(Tab::Grid(a)) => a.session.clipboard.take(),
            _ => None,
        };
        let copy = self.tab_run(ctx, id, "edit.copy", serde_json::json!({ "range": range, "html": true }));
        if let Some(Tab::Grid(a)) = self.entry_mut(id).map(|e| &mut e.tab) {
            a.session.clipboard = saved;
        }
        let copy = copy?;
        let text = copy.get("text").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
        let html = copy.get("html").and_then(serde_json::Value::as_str).filter(|h| !h.is_empty()).map(str::to_string);
        Ok((text, html, range))
    }

    /// A new slide in deck `id` to put something on, with `layout` and `title`. A deck made for
    /// this has its empty first slide replaced.
    fn new_slide_for(&mut self, ctx: &egui::Context, id: u64, fresh: bool, layout: &str, title: &str) -> Result<(), String> {
        let mut p = serde_json::json!({ "layout": layout });
        if !title.is_empty() {
            p["title"] = serde_json::json!(title);
        }
        self.tab_run(ctx, id, "slide.new", p)?;
        if fresh {
            self.tab_run(ctx, id, "slide.delete", serde_json::json!({ "index": 0 }))?;
        }
        Ok(())
    }

    /// Docs, Sheets or Slides → a PDF at `path`, opened in a PDF tab (desktop).
    #[cfg(not(target_arch = "wasm32"))]
    fn send_pdf(&mut self, ctx: &egui::Context, from: u64, path: &str) -> Result<String, String> {
        let canonical = canonical_path(path);
        if self.entries.iter().any(|e| e.path.as_deref() == Some(canonical.as_str())) {
            return Err(format!("{} is open in a tab; close it first", file_name_of(path)));
        }
        let kind = self.entries.iter().find(|e| e.id == from).map(|e| e.kind).ok_or_else(|| "that tab was closed".to_string())?;
        match kind {
            Kind::Word => self.tab_run(ctx, from, "file.exportPdf", serde_json::json!({ "path": path }))?,
            Kind::Grid => self.tab_run(ctx, from, "file.exportPdf", serde_json::json!({ "path": path, "sheets": "all" }))?,
            Kind::Deck => self.tab_run(ctx, from, "file.export", serde_json::json!({ "path": path, "format": "pdf" }))?,
            other => return Err(format!("{} has no PDF export", other.label())),
        };
        self.open_file(ctx, path);
        Ok(format!("Saved {} and opened it.", file_name_of(path)))
    }

    fn equation_inserts(&mut self, ctx: &egui::Context) {
        let targets: Vec<equation::Target> = self
            .entries
            .iter()
            .filter_map(|e| match e.kind {
                Kind::Word => Some(equation::Target { id: e.id, name: e.name.clone(), slides: false }),
                Kind::Deck => Some(equation::Target { id: e.id, name: e.name.clone(), slides: true }),
                _ => None,
            })
            .collect();
        let mut asked = Vec::new();
        for e in &mut self.entries {
            if let Tab::Equation(a) = &mut e.tab {
                a.targets = targets.clone();
                if let Some(i) = a.insert.take() {
                    asked.push((e.id, i));
                }
            }
        }
        for (from, ins) in asked {
            let (to, result) = match ins {
                equation::Insert::Equation { id, latex } => (
                    id,
                    match self.entry_mut(id).map(|e| &mut e.tab) {
                        Some(Tab::Word(a)) => a.run("insert.equation", serde_json::json!({ "latex": latex })).map(|_| ()).map_err(|e| e.to_string()),
                        _ => Err("that document was closed".to_string()),
                    },
                ),
                equation::Insert::Picture { id, png } => (
                    id,
                    match self.entry_mut(id).map(|e| &mut e.tab) {
                        Some(Tab::Deck(a)) => a
                            .run("insert.picture", serde_json::json!({ "data": graphpic::base64_encode(&png), "name": "equation.png" }))
                            .map(|_| ())
                            .map_err(|e| e.to_string()),
                        _ => Err("that presentation was closed".to_string()),
                    },
                ),
            };
            match result {
                Ok(()) => self.view = View::Tab(to),
                Err(e) => {
                    let now = ctx.input(|i| i.time);
                    if let Some(Tab::Equation(a)) = self.entry_mut(from).map(|e| &mut e.tab) {
                        a.failed(format!("Couldn't insert: {e}"), now);
                    }
                }
            }
            ctx.request_repaint();
        }
    }

    /// Tabs holding unsaved work: (id, name).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn dirty_names(&self) -> Vec<(u64, String)> {
        self.entries.iter().filter(|e| e.tab.is_dirty()).map(|e| (e.id, e.name.clone())).collect()
    }

    /// Closes the window (and for an update, starts the new build first). With unsaved tabs it
    /// asks instead, unless the user already chose to discard them.
    #[cfg(not(target_arch = "wasm32"))]
    fn leave(&mut self, ctx: &egui::Context, why: Quit) {
        if !self.quit_ok && !self.dirty_names().is_empty() {
            self.confirm_quit = Some(why);
            return;
        }
        self.quit_ok = true;
        if why == Quit::Restart && !self.leaving {
            updater::spawn_new_instance();
        }
        // Slides and PDF tabs cancel a close themselves while they hold unsaved work (each asks
        // its own question). The user has answered already: tell DeckCraft, and show Home so no
        // app's own guard runs (PdfCraft's answer can't be set from outside).
        for e in &mut self.entries {
            if let Tab::Deck(a) = &mut e.tab {
                a.quit_confirmed = true;
            }
        }
        if !self.leaving {
            self.untile();
            self.view = View::Home;
        }
        self.leaving = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
        let mut make: Option<(Kind, String)> = None;
        let mut pick_files = false;
        let mut add_folder = false;
        let mut remove_folder: Option<usize> = None;
        let mut select: Option<Option<usize>> = None;
        let mut want_filter = self.filter;
        let mut acts = HomeActions::default();
        // Breadcrumb: go to this folder (`None`: the top of the sidebar folder).
        let mut crumb_go: Option<Option<String>> = None;
        let mut new_folder = false;
        // Sidebar folder i dragged to position j.
        let mut reorder: Option<(usize, usize)> = None;
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut open_trash = false;

        let mut want_grid = self.home_grid;

        // Recent rows are read from disk only when the list changes, not every frame (a file on
        // a NAS makes every `stat` slow).
        if self.recent_rows.0 != self.recent {
            self.recent_rows = (self.recent.clone(), self.recent.iter().map(|p| row_for(p)).collect());
        }
        let source: Vec<Row> = match self.folder_sel {
            None => self.recent_rows.1.clone(),
            Some(_) => self.folder_rows.clone(),
        };
        let q = self.search.to_lowercase();
        let rows: Vec<Row> = source
            .iter()
            .filter(|r| match want_filter {
                Filter::All => true,
                // Folders stay, so the filter never hides the way into one.
                Filter::Only(k) => r.is_dir || r.kind == Some(k),
            })
            .filter(|r| q.is_empty() || r.name.to_lowercase().contains(&q) || r.folder.to_lowercase().contains(&q))
            .cloned()
            .collect();
        let top = self.folder_sel.and_then(|i| self.folders.get(i)).cloned();
        let title = match &top {
            Some(f) => file_name_of(f),
            None => "Recent files".to_string(),
        };
        // The folders between the sidebar folder and the one shown: (name, path).
        let crumbs: Vec<(String, String)> = match (&top, &self.cwd) {
            (Some(t), Some(c)) => std::path::Path::new(c)
                .strip_prefix(t)
                .map(|rel| {
                    let mut acc = std::path::PathBuf::from(t);
                    rel.components()
                        .map(|part| {
                            acc.push(part);
                            (part.as_os_str().to_string_lossy().to_string(), acc.to_string_lossy().to_string())
                        })
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };

        egui::Panel::left("home_sidebar")
            .exact_size(232.0)
            .frame(egui::Frame::new().fill(egui::Color32::WHITE).stroke(egui::Stroke::new(1.0, LINE)).inner_margin(egui::Margin::symmetric(12, 0)))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    let (logo, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
                    paint_logo(ui.painter(), logo);
                    ui.add_space(4.0);
                    ui.label(b("OfficeCraft", 17.0));
                });
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new(platform::build_label()).size(10.0).color(MUTED))
                        .on_hover_text(format!("The commit this build came from. The desktop and web builds match when these agree.\n\nApps: {}", platform::app_versions()));
                });
                ui.add_space(8.0);
                if side_row(ui, self.folder_sel.is_none(), false, "Recent", Some(self.recent.len()), false).0.clicked() {
                    select = Some(None);
                }
                // A page can browse folders only where the browser offers a directory picker.
                let folders_ui = !cfg!(target_arch = "wasm32") || web_can_browse();
                if folders_ui {
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    ui.label(b("FOLDERS", 12.0).color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(10.0);
                        if ui.add(egui::Button::new(b("+", 18.0).color(ACCENT)).frame(false)).on_hover_text("Add folder").clicked() {
                            add_folder = true;
                        }
                    });
                });
                for (i, f) in self.folders.iter().enumerate() {
                    let name = file_name_of(f);
                    let (resp, removed) = side_row(ui, self.folder_sel == Some(i), true, &name, self.folder_counts.get(i).copied(), true);
                    let resp = resp.on_hover_text(f);
                    // Drag a folder up or down the list to reorder it.
                    if FILE_OPS {
                        resp.dnd_set_drag_payload(FolderDrag(i));
                    }
                    if let Some(from) = resp.dnd_hover_payload::<FolderDrag>()
                        && from.0 != i
                        && let Some(p) = ui.ctx().pointer_latest_pos()
                    {
                        let y = if p.y < resp.rect.center().y { resp.rect.top() } else { resp.rect.bottom() };
                        ui.painter().hline(resp.rect.x_range(), y, egui::Stroke::new(2.0, ACCENT));
                    }
                    if let Some(from) = dropped::<FolderDrag>(&resp)
                        && let Some(p) = ui.ctx().pointer_latest_pos()
                    {
                        let below = p.y >= resp.rect.center().y;
                        reorder = Some((from.0, if below { i + 1 } else { i }));
                    }
                    // A file or folder from the list dropped here moves into this folder.
                    if FILE_OPS {
                        paint_drop_hint(ui, &resp, resp.rect);
                        if let Some(m) = dropped::<MovePayload>(&resp) {
                            acts.moves.push((m.0.clone(), f.clone()));
                        }
                    }
                    resp.context_menu(|ui| {
                        menu_visuals(ui);
                        if ui.button("Remove from sidebar").on_hover_text("The folder and its files stay on disk").clicked() {
                            remove_folder = Some(i);
                            ui.close();
                        }
                        if FILE_OPS {
                            if ui.button("Show in file manager").clicked() {
                                acts.reveal = Some(f.clone());
                                ui.close();
                            }
                        }
                    });
                    if removed {
                        remove_folder = Some(i);
                    } else if resp.clicked() {
                        select = Some(Some(i));
                    }
                }
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), egui::Sense::click());
                let p = ui.painter();
                if resp.hovered() {
                    p.rect_filled(rect, 8.0, egui::Color32::from_gray(244));
                }
                p.text(rect.left_center() + egui::vec2(16.0, 0.0), egui::Align2::CENTER_CENTER, "+", egui::FontId::new(18.0, bold()), ACCENT);
                p.text(rect.left_center() + egui::vec2(36.0, 0.0), egui::Align2::LEFT_CENTER, "Add folder", egui::FontId::new(14.0, bold()), ACCENT);
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    add_folder = true;
                }
                // Items trashed from places with no system trash (a NAS share): restore them here.
                #[cfg(not(target_arch = "wasm32"))]
                if !self.own_trash.is_empty() {
                    ui.add_space(14.0);
                    let r = side_row(ui, self.show_trash, false, "OfficeCraft Trash", Some(self.own_trash.len()), false).0;
                    if r.on_hover_text("Files moved to Trash from folders that have no system trash").clicked() {
                        open_trash = true;
                    }
                }
                }
                // the sidebar's foot: the contact page (contact.html, the shared contact form)
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.add_space(12.0);
                    let resp = side_row(ui, false, false, "Contact", None, false).0;
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Link, true, "Contact"));
                    if resp.on_hover_text("Send a question or note (opens in a new tab)").clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(CONTACT_URL));
                    }
                });
            });

        egui::CentralPanel::default().frame(egui::Frame::new().fill(egui::Color32::from_rgb(237, 241, 245)).inner_margin(egui::Margin::symmetric(36, 26))).show(ui, |ui| {
            ui.horizontal(|ui| {
                if self.show_trash {
                    ui.label(b("OfficeCraft Trash", 24.0));
                } else if crumbs.is_empty() {
                    ui.label(b(&title, 24.0));
                } else {
                    // The path from the sidebar folder down to the one shown: each part goes back
                    // up to it, and takes a dragged row (move it there).
                    let top_resp = ui.add(egui::Label::new(b(&title, 24.0).color(SOFT)).sense(egui::Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand);
                    if top_resp.clicked() {
                        crumb_go = Some(None);
                    }
                    if let Some(m) = dropped::<MovePayload>(&top_resp)
                        && let Some(t) = &top
                    {
                        acts.moves.push((m.0.clone(), t.clone()));
                    }
                    paint_drop_hint(ui, &top_resp, top_resp.rect.expand(4.0));
                    let last = crumbs.len() - 1;
                    for (k, (name, path)) in crumbs.iter().enumerate() {
                        ui.label(b("›", 22.0).color(MUTED));
                        let text = if k == last { b(name, 24.0) } else { b(name, 24.0).color(SOFT) };
                        let r = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
                        if k != last {
                            let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
                            if r.clicked() {
                                crumb_go = Some(Some(path.clone()));
                            }
                            if let Some(m) = dropped::<MovePayload>(&r) {
                                acts.moves.push((m.0.clone(), path.clone()));
                            }
                            paint_drop_hint(ui, &r, r.rect.expand(4.0));
                        }
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.scope(|ui| {
                        let hover = egui::Color32::from_rgb(9, 100, 96);
                        let v = ui.visuals_mut();
                        for (w, fill) in [(&mut v.widgets.inactive, ACCENT), (&mut v.widgets.hovered, hover), (&mut v.widgets.active, hover), (&mut v.widgets.open, hover)] {
                            w.weak_bg_fill = fill;
                            w.bg_fill = fill;
                            w.bg_stroke = egui::Stroke::NONE;
                            w.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
                            w.corner_radius = egui::CornerRadius::same(10);
                        }
                        ui.spacing_mut().button_padding = egui::vec2(18.0, 10.0);
                        ui.menu_button(b("New file", 14.0).color(egui::Color32::WHITE), |ui| {
                            *ui.visuals_mut() = home_visuals();
                            for kind in Kind::ALL {
                                if ui.button(format!("New {}", kind.label())).clicked() {
                                    new = Some(kind);
                                    ui.close();
                                }
                            }
                        });
                    });
                    ui.spacing_mut().button_padding = egui::vec2(16.0, 10.0);
                    let open_btn = egui::Button::new(b("Open file", 14.0)).fill(egui::Color32::WHITE).stroke(egui::Stroke::new(1.0, LINE)).corner_radius(10.0).min_size(egui::vec2(0.0, 40.0));
                    if ui.add(open_btn).clicked() {
                        pick_files = true;
                    }
                    if FILE_OPS && self.folder_sel.is_some() && !self.show_trash && matches!(self.listing_state, Listing::Ready) {
                        let nf = egui::Button::new(b("New folder", 14.0)).fill(egui::Color32::WHITE).stroke(egui::Stroke::new(1.0, LINE)).corner_radius(10.0).min_size(egui::vec2(0.0, 40.0));
                        if ui.add(nf).on_hover_text("Make a folder here").clicked() {
                            new_folder = true;
                        }
                    }
                    framed().inner_margin(egui::Margin::symmetric(12, 0)).show(ui, |ui| {
                        ui.set_height(38.0);
                        ui.horizontal_centered(|ui| {
                            let (r, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                            paint_search(ui.painter(), r.center(), MUTED);
                            ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search").frame(egui::Frame::NONE).desired_width(220.0));
                        });
                    });
                });
            });
            if let Some(n) = &self.notice {
                ui.add_space(6.0);
                ui.colored_label(egui::Color32::from_rgb(190, 40, 50), n);
            } else if let Some(st) = &self.status {
                ui.add_space(6.0);
                ui.colored_label(ACCENT, st);
            }

            ui.add_space(14.0);
            make = self.make_from_prompt_ui(ui);
            ui.add_space(14.0);
            let starts = [(Kind::Word, "WordCraft", "Docs"), (Kind::Grid, "GridCraft", "Sheets"), (Kind::Deck, "DeckCraft", "Slides"), (Kind::Pdf, "PdfCraft", "PDF"), (Kind::Graph, "viSHual", "Graphs"), (Kind::Equation, "equaSHion", "Equations"), (Kind::Vershion, "verSHion", "Exams")];
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.columns(7, |cols| {
                for (col, (kind, label, sub)) in cols.iter_mut().zip(starts) {
                    let card = framed().inner_margin(egui::Margin::symmetric(12, 10)).show(col, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(egui::vec2(26.0, 34.0), egui::Sense::hover());
                            ui.painter().rect_filled(r, 5.0, kind_color(Some(kind)));
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                ui.label(b(label, 14.0));
                                ui.label(egui::RichText::new(sub).size(12.0).color(MUTED));
                            });
                        });
                    });
                    let r = card.response.interact(egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                    if r.clicked() {
                        new = Some(kind);
                    }
                }
            });

            ui.add_space(20.0);
            ui.horizontal(|ui| {
                framed().inner_margin(3.0).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.horizontal(|ui| {
                        for (f, label) in [
                            (Filter::All, "All"),
                            (Filter::Only(Kind::Word), "Docs"),
                            (Filter::Only(Kind::Grid), "Sheets"),
                            (Filter::Only(Kind::Deck), "Slides"),
                            (Filter::Only(Kind::Pdf), "PDF"),
                            (Filter::Only(Kind::Graph), "Graphs"),
                            (Filter::Only(Kind::Equation), "Equations"),
                            (Filter::Only(Kind::Vershion), "Exams"),
                        ] {
                            if pill(ui, want_filter == f, label).clicked() {
                                want_filter = f;
                            }
                        }
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(80.0, 36.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 10.0, egui::Color32::WHITE);
                    ui.painter().rect_stroke(rect, 10.0, egui::Stroke::new(1.0, LINE), egui::StrokeKind::Inside);
                    for (i, grid_icon) in [true, false].into_iter().enumerate() {
                        let cell = egui::Rect::from_min_size(rect.min + egui::vec2(3.0 + i as f32 * 36.0, 3.0), egui::vec2(34.0, 30.0));
                        let active = want_grid == grid_icon;
                        let resp = ui.interact(cell, ui.id().with(("home_view", i)), egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                        if active {
                            ui.painter().rect_filled(cell, 8.0, INK);
                        }
                        let col = if active { egui::Color32::WHITE } else { MUTED };
                        let st = egui::Stroke::new(1.5, col);
                        let c = cell.center();
                        let p = ui.painter();
                        if grid_icon {
                            for (dx, dy) in [(-4.5, -4.5), (4.5, -4.5), (-4.5, 4.5), (4.5, 4.5)] {
                                p.rect_stroke(egui::Rect::from_center_size(c + egui::vec2(dx, dy), egui::vec2(6.0, 6.0)), 1.0, st, egui::StrokeKind::Inside);
                            }
                        } else {
                            for dy in [-5.0, 0.0, 5.0] {
                                p.line_segment([c + egui::vec2(-7.0, dy), c + egui::vec2(7.0, dy)], st);
                            }
                        }
                        if resp.clicked() {
                            want_grid = grid_icon;
                        }
                    }
                    ui.label(egui::RichText::new("Last modified").size(13.0).color(SOFT));
                });
            });
            ui.add_space(14.0);

            #[cfg(not(target_arch = "wasm32"))]
            let trash_view = self.show_trash;
            #[cfg(target_arch = "wasm32")]
            let trash_view = false;
            let in_recent = self.folder_sel.is_none();
            if trash_view {
                #[cfg(not(target_arch = "wasm32"))]
                self.trash_list(ui);
            } else if rows.is_empty() {
                ui.add_space(24.0);
                let empty = match (&self.listing_state, self.folder_sel.is_some()) {
                    (Listing::Loading, true) => "Reading the folder…".to_string(),
                    (Listing::Failed(e), true) => format!("Couldn't read this folder: {e}"),
                    _ => "No folders or supported files here.".to_string(),
                };
                ui.label(egui::RichText::new(if self.folder_sel.is_some() { empty.as_str() } else { if cfg!(target_arch = "wasm32") { web_open_hint() } else { "Files you open appear here. Drop a file on the window, or press Open file." } }).color(MUTED));
            } else if want_grid {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    let gap = 16.0;
                    let avail = ui.available_width() - 14.0;
                    let cols = ((avail + gap) / (200.0 + gap)).floor().max(1.0);
                    let card_w = ((avail - gap * (cols - 1.0)) / cols).floor();
                    ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
                    for chunk in rows.chunks(cols as usize) {
                        ui.horizontal(|ui| {
                            for r in chunk {
                                let sense = if FILE_OPS { egui::Sense::click_and_drag() } else { egui::Sense::click() };
                                let (rect, resp) = ui.allocate_exact_size(egui::vec2(card_w, 164.0), sense);
                                let resp = resp.on_hover_text(&r.path).on_hover_cursor(egui::CursorIcon::PointingHand);
                                let stroke = if resp.hovered() { egui::Stroke::new(1.5, ACCENT) } else { egui::Stroke::new(1.0, LINE) };
                                let p = ui.painter();
                                p.rect_filled(rect, 10.0, egui::Color32::WHITE);
                                let top_r = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 96.0));
                                if r.is_dir {
                                    p.rect_filled(top_r, egui::CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 }, ACCENT.gamma_multiply(0.10));
                                    paint_folder_big(p, top_r.center(), ACCENT);
                                } else {
                                    p.rect_filled(top_r, egui::CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 }, kind_color(r.kind).gamma_multiply(0.14));
                                    thumb(p, r.kind, top_r);
                                }
                                p.rect_stroke(rect, 10.0, stroke, egui::StrokeKind::Inside);
                                let name: String = if r.name.chars().count() > 26 { r.name.chars().take(25).collect::<String>() + "…" } else { r.name.clone() };
                                p.text(rect.min + egui::vec2(14.0, 110.0), egui::Align2::LEFT_TOP, name, egui::FontId::new(14.0, bold()), INK);
                                let sub = match (r.is_dir, r.modified) {
                                    (true, _) => "Folder".to_string(),
                                    (false, Some(t)) => format!("{}, {}", r.folder, ago(t)),
                                    (false, None) => r.folder.clone(),
                                };
                                p.text(rect.min + egui::vec2(14.0, 136.0), egui::Align2::LEFT_TOP, sub, egui::FontId::proportional(12.0), MUTED);
                                if r.is_dir {
                                    paint_drop_hint(ui, &resp, rect);
                                }
                                row_interact(&resp, r, in_recent, &mut acts);
                            }
                        });
                    }
                });
            } else {
                framed().show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let w = ui.available_width();
                    let cols = [64.0, w * 0.50, w * 0.74, w * 0.87];
                    let (hdr, _) = ui.allocate_exact_size(egui::vec2(w, 38.0), egui::Sense::hover());
                    for (x, t) in cols.iter().zip(["Name", "Folder", "Modified", "Size"]) {
                        ui.painter().text(hdr.min + egui::vec2(*x, 19.0), egui::Align2::LEFT_CENTER, t, egui::FontId::proportional(12.0), MUTED);
                    }
                    ui.painter().hline(hdr.x_range(), hdr.bottom(), egui::Stroke::new(1.0, HAIRLINE));
                    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                        for r in &rows {
                            let sense = if FILE_OPS { egui::Sense::click_and_drag() } else { egui::Sense::click() };
                            let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 45.0), sense);
                            let resp = resp.on_hover_text(&r.path).on_hover_cursor(egui::CursorIcon::PointingHand);
                            let p = ui.painter();
                            if resp.hovered() {
                                p.rect_filled(rect, 0.0, egui::Color32::from_rgb(246, 248, 250));
                            }
                            let mid = rect.center().y;
                            let badge_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + 28.0, mid), egui::vec2(24.0, 24.0));
                            if r.is_dir {
                                p.rect_filled(badge_rect, 6.0, ACCENT.gamma_multiply(0.14));
                                paint_folder(p, badge_rect.center(), ACCENT);
                            } else {
                                p.rect_filled(badge_rect, 6.0, kind_color(r.kind));
                                p.text(badge_rect.center(), egui::Align2::CENTER_CENTER, kind_abbr(r.kind), egui::FontId::new(11.0, bold()), egui::Color32::WHITE);
                            }
                            let clip = p.with_clip_rect(egui::Rect::from_min_max(rect.min + egui::vec2(cols[0], 0.0), egui::pos2(rect.left() + cols[1] - 12.0, rect.bottom())));
                            clip.text(egui::pos2(rect.left() + cols[0], mid), egui::Align2::LEFT_CENTER, &r.name, egui::FontId::new(14.0, bold()), INK);
                            let clip2 = p.with_clip_rect(egui::Rect::from_min_max(egui::pos2(rect.left() + cols[1], rect.top()), egui::pos2(rect.left() + cols[2] - 12.0, rect.bottom())));
                            clip2.text(egui::pos2(rect.left() + cols[1], mid), egui::Align2::LEFT_CENTER, if r.is_dir { "Folder" } else { &r.folder }, egui::FontId::proportional(14.0), SOFT);
                            p.text(egui::pos2(rect.left() + cols[2], mid), egui::Align2::LEFT_CENTER, r.modified.map(ago).unwrap_or_default(), egui::FontId::proportional(14.0), SOFT);
                            p.text(egui::pos2(rect.left() + cols[3], mid), egui::Align2::LEFT_CENTER, if r.is_dir { "—".to_string() } else { human_size(r.size) }, egui::FontId::proportional(14.0), SOFT);
                            p.hline(rect.x_range(), rect.bottom(), egui::Stroke::new(1.0, HAIRLINE));
                            if r.is_dir {
                                paint_drop_hint(ui, &resp, rect);
                            }
                            row_interact(&resp, r, in_recent, &mut acts);
                        }
                    });
                });
            }
        });

        // A dragged row follows the pointer with its name.
        if let Some(m) = egui::DragAndDrop::payload::<MovePayload>(&ctx)
            && let Some(p) = ctx.pointer_latest_pos()
        {
            let layer = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("row-drag")));
            layer.text(p + egui::vec2(14.0, 14.0), egui::Align2::LEFT_TOP, format!("Move {}", file_name_of(&m.0)), egui::FontId::proportional(13.0), ACCENT);
        }

        if let Some(p) = acts.open.take() {
            open = Some(p);
        }
        if want_grid != self.home_grid {
            self.home_grid = want_grid;
            save_home_grid(want_grid);
        }

        self.filter = want_filter;
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.apply_home(&ctx, acts, crumb_go, new_folder, reorder, open_trash);
            self.file_op_ui(&ctx);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (acts, crumb_go, new_folder, reorder, open_trash);
        if let Some(s) = select {
            self.folder_sel = s;
            self.cwd = None;
            self.show_trash = false;
            #[cfg(not(target_arch = "wasm32"))]
            {
                self.purge_confirm = None;
            }
            self.folder_rows = s.and_then(|i| self.folders.get(i)).map(|d| folder_listing(&self.scans, d)).unwrap_or_default();
            // Files may have changed since the folder was added: list it again in the background.
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(d) = self.shown_dir() {
                self.listing_state = if self.scans.contains_key(&d) { Listing::Ready } else { Listing::Loading };
                self.request_listing(&ctx, &d);
            }
            #[cfg(target_arch = "wasm32")]
            if let Some(d) = s.and_then(|i| self.folders.get(i)) {
                web::rescan_folder(d, self.folder_scans.clone());
            }
        }
        if let Some(i) = remove_folder {
            let gone = self.folders.remove(i);
            self.scans.remove(&gone);
            save_folders(&self.folders);
            self.folder_sel = None;
            self.cwd = None;
            self.folder_rows.clear();
            self.folder_counts.truncate(0);
            self.recount(&ctx);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if add_folder && let Some(dir) = rfd::FileDialog::new().pick_folder() {
            let dir = canonical_path(&dir.to_string_lossy());
            if !self.folders.contains(&dir) {
                self.folders.push(dir.clone());
                save_folders(&self.folders);
            }
            let i = self.folders.iter().position(|f| *f == dir);
            self.folder_sel = i;
            self.show_trash = false;
            self.browse(&ctx, None);
            self.recount(&ctx);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if pick_files && let Some(p) = rfd::FileDialog::new().add_filter("Office files", OPEN_EXTS).pick_files() {
            // Several files at once each get their own tab.
            self.pending.extend(p.iter().map(|f| f.to_string_lossy().to_string()));
            ctx.request_repaint();
        }
        #[cfg(target_arch = "wasm32")]
        if pick_files {
            web::pick_files(OPEN_EXTS, self.inbox.clone());
        }
        #[cfg(target_arch = "wasm32")]
        if add_folder {
            web::add_folder(self.folder_scans.clone());
        }
        if let Some(k) = new {
            self.new_tab(k);
            ctx.request_repaint();
        }
        if let Some((k, text)) = make {
            self.make_from_prompt(&ctx, k, &text);
        }
        if let Some(p) = open {
            self.open_file(&ctx, &p);
        }
    }
}

/// Size, in points, the graph pictures are drawn at (the stage's shape).
const GRAPH_PX: (f32, f32) = (720.0, 480.0);

/// Draws the graph an assistant described: the PNG and the viSHual document it came from.
fn render_graph(args: &serde_json::Value) -> Result<(Vec<u8>, String), String> {
    let list: Vec<&str> = args.get("expressions").and_then(serde_json::Value::as_array).map(|a| a.iter().filter_map(serde_json::Value::as_str).map(str::trim).filter(|t| !t.is_empty()).collect()).unwrap_or_default();
    if list.is_empty() {
        return Err("no expressions".into());
    }
    let mut app = vishual_ui_egui::VishualApp::new(graph_services());
    let _ = app.control("engine.execute", serde_json::json!({"command": "newGraph", "params": {}}));
    for (i, e) in list.iter().enumerate() {
        app.control("engine.execute", serde_json::json!({"command": "addItem", "params": {"id": format!("a{}", i + 1), "kind": "equation", "latex": e}})).map_err(|er| format!("{e}: {er}"))?;
    }
    if args.get("mode").and_then(serde_json::Value::as_str) == Some("3d") {
        let _ = app.control("engine.execute", serde_json::json!({"command": "setMode", "params": {"mode": "3d"}}));
    }
    let png = app.render_png(GRAPH_PX)?;
    Ok((png, app.doc_json()))
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
            Tab::Equation(_) => Done(Err("the assistant does not work in the Equations tab".into())),
            Tab::Vershion(_) => Done(Err("the assistant does not work in the verSHion tab".into())),
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
        // Never asked: the Equations and verSHion tabs have no assistant (`Kind::has_assistant`).
        Kind::Equation | Kind::Vershion => ai::App::Docs,
    }
}

impl eframe::App for Suite {
    /// Last chance before the process ends: WordCraft finishes writing (or clearing) its recovery
    /// copies, so a document saved just before quitting is not offered back next launch.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        for e in &mut self.entries {
            if let Tab::Word(a) = &mut e.tab {
                a.session.finish_recovery();
            }
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let mut shot_tab = None;
        // Leaving GridCraft or PdfCraft: the context still holds what they installed, before
        // another app or Home replaces it this frame. Keep it for the way back.
        if let Some(k) = self.ctx_owner
            && restores_look(k)
            && self.owner_frames >= 4
        {
            let showing = matches!(self.view, View::Tab(id) if self.entries.iter().any(|e| e.id == id && e.kind == k));
            if !showing || self.owner_frames % 60 == 4 {
                self.looks.insert(k, Look::capture(ctx));
            }
        }
        if let Some((path, n)) = self.shot.as_mut() {
            *n += 1;
            ctx.request_repaint();
            // `OFFICECRAFT_SHOT_TAB=docs|sheets|slides|pdf|graph`: open a blank tab of that app first.
            if *n == 2
                && let Ok(t) = std::env::var("OFFICECRAFT_SHOT_TAB")
            {
                let kind = match t.as_str() {
                    "docs" => Some(Kind::Word),
                    "sheets" => Some(Kind::Grid),
                    "slides" => Some(Kind::Deck),
                    "pdf" => Some(Kind::Pdf),
                    "graph" => Some(Kind::Graph),
                    "equations" => Some(Kind::Equation),
                    "vershion" | "exams" => Some(Kind::Vershion),
                    _ => None,
                };
                shot_tab = kind;
            }
            // `OFFICECRAFT_SHOT_VISHUAL=chat,dark`: set up the Graph tab's sidebar and theme.
            if *n == 5
                && let Ok(spec) = std::env::var("OFFICECRAFT_SHOT_VISHUAL")
            {
                for e in self.entries.iter_mut() {
                    if let Tab::Graph(g) = &mut e.tab {
                        graph_shot_options(g, &spec);
                    }
                }
            }
            // `OFFICECRAFT_SHOT_EQUATION=1`: insert an equation on the front deck and edit it in place.
            if *n == 8 && std::env::var_os("OFFICECRAFT_SHOT_EQUATION").is_some() {
                for e in self.entries.iter_mut() {
                    if let Tab::Word(a) = &mut e.tab {
                        let r = a.run("insert.equation", serde_json::json!({"linear": "x=(-b+-sqrt(b^2-4ac))/2a"}));
                        eprintln!("shot-equation: docs insert.equation -> {:?}", r.map(|_| "ok"));
                    }
                    // Prototype (official releases): DeckCraft v0.18.1 has no `insert.equation`, so
                    // the Equations tab's picture goes in as a PNG.
                    if let Tab::Deck(a) = &mut e.tab
                        && std::env::var("OFFICECRAFT_SHOT_EQUATION").as_deref() == Ok("picture")
                    {
                        let m = equashion_egui::equashion_math::from_linear("x=(-b+-sqrt(b^2-4ac))/2a");
                        let png = equation::png_for(&m);
                        let r = a.run("insert.picture", serde_json::json!({"data": graphpic::base64_encode(&png), "name": "equation.png"}));
                        eprintln!("shot-equation: slides insert.picture -> {:?}", r.map(|_| "ok"));
                        continue;
                    }
                    if let Tab::Deck(a) = &mut e.tab {
                        // An existing equation, edited where it sits.
                        let made = a.run("insert.equation", serde_json::json!({"linear": "x=(-b+-sqrt(b^2-4ac))/2a", "rect": [160, 200, 640, 90]}));
                        if std::env::var_os("OFFICECRAFT_SHOT_EQUATION_STATIC").is_some() {
                            continue;
                        }
                        if let Some(id) = made.ok().and_then(|v| v.get("id").and_then(serde_json::Value::as_u64)) {
                            a.session.ui_requests.push(deckcraft_engine::UiRequest::Dialog {
                                id: "equation".into(),
                                params: serde_json::json!({"edit": true, "id": id, "paragraph": 0, "run": 0}),
                            });
                        }
                    }
                }
            }
            // `OFFICECRAFT_SHOT_CHAT=<prompt>`: open the assistant on the front tab and ask it;
            // the picture is taken once it has answered (or after 3 minutes).
            let chat = std::env::var("OFFICECRAFT_SHOT_CHAT").ok();
            let waiting = chat.is_some() && *n < 4000 && (*n < 30 || self.entries.last().is_some_and(|e| e.chat.busy()));
            if *n == 3
                && let (Some(prompt), Some(e)) = (chat.clone(), self.entries.last_mut())
            {
                e.chat.open = true;
                let (kind, cfg) = (e.kind, self.ai_cfg.clone());
                e.chat.ask_text(prompt, ai_app(kind), &cfg, ctx);
            }
            let settled = if self.tiles_test.is_some() { 420 } else { 20 };
            if *n >= settled && !waiting && !self.shot_sent {
                self.shot_sent = true;
                let c = ctx.clone();
                ctx.request_screenshot(move |img| {
                    c.data_mut(|d| d.insert_temp(egui::Id::new("officecraft-shot"), img));
                    c.request_repaint();
                });
            }
            let img = ctx.data_mut(|d| d.remove_temp::<std::sync::Arc<egui::ColorImage>>(egui::Id::new("officecraft-shot")));
            if let Some(img) = img {
                let [w, h] = img.size;
                let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
                out.extend(img.pixels.iter().flat_map(|c| [c.r(), c.g(), c.b()]));
                let ok = std::fs::write(&*path, out).is_ok();
                std::process::exit(if ok { 0 } else { 1 });
            }
            if *n > 4400 {
                eprintln!("shot: no screenshot event arrived");
                std::process::exit(1);
            }
        }
        if let Some(k) = shot_tab {
            self.new_tab(k);
        }
        if let Some(n) = self.tiles_test {
            // `OFFICECRAFT_TILES=ask`: open Docs, then Sheets beside it, and let the prompt show.
            let ask = std::env::var("OFFICECRAFT_TILES").is_ok_and(|v| v == "ask");
            if ask {
                match n {
                    0 => {
                        self.tile_pref = TilePref::Ask;
                        self.new_tab(Kind::Word);
                    }
                    40 => {
                        self.new_tab(Kind::Grid);
                    }
                    120 if !self.tile_prompt => {
                        eprintln!("tiles(ask): the prompt never appeared");
                        std::process::exit(1);
                    }
                    200 => self.tile_all(),
                    _ => {}
                }
                if n >= 320 && self.shot.is_none() {
                    eprintln!("tiles(ask): prompt shown and accepted over {n} frames without a panic");
                    std::process::exit(if self.tiles.len() >= 2 { 0 } else { 1 });
                }
            } else {
                if n == 0 {
                    for k in Kind::ALL {
                        self.new_tab(k);
                    }
                    self.tile_pref = TilePref::Never;
                    self.tile_all();
                }
                // Move focus, go back to single tabs, tile again: every switch re-installs fonts.
                match n {
                    120 => self.view = View::Tab(self.tiles[0]),
                    // Drag the last tile above the first, as a user dropping it on its top edge would.
                    // Take a tile out, then bring its tab back by dropping it on a tile's lower edge.
                    340 if self.tiles.len() >= 4 => {
                        let id = self.tiles[0];
                        self.take_out_tile(id);
                        self.tile_test_out = Some(id);
                    }
                    370 => {
                        if let (Some(id), Some(t)) = (self.tile_test_out.take(), self.tiles.first().copied()) {
                            self.tile_with(id, Some((t, DropZone::Below)));
                        }
                    }
                    300 if self.tiles.len() >= 4 => {
                        let (a, b) = (self.tiles[3], self.tiles[0]);
                        self.move_tile(a, b, DropZone::Above);
                    }
                    180 => self.untile(),
                    240 => self.tile_all(),
                    _ => {}
                }
                if n >= 420 && self.shot.is_none() {
                    eprintln!("tiles: ran {n} frames with {} tiles without a panic", self.tiles.len());
                    std::process::exit(if self.tiles.len() >= 2 { 0 } else { 1 });
                }
            }
            self.tiles_test = Some(n + 1);
            ctx.request_repaint();
        }
        if let Some(n) = self.selftest {
            const EVERY: u64 = 40;
            if n == 0 {
                for k in Kind::ALL {
                    self.new_tab(k);
                }
            }
            let step = (n / EVERY) as usize;
            if step >= 20 {
                eprintln!("selftest: cycled {step} views without a panic");
                std::process::exit(0);
            }
            if n % EVERY == 0 {
                let ids: Vec<u64> = self.entries.iter().map(|e| e.id).collect();
                // Every app in order (Word, Grid, Deck, Pdf, Graph, Equations, verSHion), Home
                // (index 7, past the tabs), then the same again so each pair of apps meets.
                let order = [0usize, 1, 2, 3, 4, 5, 6, 7, 6, 5, 2, 4, 0, 7, 3, 5, 1, 4, 7, 2, 6, 0];
                let i = order[step % order.len()];
                self.view = ids.get(i).map(|id| View::Tab(*id)).unwrap_or(View::Home);
            }
            self.selftest = Some(n + 1);
            ctx.request_repaint();
        }
        if self.view == View::Home {
            if !self.home_styled {
                BOLD_READY.store(false, std::sync::atomic::Ordering::Relaxed);
                // The app that was showing may have left a style naming font families of its own
                // (GridCraft's "ui"); Home's fonts do not have them.
                ctx.all_styles_mut(|s| *s = egui::Style::default());
                ctx.set_visuals(home_visuals());
                install_home_fonts(ctx);
                self.home_styled = true;
                // The next tab shown must install its own fonts and visuals again.
                self.ctx_owner = None;
                ctx.request_repaint();
            } else {
                let bound = ctx.fonts(|f| f.families().contains(&egui::FontFamily::Name("home_bold".into())));
                BOLD_READY.store(bound, std::sync::atomic::Ordering::Relaxed);
            }
        } else {
            self.home_styled = false;
            BOLD_READY.store(false, std::sync::atomic::Ordering::Relaxed);
        }
        for p in std::mem::take(&mut self.pending) {
            self.open_file(ctx, &p);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let dirty = self.entries.iter().any(|e| e.tab.is_dirty());
            web::set_dirty(dirty);
            if host::active() {
                host::report_dirty(dirty);
                for (name, bytes) in host::take_arrived() {
                    // An empty file is one the host just created ("New document" in OpenCloud):
                    // a blank tab of its kind, not a parse error.
                    match kind_for(&name) {
                        Some(kind) if bytes.is_empty() => {
                            let id = self.push_tab(kind, Tab::create(kind));
                            if let Some(e) = self.entry_mut(id) {
                                e.name = name;
                            }
                            ctx.request_repaint();
                        }
                        _ => self.open_bytes(ctx, name, bytes),
                    }
                }
            }
            let arrived = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(|e| e.into_inner()));
            for (name, bytes) in arrived {
                self.open_bytes(ctx, name, bytes);
            }
            let scanned = std::mem::take(&mut *self.folder_scans.lock().unwrap_or_else(|e| e.into_inner()));
            for FolderScan { key, rows, added, .. } in scanned {
                let i = match self.folders.iter().position(|f| *f == key) {
                    Some(i) => i,
                    None => {
                        self.folders.push(key.clone());
                        self.folders.len() - 1
                    }
                };
                self.folder_counts.resize(self.folders.len(), 0);
                self.folder_counts[i] = rows.len();
                if added {
                    self.folder_sel = Some(i);
                }
                if self.folder_sel == Some(i) {
                    self.folder_rows = rows.clone();
                }
                self.scans.insert(key, rows);
                ctx.request_repaint();
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            // First frame (and after a folder is added or removed): count the sidebar folders.
            if self.folder_counts.len() != self.folders.len() {
                self.recount(ctx);
            }
            // Coming back to the window: files may have changed elsewhere, list them again.
            let focused = ctx.input(|i| i.focused);
            if focused && !self.focused && self.view == View::Home {
                self.recount(ctx);
            }
            self.focused = focused;
            self.take_listings();
            self.poll_word_recovery();
        }
        if self.restore_home && self.pending.is_empty() {
            self.view = View::Home;
            self.restore_home = false;
        }
        self.sync_paths();
        self.save_session_if_changed();
        // The fonts put back last frame are active now: the app's own style can follow.
        if self.settle == 1
            && let Some(k) = self.restyle.take()
            && let Some(look) = self.looks.get(&k)
        {
            look.apply_style(ctx);
            ctx.request_repaint();
        }
        self.prune_tiles();
        let tiled = self.tiled_ids();
        if let Some(ids) = &tiled {
            // Several apps at once: the one-owner font handling below does not apply.
            self.ctx_owner = None;
            self.tile_logic(ctx, frame, ids);
        }
        if let View::Tab(id) = self.view
            && tiled.is_none()
            && let Some(e) = self.entries.iter_mut().find(|e| e.id == id)
        {
            if self.ctx_owner != Some(e.kind) {
                // The app that was in front may have left a style naming font families of its own
                // (viSHual's "semibold"): until this one installs its own, the shell's own widgets
                // must only meet the built-in families.
                ctx.all_styles_mut(|s| *s = egui::Style::default());
                e.tab.context_changed(ctx);
                self.ctx_owner = Some(e.kind);
                self.owner_frames = 0;
                // verSHion installs its fonts there, and its first screen names them ("vs-serif")
                // straight away: they are only bound on the next pass, so draw nothing until then.
                if e.kind == Kind::Vershion {
                    self.settle = 2;
                    ctx.request_repaint();
                }
                if restores_look(e.kind)
                    && let Some(look) = self.looks.get(&e.kind)
                {
                    look.apply_fonts(ctx);
                    // New fonts take effect on the next pass: draw nothing until then.
                    self.settle = 2;
                    self.restyle = Some(e.kind);
                    ctx.request_repaint();
                }
            }
            self.owner_frames = self.owner_frames.saturating_add(1);
            e.tab.logic(ctx, frame);
        }
        if let View::Tab(id) = self.view
            && let Some(e) = self.entries.iter_mut().find(|e| e.id == id)
        {
            let app = ai_app(e.kind);
            let mut host = TabHost { tab: &mut e.tab, client: &mut e.pdf_client, ctx };
            e.chat.poll(&mut host, app, &self.ai_cfg, ctx);
        }
        self.open_requested_decks(ctx);
        self.equation_inserts(ctx);
        #[cfg(not(target_arch = "wasm32"))]
        self.serve_mcp(ctx);
        self.make_requested_graphs(ctx);
        self.open_double_clicked_graphs(ctx);
        // `OFFICECRAFT_AUTOSEND=<prompt>`: once a tab is open, show its assistant and send the prompt
        // (a demo: the window stays open and the user watches).
        static AUTOSENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if let Ok(prompt) = std::env::var("OFFICECRAFT_AUTOSEND")
            && self.pending.is_empty()
            && let Some(id) = self.entries.iter().find(|e| e.kind == Kind::Word).map(|e| e.id)
            && !AUTOSENT.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            let cfg = self.ai_cfg.clone();
            self.view = View::Tab(id);
            if let Some(e) = self.entry_mut(id) {
                e.chat.open = true;
                e.chat.ask_text(prompt, ai::App::Docs, &cfg, ctx);
            }
        }
        if let Ok(dir) = std::env::var("OFFICECRAFT_SELFTEST_GRAPH") {
            self.graph_selftest(ctx, &dir);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Ok(dir) = std::env::var("OFFICECRAFT_SELFTEST_SENDTO")
            && ctx.cumulative_frame_nr() >= 3
        {
            self.send_selftest(ctx, &dir);
        }
        if let Some((prompt, started)) = self.ai_selftest.clone() {
            self.drive_ai_selftest(ctx, prompt, started);
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // The window is asked to close while a tab holds unsaved work: the shell asks once, for
        // every tab. The request is taken out of the input, so no app shows its own "Save
        // changes?" on top (DeckCraft and PdfCraft would) and eframe keeps the window open.
        #[cfg(not(target_arch = "wasm32"))]
        if !self.quit_ok
            && let Some(info) = raw.viewports.get_mut(&egui::ViewportId::ROOT)
            && info.events.contains(&egui::ViewportEvent::Close)
            && !self.dirty_names().is_empty()
        {
            info.events.retain(|e| *e != egui::ViewportEvent::Close);
            self.confirm_quit = Some(Quit::Close);
            ctx.request_repaint();
        }
        // Files of a type the suite opens get their own tab, whichever tab is showing; anything
        // else (a picture dropped on a document) stays for the active app.
        let mut keep = Vec::new();
        for f in std::mem::take(&mut raw.dropped_files) {
            // A browser hands over a name and reads the bytes asynchronously.
            #[cfg(target_arch = "wasm32")]
            if let Some(name) = f.path().file_name().map(|n| n.to_string_lossy().to_string()).filter(|n| kind_for(n).is_some()) {
                web::read_dropped(f, name, self.inbox.clone(), ctx.clone());
                continue;
            }
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
        let mut send: Option<(u64, SendTo)> = None;
        let mut toggle_tile = false;
        let mut drag_now: Option<u64> = None;
        let mut drop_tab: Option<(u64, egui::Pos2)> = None;
        // Where each tab was drawn this frame: drop positions for reordering by drag.
        let mut tab_rects: Vec<(u64, egui::Rect)> = Vec::new();
        let mut bar_rect = egui::Rect::NOTHING;
        let mut tile_in: Option<u64> = None;
        let mut tile_out: Option<u64> = None;
        // Tiled: the shell's own bar takes the front app's colours, not whichever installed last.
        if self.tiled_ids().is_some()
            && let View::Tab(id) = self.view
            && let Some(kind) = self.entries.iter().find(|e| e.id == id).map(|e| e.kind)
            && let Some(style) = self.tile_styles.get(&kind)
        {
            ui.set_style(style.clone());
        }
        let mut open_settings = false;
        let bar_frame = if self.view == View::Home {
            egui::Frame::new().fill(egui::Color32::from_rgb(223, 229, 236)).inner_margin(egui::Margin::symmetric(8, 4))
        } else {
            egui::Frame::side_top_panel(ui.style())
        };
        egui::Panel::top("officecraft_tabs").exact_size(34.0).frame(bar_frame).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                let home_on = self.view == View::Home;
                let (hr, hresp) = ui.allocate_exact_size(egui::vec2(64.0, 26.0), egui::Sense::click());
                if home_on {
                    ui.painter().rect_filled(hr, egui::CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }, egui::Color32::from_rgb(237, 241, 245));
                }
                let (hf, hc) = if home_on { (egui::FontId::new(14.0, bold()), INK) } else { (egui::FontId::proportional(14.0), ui.visuals().text_color()) };
                ui.painter().text(hr.center(), egui::Align2::CENTER_CENTER, "Home", hf, hc);
                if hresp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    select = Some(View::Home);
                    self.show_trash = false;
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        self.purge_confirm = None;
                    }
                }
                bar_rect = ui.max_rect();
                // Many tabs: the strip scrolls (mouse wheel) and keeps the active tab in view; the
                // ⋯ menu lists them all. Room is left for the controls on the right.
                let strip_w = (ui.available_width() - 300.0).max(120.0);
                let strip = egui::ScrollArea::horizontal()
                    .id_salt("tab_strip")
                    .max_width(strip_w)
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| { ui.horizontal_centered(|ui| {
                for e in &self.entries {
                    let title = if e.tab.is_dirty() { format!("{} •", e.name) } else { e.name.clone() };
                    let (stripe, _) = ui.allocate_exact_size(egui::vec2(3.0, 14.0), egui::Sense::hover());
                    ui.painter().rect_filled(stripe, 1.5, kind_color(Some(e.kind)));
                    let shown = title;
                    let r = ui.selectable_label(self.view == View::Tab(e.id), shown).on_hover_text(e.path.as_deref().unwrap_or("Not saved yet"));
                    // Also draggable (along the bar to reorder, onto the content to tile) and
                    // right-clickable.
                    let r = r.interact(egui::Sense::click_and_drag());
                    let tileable = e.kind.tileable();
                    if self.view == View::Tab(e.id) && self.tab_in_view != Some(e.id) {
                        r.scroll_to_me(Some(egui::Align::Center));
                        self.tab_in_view = Some(e.id);
                    }
                    if r.clicked() {
                        select = Some(View::Tab(e.id));
                    }
                    if r.dragged() {
                        drag_now = Some(e.id);
                    }
                    if r.drag_stopped() {
                        drop_tab = ui.ctx().input(|i| i.pointer.latest_pos()).map(|p| (e.id, p));
                    }
                    let in_tiles = self.tiles.len() >= 2 && self.tiles.contains(&e.id);
                    r.context_menu(|ui| {
                        if tileable {
                            if in_tiles {
                                if ui.button("Remove from the tiles").clicked() {
                                    tile_out = Some(e.id);
                                    ui.close();
                                }
                            } else if ui.button("Tile this tab").clicked() {
                                tile_in = Some(e.id);
                                ui.close();
                            }
                        }
                        let targets = Suite::send_targets(&self.entries, e.id);
                        if !targets.is_empty() {
                            ui.menu_button("Send to", |ui| {
                                for (label, to) in &targets {
                                    if ui.button(label).clicked() {
                                        send = Some((e.id, *to));
                                        ui.close();
                                    }
                                }
                            });
                        }
                        if ui.button("Close tab").clicked() {
                            close = Some(e.id);
                            ui.close();
                        }
                    });
                    let x = ui.small_button("×").on_hover_text("Close tab");
                    if x.clicked() {
                        close = Some(e.id);
                    }
                    tab_rects.push((e.id, stripe.union(r.rect).union(x.rect)));
                    ui.separator();
                }
                }); });
                self.tabs_overflow = strip.content_size.x > strip.inner_rect.width() + 1.0;
                if self.tabs_overflow {
                    ui.menu_button("⋯", |ui| {
                        for e in &self.entries {
                            if ui.selectable_label(self.view == View::Tab(e.id), &e.name).clicked() {
                                select = Some(View::Tab(e.id));
                                ui.close();
                            }
                        }
                    })
                    .response
                    .on_hover_text("All open tabs");
                }
                ui.menu_button("+", |ui| {
                    for kind in Kind::ALL {
                        if ui.button(format!("New {}", kind.label())).clicked() {
                            new = Some(kind);
                            ui.close();
                        }
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⚙").on_hover_text("Assistant settings: provider, endpoint, API keys").clicked() {
                        open_settings = true;
                    }
                    // A page cannot rebuild itself; the updater is for the installed app.
                    #[cfg(not(target_arch = "wasm32"))]
                    if ui.button("⟳").on_hover_text("Updates: rebuild against the newest app commits").clicked() {
                        self.updater.open = !self.updater.open;
                    }
                    if let View::Tab(id) = self.view {
                        let targets = Suite::send_targets(&self.entries, id);
                        if !targets.is_empty() {
                            ui.menu_button("Send to ▾", |ui| {
                                for (label, to) in &targets {
                                    if ui.button(label).clicked() {
                                        send = Some((id, *to));
                                        ui.close();
                                    }
                                }
                            })
                            .response
                            .on_hover_text("Make this into a document of another app, or a PDF");
                        }
                    }
                    if let View::Tab(id) = self.view
                        && self.entries.iter().find(|e| e.id == id).is_some_and(|e| e.kind.has_assistant())
                    {
                        let open = self.entries.iter().find(|e| e.id == id).is_some_and(|e| e.chat.open);
                        if ui.selectable_label(open, "✨ Assistant").clicked() {
                            toggle_chat = Some(id);
                        }
                    }
                    let tiled = self.tiles.len() >= 2;
                    let hint = if tiled { "Back to one document at a time" } else { "Show open documents side by side" };
                    let can = tiled || self.entries.iter().filter(|e| e.kind.tileable()).count() >= 2;
                    if ui.add_enabled_ui(can, |ui| ui.selectable_label(tiled, "▥ Tile").on_hover_text(hint).clicked()).inner {
                        toggle_tile = true;
                    }
                });
            });
        });
        if open_settings && self.ai_settings.is_none() {
            self.ai_settings = Some(ai::Settings::new(&self.ai_cfg));
        }
        if let Some(st) = &mut self.ai_settings
            && let Some(outcome) = st.ui(ui.ctx())
        {
            if let Some(cfg) = outcome {
                self.ai_cfg = cfg;
                ai::refresh_models();
                // Claude Code sessions the user starts: the server stays up from now on.
                #[cfg(not(target_arch = "wasm32"))]
                if self.ai_cfg.mcp_listen
                    && let Err(e) = mcp::start(ui.ctx())
                {
                    self.toast = Some((format!("Couldn't start the Claude Code connection: {e}"), true, ui.ctx().input(|i| i.time)));
                }
            }
            self.ai_settings = None;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.updater.ui(ui.ctx()) {
            self.leave(ui.ctx(), Quit::Restart);
        }
        // Leaving was decided: keep asking until the window closes (a close the desktop sent
        // in the same frame may have been cancelled).
        #[cfg(not(target_arch = "wasm32"))]
        if self.leaving {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            ui.ctx().request_repaint();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(why) = self.confirm_quit {
            let dirty = self.dirty_names();
            let mut back = false;
            let mut go = false;
            let title = if why == Quit::Restart { "Restart without saving?" } else { "Quit without saving?" };
            egui::Window::new(title).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
                ui.label(if dirty.len() == 1 { "This document has unsaved changes:".to_string() } else { format!("{} documents have unsaved changes:", dirty.len()) });
                for (_, name) in dirty.iter().take(8) {
                    ui.label(format!("  •  {name}"));
                }
                if dirty.len() > 8 {
                    ui.label(format!("  …and {} more", dirty.len() - 8));
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Go back and save").clicked() {
                        back = true;
                    }
                    let verb = if why == Quit::Restart { "Restart and discard" } else { "Quit and discard" };
                    if ui.button(verb).clicked() {
                        go = true;
                    }
                });
            });
            if back {
                self.confirm_quit = None;
                if let Some((id, _)) = dirty.first() {
                    self.view = View::Tab(*id);
                }
            }
            if go {
                self.confirm_quit = None;
                self.quit_ok = true;
                self.leave(ui.ctx(), why);
            }
        }
        if let Some(id) = toggle_chat
            && let Some(e) = self.entry_mut(id)
        {
            e.chat.open = !e.chat.open;
        }
        if let Some((from, to)) = send {
            self.send_to(ui.ctx(), from, to);
        }
        self.toast_ui(ui.ctx());
        if let Some(id) = tile_in {
            self.tile_with(id, None);
        }
        if let Some(id) = tile_out {
            self.take_out_tile(id);
        }
        if let Some((id, p)) = drop_tab
            && p.y <= bar_rect.bottom() + 6.0
        {
            // Released on the tab bar: the tab moves to where it was dropped.
            if let Some(to) = tab_insert_index(&tab_rects, id, p.x)
                && let Some(from) = self.entries.iter().position(|e| e.id == id)
            {
                let e = self.entries.remove(from);
                let to = if to > from { to - 1 } else { to }.min(self.entries.len());
                self.entries.insert(to, e);
            }
        } else if let Some((id, p)) = drop_tab
            && self.entries.iter().any(|e| e.id == id && e.kind.tileable())
        {
            // Released over the content, not back on the tab bar.
            if p.y > ui.max_rect().min.y + 44.0 && !self.tiles.contains(&id) {
                let room = self.tile_layout.len() < self.tile_max_cols;
                let at = if self.tiles.len() >= 2 { drop_target(&self.tile_rects, id, p, room).map(|(t, z, _)| (t, z)) } else { None };
                self.tile_with(id, at);
            }
        }
        self.tab_drag = drag_now;
        if let Some(id) = drag_now
            && let Some(p) = ui.ctx().input(|i| i.pointer.latest_pos())
        {
            let name = self.entries.iter().find(|e| e.id == id).map(|e| e.name.clone()).unwrap_or_default();
            let tileable = self.entries.iter().any(|e| e.id == id && e.kind.tileable());
            let layer = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("tab-drag")));
            let on_bar = p.y <= bar_rect.bottom() + 6.0;
            if on_bar {
                // Where the tab would land: a caret between tabs.
                let others: Vec<&egui::Rect> = tab_rects.iter().filter(|(t, _)| *t != id).map(|(_, r)| r).collect();
                let before = others.iter().filter(|r| r.center().x < p.x).count();
                let x = match others.get(before) {
                    Some(r) => Some(r.left() - 3.0),
                    None => others.last().map(|r| r.right() + 3.0),
                };
                if let Some(x) = x {
                    layer.vline(x, bar_rect.top() + 4.0..=bar_rect.bottom() - 4.0, egui::Stroke::new(2.0, ACCENT));
                }
                layer.text(p + egui::vec2(14.0, 14.0), egui::Align2::LEFT_TOP, format!("Move {name}"), egui::FontId::proportional(13.0), ACCENT);
            } else if tileable {
                layer.text(p + egui::vec2(14.0, 14.0), egui::Align2::LEFT_TOP, format!("Tile {name}"), egui::FontId::proportional(13.0), ACCENT);
            }
            ui.ctx().request_repaint();
        }
        if toggle_tile {
            if self.tiles.len() >= 2 {
                self.untile();
            } else {
                self.tile_all();
            }
        }
        // The prompt is shell UI drawn with whatever style the context holds. Right after an app is
        // shown, its style is in place a pass before its fonts are, and naming a family that is not
        // bound yet panics: wait until the front app has settled.
        let front_settled = match self.view {
            View::Tab(id) => {
                self.settle == 0
                    && self.owner_frames >= 6
                    && self.entries.iter().find(|e| e.id == id).is_some_and(|e| self.ctx_owner == Some(e.kind))
            }
            View::Home => self.home_styled,
        };
        if self.tile_prompt && self.tiles.len() < 2 && !front_settled {
            // An idle window draws no frames, and the front app needs a few to settle.
            ui.ctx().request_repaint();
        }
        if self.tile_prompt && self.tiles.len() < 2 && front_settled {
            let mut choice = None;
            egui::Window::new("Tile side by side?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::RIGHT_TOP, [-12.0, 46.0])
                .show(ui.ctx(), |ui| {
                    ui.label("You have more than one document open. Show them next to each other?");
                    ui.checkbox(&mut self.tile_remember, "Remember my choice");
                    ui.horizontal(|ui| {
                        if ui.button("Tile").clicked() {
                            choice = Some(true);
                        }
                        if ui.button("Keep as tabs").clicked() {
                            choice = Some(false);
                        }
                    });
                });
            if let Some(tile) = choice {
                if self.tile_remember {
                    self.tile_pref = if tile { TilePref::Always } else { TilePref::Never };
                    self.tile_pref.save();
                }
                self.tile_prompt = false;
                if tile {
                    self.tile_all();
                }
            }
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
            let accent = kind_color(Some(e.kind));
            let models = ai::models(&self.ai_cfg, ui.ctx());
            let mut action = None;
            egui::Panel::right("officecraft_ai").default_size(400.0).min_size(300.0).resizable(true).show(ui, |ui| {
                action = e.chat.ui(ui, app, &self.ai_cfg, accent, &e.name, &models);
            });
            match action {
                Some(ai::Action::Send) => e.chat.send(app, &self.ai_cfg, ui.ctx()),
                Some(ai::Action::SetModel(m)) => {
                    self.ai_cfg.set_model(&m);
                    let _ = self.ai_cfg.save();
                }
                Some(ai::Action::OpenSettings) => {
                    if self.ai_settings.is_none() {
                        self.ai_settings = Some(ai::Settings::new(&self.ai_cfg));
                    }
                }
                None => {}
            }
        }
        // A Graph tab opened from a picture in a document: put the edit back, or drop it.
        if let View::Tab(id) = self.view
            && let Some((doc_id, doc_name)) = self
                .entries
                .iter()
                .find(|e| e.id == id)
                .and_then(|e| e.graph_for)
                .and_then(|d| self.entries.iter().find(|x| x.id == d).map(|x| (d, x.name.clone())))
        {
            let (mut update, mut cancel) = (false, false);
            egui::Panel::top("officecraft_graph_link").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("Editing the graph picture in {doc_name}"));
                    if ui.button("Update picture").on_hover_text("Replace the picture in the document with this graph").clicked() {
                        update = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
            if update {
                self.update_graph_picture(id, ui.ctx());
            } else if cancel {
                self.close(id);
                self.view = View::Tab(doc_id);
            }
        }
        match self.view {
            View::Home => {
                self.settle = self.settle.saturating_sub(1);
                self.home(ui)
            }
            View::Tab(_) if self.tiled_ids().is_some() => {
                self.settle = self.settle.saturating_sub(1);
                let ids = self.tiled_ids().unwrap_or_default();
                self.tile_ui(ui, frame, &ids);
            }
            View::Tab(id) => {
                let owner = self.ctx_owner;
                let cfg = self.ai_cfg.clone();
                let settling = self.settle > 0;
                self.settle = self.settle.saturating_sub(1);
                if let Some(e) = self.entry_mut(id) {
                    // A tab clicked in this very frame has not had its fonts and visuals
                    // installed yet (`logic` does that at the start of the next frame): drawing it
                    // now would use another app's font families, which panics.
                    if owner == Some(e.kind) && !settling {
                        e.tab.ui(ui, frame);
                        // viSHual's own chat box asks the suite's assistant: one AI key for all.
                        if let Tab::Graph(g) = &mut e.tab {
                            for r in g.take_requests() {
                                let vishual_ui_egui::Request::Generate(prompt) = r;
                                e.chat.open = true;
                                e.chat.ask_text(vishual_mcp::generate_prompt(&prompt), ai_app(e.kind), &cfg, ui.ctx());
                            }
                        }
                    } else {
                        ui.ctx().request_repaint();
                    }
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

#[cfg(target_arch = "wasm32")]
fn main() {
    web::start();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    // Window/taskbar icon; the installed copies under hicolor are what the desktop entry uses.
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/app-icon/hicolor/256x256/apps/ai.storyteller.officecraft.png"))
        .expect("bundled app icon is a valid png");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("OfficeCraft")
            .with_app_id("ai.storyteller.officecraft")
            .with_icon(icon)
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
    if let Ok(p) = std::env::var("OFFICECRAFT_SHOT") {
        suite.shot = Some((p, 0));
    }
    if std::env::var_os("OFFICECRAFT_TILES").is_some() {
        suite.tiles_test = Some(0);
    }
    let selftest = suite.selftest.is_some() || suite.ai_selftest.is_some() || suite.shot.is_some() || suite.tiles_test.is_some();
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
    eframe::run_native(
        "OfficeCraft",
        options,
        Box::new(move |cc| {
            if !selftest
                && std::env::var_os("OFFICECRAFT_NO_RECOVERY").is_none()
                && let Some(dir) = platform::data_dir()
            {
                let _ = RECOVERY.set((dir.join("recovery"), cc.egui_ctx.clone()));
                suite.reopen_recovered();
            }
            Ok(Box::new(suite))
        }),
    )
}

/// New from prompt: the app a description asks for, when the user leaves the choice to us. A
/// word for slides, a sheet or a graph picks that app; anything else is a document.
fn kind_for_prompt(text: &str) -> Kind {
    let t = text.to_lowercase();
    let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |list: &[&str]| words.iter().any(|w| list.contains(w));
    // Order matters: "a slide deck with a chart" is slides, "a spreadsheet to graph" a sheet.
    if has(&["slide", "slides", "deck", "presentation", "presentations", "slideshow", "powerpoint", "pptx", "keynote"]) {
        Kind::Deck
    } else if has(&["spreadsheet", "spreadsheets", "sheet", "sheets", "workbook", "budget", "gradebook", "ledger", "tracker", "excel", "xlsx", "csv"]) {
        Kind::Grid
    } else if has(&["graph", "graphs", "plot", "plots", "desmos"]) && !has(&["essay", "letter", "report", "document"]) {
        Kind::Graph
    } else {
        Kind::Word
    }
}

/// What the assistant of a new, empty tab is asked, around the user's own words.
fn prompt_for_new(kind: Kind, text: &str) -> String {
    let what = match kind {
        Kind::Grid => "workbook",
        Kind::Deck => "presentation",
        _ => "document",
    };
    format!("This is a new, empty {what}. Make it from this description, all of it, in this {what}:\n\n{}", text.trim())
}

#[cfg(test)]
mod prompt_tests {
    use super::*;

    #[test]
    fn a_description_picks_its_app() {
        assert!(kind_for_prompt("A 6-slide lesson on fractions") == Kind::Deck);
        assert!(kind_for_prompt("presentation about the water cycle with a chart") == Kind::Deck);
        assert!(kind_for_prompt("Monthly budget for a family of four") == Kind::Grid);
        assert!(kind_for_prompt("a gradebook for 30 students") == Kind::Grid);
        assert!(kind_for_prompt("Plot y = x^2 and its derivative") == Kind::Graph);
        assert!(kind_for_prompt("A one-page field trip letter to parents") == Kind::Word);
        assert!(kind_for_prompt("a report that includes a graph of sales") == Kind::Word);
        // Whole words only: "sheetrock" is not a sheet, "slideshow" is slides.
        assert!(kind_for_prompt("how to hang sheetrock") == Kind::Word);
        assert!(kind_for_prompt("Slideshow: our trip") == Kind::Deck);
    }

    #[test]
    fn the_new_tab_prompt_names_the_app() {
        let p = prompt_for_new(Kind::Deck, "  five slides on volcanoes ");
        assert!(p.contains("new, empty presentation") && p.ends_with("five slides on volcanoes"));
        assert!(prompt_for_new(Kind::Word, "x").contains("document"));
    }
}

#[cfg(test)]
mod tab_order_tests {
    use super::tab_insert_index;

    /// Tabs A..D at x = 0, 100, 200, 300 (60 wide), ids 1..4.
    fn rects() -> Vec<(u64, egui::Rect)> {
        (0..4).map(|i| (i as u64 + 1, egui::Rect::from_min_size(egui::pos2(i as f32 * 100.0, 0.0), egui::vec2(60.0, 20.0)))).collect()
    }

    /// Applies a drop the way the tab bar does and returns the new order.
    fn drop(id: u64, x: f32) -> Vec<u64> {
        let mut order: Vec<u64> = (1..=4).collect();
        let to = tab_insert_index(&rects(), id, x).unwrap();
        let from = order.iter().position(|t| *t == id).unwrap();
        let t = order.remove(from);
        let to = if to > from { to - 1 } else { to }.min(order.len());
        order.insert(to, t);
        order
    }

    #[test]
    fn drops_land_between_the_tabs_under_the_pointer() {
        assert_eq!(drop(2, 400.0), [1, 3, 4, 2]);
        assert_eq!(drop(4, -10.0), [4, 1, 2, 3]);
        assert_eq!(drop(2, 250.0), [1, 3, 2, 4]);
        assert_eq!(drop(3, 50.0), [1, 3, 2, 4]);
        assert_eq!(drop(2, 60.0), [1, 2, 3, 4]);
        assert!(tab_insert_index(&rects()[..1], 1, 10.0).is_none());
    }
}
