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
                d.add_filter("WordCraft", &["docx", "docm", "dotx", "odt", "rtf", "txt", "md", "html", "htm", "json"])
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
        "vshl" | "vishual" => Some(Kind::Graph),
        _ => None,
    }
}

const OPEN_EXTS: &[&str] = &[
    "docx", "docm", "dotx", "odt", "rtf", "txt", "md", "html", "htm", "xlsx", "xlsm", "csv", "tsv", "pptx", "potx", "ppsx", "deckcraft", "pdf", "vshl", "vishual",
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
            Tab::Graph(a) => a.changed(),
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

fn kind_color(kind: Option<Kind>) -> egui::Color32 {
    match kind {
        Some(Kind::Word) => egui::Color32::from_rgb(47, 99, 198),
        Some(Kind::Grid) => egui::Color32::from_rgb(31, 143, 92),
        Some(Kind::Deck) => egui::Color32::from_rgb(212, 98, 46),
        Some(Kind::Pdf) => egui::Color32::from_rgb(194, 59, 69),
        Some(Kind::Graph) => egui::Color32::from_rgb(183, 121, 31),
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

fn home_view_file() -> Option<std::path::PathBuf> {
    recent_file().map(|p| p.with_file_name("homeview.txt"))
}

fn load_home_grid() -> bool {
    home_view_file().and_then(|p| std::fs::read_to_string(p).ok()).map(|s| s.trim() != "list").unwrap_or(true)
}

fn save_home_grid(grid: bool) {
    if let Some(p) = home_view_file() {
        let _ = std::fs::write(p, if grid { "grid" } else { "list" });
    }
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
        None => {}
    }
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
    /// Home shows thumbnails (true) or a table (false). Remembered in `homeview.txt`.
    home_grid: bool,
    /// `OFFICECRAFT_SHOT=<file.ppm>`: draw Home for a few frames, save the window as a PPM, exit.
    shot: Option<(String, u32)>,
    shot_sent: bool,
    home_styled: bool,
    folder_counts: Vec<usize>,
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
        let mut suite = Suite { view: View::Home, entries: Vec::new(), next_id: 1, recent, pending: Vec::new(), notice: None, confirm_close: None, ai_cfg: ai::Config::load(), ctx_owner: None, selftest: None, ai_selftest: None, persist: true, last_session: Vec::new(), restore_home: false, filter: Filter::All, search: String::new(), folders, folder_sel, folder_rows, home_grid: load_home_grid(), shot: None, shot_sent: false, home_styled: false, folder_counts: Vec::new() };
        suite.recount();
        suite
    }

    fn recount(&mut self) {
        self.folder_counts = self.folders.iter().map(|f| list_folder(f).len()).collect();
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
                    ui.label(b("OfficeCraft", 17.0));
                });
                ui.add_space(12.0);
                if side_row(ui, self.folder_sel.is_none(), false, "Recent", Some(self.recent.len()), false).0.clicked() {
                    select = Some(None);
                }
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
            let starts = [(Kind::Word, "Document", "Docs"), (Kind::Grid, "Spreadsheet", "Sheets"), (Kind::Deck, "Presentation", "Slides"), (Kind::Pdf, "PDF tools", "PDF"), (Kind::Graph, "Graph", "Graphs")];
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.columns(5, |cols| {
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
                ui.label(egui::RichText::new(if self.folder_sel.is_some() { "No supported files in this folder." } else { "Files you open appear here. Drop a file on the window, or press Open file." }).color(MUTED));
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
            self.folder_rows = s.and_then(|i| self.folders.get(i)).map(|d| list_folder(d)).unwrap_or_default();
        }
        if let Some(i) = remove_folder {
            self.folders.remove(i);
            save_folders(&self.folders);
            self.folder_sel = None;
            self.folder_rows.clear();
            self.recount();
        }
        if add_folder && let Some(dir) = rfd::FileDialog::new().pick_folder() {
            let dir = dir.to_string_lossy().to_string();
            if !self.folders.contains(&dir) {
                self.folders.push(dir.clone());
                save_folders(&self.folders);
            }
            self.recount();
            let i = self.folders.iter().position(|f| *f == dir);
            self.folder_sel = i;
            self.folder_rows = list_folder(&dir);
        }
        if pick_files && let Some(p) = rfd::FileDialog::new()
            .add_filter("All Office files", OPEN_EXTS)
            .add_filter("WordCraft", &["docx", "docm", "dotx", "odt", "rtf", "txt", "md", "html", "htm"])
            .add_filter("GridCraft", &["xlsx", "xlsm", "csv", "tsv"])
            .add_filter("DeckCraft", &["deckcraft", "pptx", "potx", "ppsx"])
            .add_filter("PDFCraft", &["pdf"])
            .add_filter("viSHual", &["vshl", "vishual"])
            .pick_files() {
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
        let mut shot_tab = None;
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
                        g.shot_options(&spec);
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
            if *n >= 20 && !waiting && !self.shot_sent {
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
        if self.view == View::Home {
            if !self.home_styled {
                BOLD_READY.store(false, std::sync::atomic::Ordering::Relaxed);
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
            let accent = kind_color(Some(e.kind));
            let models = ai::models(&self.ai_cfg, ui.ctx());
            let mut action = None;
            egui::Panel::right("officecraft_ai").default_size(400.0).min_size(300.0).resizable(true).show(ui, |ui| {
                action = e.chat.ui(ui, app, &self.ai_cfg, accent, &e.name, &models);
            });
            match action {
                Some(ai::Action::Send) => e.chat.send(app, &self.ai_cfg, ui.ctx()),
                Some(ai::Action::SetModel(m)) => {
                    ai::save_model(&m);
                    self.ai_cfg.model = m;
                }
                None => {}
            }
        }
        match self.view {
            View::Home => self.home(ui),
            View::Tab(id) => {
                let owner = self.ctx_owner;
                let cfg = self.ai_cfg.clone();
                if let Some(e) = self.entry_mut(id) {
                    // A tab clicked in this very frame has not had its fonts and visuals
                    // installed yet (`logic` does that at the start of the next frame): drawing it
                    // now would use another app's font families, which panics.
                    if owner == Some(e.kind) {
                        e.tab.ui(ui, frame);
                        // viSHual's own chat box asks the suite's assistant: one AI key for all.
                        if let Tab::Graph(g) = &mut e.tab {
                            for prompt in g.take_prompts() {
                                e.chat.open = true;
                                e.chat.ask_text(prompt, ai_app(e.kind), &cfg, ui.ctx());
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
    if let Ok(p) = std::env::var("OFFICECRAFT_SHOT") {
        suite.shot = Some((p, 0));
    }
    let selftest = suite.selftest.is_some() || suite.ai_selftest.is_some() || suite.shot.is_some();
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
