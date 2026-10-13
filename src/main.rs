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
mod equation;
mod platform;
#[cfg(not(target_arch = "wasm32"))]
mod updater;
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
}

impl Kind {
    const ALL: [Kind; 6] = [Kind::Word, Kind::Grid, Kind::Deck, Kind::Pdf, Kind::Graph, Kind::Equation];
    /// Apps that bring their own fonts and layout (never tiled beside another app).
    fn tileable(self) -> bool {
        !matches!(self, Kind::Graph | Kind::Equation)
    }
    /// Whether the suite's assistant can work in this app.
    fn has_assistant(self) -> bool {
        self != Kind::Equation
    }
    fn label(self) -> &'static str {
        match self {
            Kind::Word => "Docs",
            Kind::Grid => "Sheets",
            Kind::Deck => "Slides",
            Kind::Pdf => "PDF",
            Kind::Graph => "Graph",
            Kind::Equation => "Equations",
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
            Kind::Graph => Tab::Graph(Box::new(vishual_ui_egui::VishualApp::new(graph_services()))),
            Kind::Equation => Tab::Equation(Box::new(equation::EquationApp::new())),
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        match self {
            Tab::Word(a) => a.logic(ctx),
            Tab::Grid(a) => a.logic(ctx),
            Tab::Deck(a) => a.logic(ctx),
            Tab::Pdf(a) => eframe::App::logic(a.as_mut(), ctx, frame),
            Tab::Graph(_) | Tab::Equation(_) => {}
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        match self {
            Tab::Word(a) => a.raw_input_hook(raw),
            Tab::Grid(a) => a.raw_input_hook(raw),
            Tab::Deck(a) => a.raw_input_hook(raw),
            Tab::Pdf(a) => eframe::App::raw_input_hook(a.as_mut(), ctx, raw),
            Tab::Graph(_) | Tab::Equation(_) => {}
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
    let ext = std::path::Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "docx" | "docm" | "dotx" | "odt" | "rtf" | "txt" | "md" | "html" | "htm" => Some(Kind::Word),
        "xlsx" | "xlsm" | "csv" | "tsv" => Some(Kind::Grid),
        "pptx" | "potx" | "ppsx" | "deckcraft" => Some(Kind::Deck),
        "pdf" => Some(Kind::Pdf),
        "vshl" | "vishual" => Some(Kind::Graph),
        _ => None,
    }
}

const OPEN_EXTS: &[&str] = &[
    "docx", "docm", "dotx", "odt", "rtf", "txt", "md", "html", "htm", "xlsx", "xlsm", "csv", "tsv", "pptx", "potx", "ppsx", "deckcraft", "pdf", "vshl", "vishual",
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
        }
    }

    /// The apps share one egui context and each installs its own fonts and visuals into it.
    /// After another app has drawn, this one must install them again.
    fn context_changed(&mut self, ctx: &egui::Context) {
        match self {
            Tab::Word(a) => a.context_changed(),
            Tab::Deck(a) => a.context_changed(),
            Tab::Graph(a) => a.context_changed(ctx),
            Tab::Equation(a) => a.context_changed(ctx),
            // GridCraft and PdfCraft keep their font state private: the shell puts their fonts
            // and style back itself (`Look`).
            Tab::Grid(_) | Tab::Pdf(_) => {}
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
            Tab::Deck(a) => push(a.services.inbox.as_ref()),
            Tab::Pdf(a) => push(Some(&a.startup_inbox)),
            Tab::Graph(_) | Tab::Equation(_) => {}
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

/// Apps whose own `context_changed` is not available to the shell; it restores their `Look`.
fn restores_look(kind: Kind) -> bool {
    matches!(kind, Kind::Grid | Kind::Pdf)
}

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

/// A folder listing the browser delivered: a folder the visitor just added (`added`) or a
/// fresh listing of one already in the sidebar.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct FolderScan {
    key: String,
    rows: Vec<Row>,
    added: bool,
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

/// Supported files in `dir` and the folders two levels below it, newest first.
fn list_folder(dir: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let mut stack = vec![(std::path::PathBuf::from(dir), 0u8)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&d) else { continue };
        for e in read.flatten() {
            let path = e.path();
            let hidden = path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
            let Ok(ft) = e.file_type() else { continue };
            if hidden {
                continue;
            }
            if ft.is_dir() && depth < 2 {
                stack.push((path, depth + 1));
            } else if ft.is_file()
                && let Some(p) = path.to_str().filter(|p| kind_for(p).is_some())
            {
                rows.push(row_for(p));
            }
        }
    }
    rows.sort_by(|a, b| b.modified.cmp(&a.modified));
    rows.truncate(500);
    rows
}

/// Folders the visitor added. A page can't reopen a folder without a fresh pick, so the browser
/// build starts with none and keeps them for the session only.
fn load_folders() -> Vec<String> {
    if cfg!(target_arch = "wasm32") {
        return Vec::new();
    }
    platform::read_lines("folders.txt")
}

fn save_folders(list: &[String]) {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    platform::write_lines("folders.txt", list);
}

/// The rows of folder `dir`: read from disk natively, the browser's last listing in a page.
fn folder_listing(scans: &std::collections::HashMap<String, Vec<Row>>, dir: &str) -> Vec<Row> {
    #[cfg(target_arch = "wasm32")]
    {
        scans.get(dir).cloned().unwrap_or_default()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = scans;
        list_folder(dir)
    }
}

fn kind_color(kind: Option<Kind>) -> egui::Color32 {
    match kind {
        Some(Kind::Word) => egui::Color32::from_rgb(47, 99, 198),
        Some(Kind::Grid) => egui::Color32::from_rgb(31, 143, 92),
        Some(Kind::Deck) => egui::Color32::from_rgb(212, 98, 46),
        Some(Kind::Pdf) => egui::Color32::from_rgb(194, 59, 69),
        Some(Kind::Graph) => egui::Color32::from_rgb(183, 121, 31),
        Some(Kind::Equation) => egui::Color32::from_rgb(98, 76, 168),
        None => egui::Color32::GRAY,
    }
}

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
        None => "?",
    }
}

fn paint_folder(p: &egui::Painter, c: egui::Pos2, col: egui::Color32) {
    let st = egui::Stroke::new(1.4, col);
    let r = egui::Rect::from_center_size(c + egui::vec2(0.0, 1.0), egui::vec2(14.0, 10.0));
    p.rect_stroke(r, 2.0, st, egui::StrokeKind::Inside);
    p.line_segment([r.left_top() + egui::vec2(1.0, -2.0), r.left_top() + egui::vec2(5.5, -2.0)], st);
}

fn paint_search(p: &egui::Painter, c: egui::Pos2, col: egui::Color32) {
    let st = egui::Stroke::new(1.6, col);
    p.circle_stroke(c + egui::vec2(-1.5, -1.5), 5.0, st);
    p.line_segment([c + egui::vec2(2.0, 2.0), c + egui::vec2(6.0, 6.0)], st);
}

/// One sidebar row: optional folder icon, label, count on the right (or a remove cross on hover).
fn side_row(ui: &mut egui::Ui, selected: bool, icon: bool, label: &str, count: Option<usize>, removable: bool) -> (egui::Response, bool) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), egui::Sense::click());
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
    if removable && hovered {
        let xr = egui::Rect::from_center_size(egui::pos2(rect.right() - 18.0, rect.center().y), egui::vec2(20.0, 20.0));
        p.text(xr.center(), egui::Align2::CENTER_CENTER, "×", egui::FontId::proportional(17.0), MUTED);
        removed = resp.clicked() && resp.interact_pointer_pos().is_some_and(|q| xr.expand(6.0).contains(q));
    } else if let Some(n) = count {
        let col = if selected { ACCENT } else { MUTED };
        p.text(egui::pos2(rect.right() - 12.0, rect.center().y), egui::Align2::RIGHT_CENTER, n.to_string(), egui::FontId::proportional(13.0), col);
    }
    (resp.on_hover_cursor(egui::CursorIcon::PointingHand), removed)
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
        let mut folders = load_folders();
        // First run: show ~/Documents so Home is not an empty page.
        if folders.is_empty()
            && let Some(d) = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Documents")).filter(|d| d.is_dir())
        {
            folders.push(d.to_string_lossy().to_string());
        }
        let recent = load_recent();
        let (folder_sel, folder_rows) = match folders.first() {
            Some(f) if recent.is_empty() => (Some(0), list_folder(f)),
            _ => (None, Vec::new()),
        };
        let mut suite = Suite { view: View::Home, entries: Vec::new(), next_id: 1, recent, pending: Vec::new(), notice: None, confirm_close: None, ai_cfg: ai::Config::load(), ai_settings: None, #[cfg(not(target_arch = "wasm32"))] updater: updater::Updater::new(), ctx_owner: None, selftest: None, ai_selftest: None, persist: true, last_session: Vec::new(), restore_home: false, filter: Filter::All, search: String::new(), folders, folder_sel, folder_rows, home_grid: load_home_grid(), shot: None, shot_sent: false, home_styled: false, folder_counts: Vec::new(), folder_scans: FolderScans::default(), scans: std::collections::HashMap::new(), inbox: Inbox::default(), looks: std::collections::HashMap::new(), owner_frames: 0, settle: 0, restyle: None, tiles: Vec::new(), tile_layout: Vec::new(), tile_rects: Vec::new(), tile_max_cols: 2, tab_drag: None, tile_test_out: None, tile_pref: TilePref::load(), tile_prompt: false, tile_remember: false, fonts_union: None, tile_settle: 0, tile_running: Vec::new(), tile_warm: std::collections::HashMap::new(), tile_styles: std::collections::HashMap::new(), tile_restores: Vec::new(), tile_frame: 0, tiles_test: None };
        suite.recount();
        suite
    }

    fn recount(&mut self) {
        self.folder_counts = self.folders.iter().map(|f| folder_listing(&self.scans, f).len()).collect();
    }

    /// Starts a fresh, empty instance of `kind` and shows it.
    /// A tab with a new, blank file in it (what New file makes).
    fn new_tab(&mut self, kind: Kind) -> u64 {
        let tab = match kind {
            // A presentation needs a slide to work on: an assistant (or a first click) has nothing
            // to act on in an empty session.
            Kind::Deck => Tab::Deck(Box::new(deckcraft_ui_egui::SlideApp::new(deckcraft_engine::Session::with_new(), deck_services()))),
            _ => Tab::create(kind),
        };
        self.push_tab(kind, tab)
    }

    fn push_tab(&mut self, kind: Kind, tab: Tab) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let n = self.entries.iter().filter(|e| e.kind == kind).count() + 1;
        self.entries.push(Entry { id, kind, name: format!("{} {n}", kind.label()), path: None, tab, chat: ai::Chat::new(), pdf_client: None });
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
        self.entries.retain(|e| e.id != id);
        self.confirm_close = None;
        self.prune_tiles();
        if self.view == View::Tab(id) {
            self.view = self.entries.last().map(|e| View::Tab(e.id)).unwrap_or(View::Home);
        }
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
            let id = self.new_tab(kind);
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

        let mut want_grid = self.home_grid;

        let source: Vec<Row> = match self.folder_sel {
            None => self.recent.iter().map(|p| row_for(p)).collect(),
            Some(_) => self.folder_rows.clone(),
        };
        let q = self.search.to_lowercase();
        let rows: Vec<Row> = source
            .iter()
            .filter(|r| match want_filter {
                Filter::All => true,
                Filter::Only(k) => r.kind == Some(k),
            })
            .filter(|r| q.is_empty() || r.name.to_lowercase().contains(&q) || r.folder.to_lowercase().contains(&q))
            .cloned()
            .collect();
        let title = match self.folder_sel.and_then(|i| self.folders.get(i)) {
            Some(f) => std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.clone()),
            None => "Recent files".to_string(),
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
                    let target = if cfg!(target_arch = "wasm32") { "web" } else { "desktop" };
                    ui.label(egui::RichText::new(format!("{target} {}", env!("OFFICECRAFT_BUILD"))).size(10.0).color(MUTED))
                        .on_hover_text("The commit this build came from. The desktop and web builds match when these agree.");
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
                    let name = std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.clone());
                    let (resp, removed) = side_row(ui, self.folder_sel == Some(i), true, &name, self.folder_counts.get(i).copied(), true);
                    let resp = resp.on_hover_text(f);
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
                }
            });

        egui::CentralPanel::default().frame(egui::Frame::new().fill(egui::Color32::from_rgb(237, 241, 245)).inner_margin(egui::Margin::symmetric(36, 26))).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(b(&title, 24.0));
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
                    let open_btn = egui::Button::new(b("Open file", 14.0)).fill(egui::Color32::WHITE).stroke(egui::Stroke::new(1.0, LINE)).corner_radius(10.0).min_size(egui::vec2(0.0, 40.0));
                    if ui.add(open_btn).clicked() {
                        pick_files = true;
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
            }

            ui.add_space(18.0);
            let starts = [(Kind::Word, "WordCraft", "Docs"), (Kind::Grid, "GridCraft", "Sheets"), (Kind::Deck, "DeckCraft", "Slides"), (Kind::Pdf, "PdfCraft", "PDF"), (Kind::Graph, "viSHual", "Graphs"), (Kind::Equation, "equaSHion", "Equations")];
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.columns(6, |cols| {
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

            if rows.is_empty() {
                ui.add_space(24.0);
                ui.label(egui::RichText::new(if self.folder_sel.is_some() { "No supported files in this folder." } else { if cfg!(target_arch = "wasm32") { web_open_hint() } else { "Files you open appear here. Drop a file on the window, or press Open file." } }).color(MUTED));
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
                                let (rect, resp) = ui.allocate_exact_size(egui::vec2(card_w, 164.0), egui::Sense::click());
                                let resp = resp.on_hover_text(&r.path).on_hover_cursor(egui::CursorIcon::PointingHand);
                                let stroke = if resp.hovered() { egui::Stroke::new(1.5, ACCENT) } else { egui::Stroke::new(1.0, LINE) };
                                let p = ui.painter();
                                p.rect_filled(rect, 10.0, egui::Color32::WHITE);
                                let top = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 96.0));
                                p.rect_filled(top, egui::CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 }, kind_color(r.kind).gamma_multiply(0.14));
                                thumb(p, r.kind, top);
                                p.rect_stroke(rect, 10.0, stroke, egui::StrokeKind::Inside);
                                let name: String = if r.name.chars().count() > 26 { r.name.chars().take(25).collect::<String>() + "…" } else { r.name.clone() };
                                p.text(rect.min + egui::vec2(14.0, 110.0), egui::Align2::LEFT_TOP, name, egui::FontId::new(14.0, bold()), INK);
                                let sub = match r.modified {
                                    Some(t) => format!("{}, {}", r.folder, ago(t)),
                                    None => r.folder.clone(),
                                };
                                p.text(rect.min + egui::vec2(14.0, 136.0), egui::Align2::LEFT_TOP, sub, egui::FontId::proportional(12.0), MUTED);
                                if resp.clicked() {
                                    open = Some(r.path.clone());
                                }
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
                            let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 45.0), egui::Sense::click());
                            let resp = resp.on_hover_text(&r.path).on_hover_cursor(egui::CursorIcon::PointingHand);
                            let p = ui.painter();
                            if resp.hovered() {
                                p.rect_filled(rect, 0.0, egui::Color32::from_rgb(246, 248, 250));
                            }
                            let mid = rect.center().y;
                            let badge_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + 28.0, mid), egui::vec2(24.0, 24.0));
                            p.rect_filled(badge_rect, 6.0, kind_color(r.kind));
                            p.text(badge_rect.center(), egui::Align2::CENTER_CENTER, kind_abbr(r.kind), egui::FontId::new(11.0, bold()), egui::Color32::WHITE);
                            let clip = p.with_clip_rect(egui::Rect::from_min_max(rect.min + egui::vec2(cols[0], 0.0), egui::pos2(rect.left() + cols[1] - 12.0, rect.bottom())));
                            clip.text(egui::pos2(rect.left() + cols[0], mid), egui::Align2::LEFT_CENTER, &r.name, egui::FontId::new(14.0, bold()), INK);
                            let clip2 = p.with_clip_rect(egui::Rect::from_min_max(egui::pos2(rect.left() + cols[1], rect.top()), egui::pos2(rect.left() + cols[2] - 12.0, rect.bottom())));
                            clip2.text(egui::pos2(rect.left() + cols[1], mid), egui::Align2::LEFT_CENTER, &r.folder, egui::FontId::proportional(14.0), SOFT);
                            p.text(egui::pos2(rect.left() + cols[2], mid), egui::Align2::LEFT_CENTER, r.modified.map(ago).unwrap_or_default(), egui::FontId::proportional(14.0), SOFT);
                            p.text(egui::pos2(rect.left() + cols[3], mid), egui::Align2::LEFT_CENTER, human_size(r.size), egui::FontId::proportional(14.0), SOFT);
                            p.hline(rect.x_range(), rect.bottom(), egui::Stroke::new(1.0, HAIRLINE));
                            if resp.clicked() {
                                open = Some(r.path.clone());
                            }
                        }
                    });
                });
            }
        });

        if want_grid != self.home_grid {
            self.home_grid = want_grid;
            save_home_grid(want_grid);
        }

        self.filter = want_filter;
        if let Some(s) = select {
            self.folder_sel = s;
            self.folder_rows = s.and_then(|i| self.folders.get(i)).map(|d| folder_listing(&self.scans, d)).unwrap_or_default();
            // Files may have changed since the folder was added: list it again in the background.
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
            self.folder_rows.clear();
            self.recount();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if add_folder && let Some(dir) = rfd::FileDialog::new().pick_folder() {
            let dir = dir.to_string_lossy().to_string();
            if !self.folders.contains(&dir) {
                self.folders.push(dir.clone());
                save_folders(&self.folders);
            }
            self.recount();
            let i = self.folders.iter().position(|f| *f == dir);
            self.folder_sel = i;
            self.folder_rows = folder_listing(&self.scans, &dir);
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
            Tab::Equation(_) => Done(Err("the assistant does not work in the Equations tab".into())),
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
        // Never asked: the Equations tab has no assistant (`Kind::has_assistant`).
        Kind::Equation => ai::App::Docs,
    }
}

impl eframe::App for Suite {
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
                        let _ = a.run("insert.equation", serde_json::json!({"linear": "x=(-b+-sqrt(b^2-4ac))/2a"}));
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
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            }
            let img = ctx.input(|i| i.events.iter().find_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None }));
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
                // Every app in order (Word, Grid, Deck, Pdf, Graph, Equations), Home (index 6, past
                // the tabs), then the same again in another order so each pair of apps meets.
                let order = [0usize, 1, 2, 3, 4, 5, 6, 5, 2, 4, 0, 6, 3, 5, 1, 4, 6, 2, 5, 0];
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
            web::set_dirty(self.entries.iter().any(|e| e.tab.is_dirty()));
            let arrived = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(|e| e.into_inner()));
            for (name, bytes) in arrived {
                self.open_bytes(ctx, name, bytes);
            }
            let scanned = std::mem::take(&mut *self.folder_scans.lock().unwrap_or_else(|e| e.into_inner()));
            for FolderScan { key, rows, added } in scanned {
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
        if self.restore_home && self.pending.is_empty() {
            self.view = View::Home;
            self.restore_home = false;
        }
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
        if let Some((prompt, started)) = self.ai_selftest.clone() {
            self.drive_ai_selftest(ctx, prompt, started);
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
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
        let mut toggle_tile = false;
        let mut drag_now: Option<u64> = None;
        let mut drop_tab: Option<(u64, egui::Pos2)> = None;
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
                }
                for e in &self.entries {
                    let title = if e.tab.is_dirty() { format!("{} •", e.name) } else { e.name.clone() };
                    let (stripe, _) = ui.allocate_exact_size(egui::vec2(3.0, 14.0), egui::Sense::hover());
                    ui.painter().rect_filled(stripe, 1.5, kind_color(Some(e.kind)));
                    let shown = title;
                    let r = ui.selectable_label(self.view == View::Tab(e.id), shown).on_hover_text(e.path.as_deref().unwrap_or("Not saved yet"));
                    // Also draggable (onto the content to tile it) and right-clickable.
                    let r = r.interact(egui::Sense::click_and_drag());
                    let tileable = e.kind.tileable();
                    if r.clicked() {
                        select = Some(View::Tab(e.id));
                    }
                    if tileable && r.dragged() {
                        drag_now = Some(e.id);
                    }
                    if tileable && r.drag_stopped() {
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
                        if ui.button("Close tab").clicked() {
                            close = Some(e.id);
                            ui.close();
                        }
                    });
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
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⚙").on_hover_text("Assistant settings: provider, endpoint, API keys").clicked() {
                        open_settings = true;
                    }
                    // A page cannot rebuild itself; the updater is for the installed app.
                    #[cfg(not(target_arch = "wasm32"))]
                    if ui.button("⟳").on_hover_text("Updates: rebuild against the newest app commits").clicked() {
                        self.updater.open = !self.updater.open;
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
            }
            self.ai_settings = None;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.updater.ui(ui.ctx()) {
            updater::spawn_new_instance();
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if let Some(id) = toggle_chat
            && let Some(e) = self.entry_mut(id)
        {
            e.chat.open = !e.chat.open;
        }
        if let Some(id) = tile_in {
            self.tile_with(id, None);
        }
        if let Some(id) = tile_out {
            self.take_out_tile(id);
        }
        if let Some((id, p)) = drop_tab {
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
            let layer = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("tab-drag")));
            layer.text(p + egui::vec2(14.0, 14.0), egui::Align2::LEFT_TOP, format!("Tile {name}"), egui::FontId::proportional(13.0), ACCENT);
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
        match self.view {
            View::Home => self.home(ui),
            View::Tab(_) if self.tiled_ids().is_some() => {
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
    eframe::run_native("OfficeCraft", options, Box::new(move |_cc| Ok(Box::new(suite))))
}
