//! Round-trip checks: a document built by each engine, saved, put through LibreOffice and read
//! back, compared with the original as the engine's own inspect commands see it.
//!
//! Ignored by default (LibreOffice is slow and may be absent). Run it with
//!
//! ```text
//! CARGO_TARGET_DIR=… cargo test --offline --bin officecraft roundtrip -- --ignored --nocapture
//! ```
//!
//! Everything is written under [`std::env::temp_dir`]. Set `OC_ROUNDTRIP_DUMP=1` to print the raw
//! inspect JSON of each file.
//!
//! The harness is deliberately arm's-length: documents are built and read through the engines'
//! commands (`text.insert`, `cell.set`, `slide.new`, `document.inspect`, …), the way the assistant
//! drives the apps, so a command that is broken for the assistant is caught too.
//!
//! Losses inside an app's own import or export are not the harness's to fix: they are listed in
//! [`KNOWN_LOSSES`] (with the upstream issue they represent) so that only a *new* loss fails.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The two flows the report is about, named once so a known loss can point at one of them.
const RT: &str = "OfficeCraft -> LibreOffice -> OfficeCraft";
const LO_ODF: &str = "LibreOffice's own file opened in OfficeCraft";
const LO_OPENS: &str = "OfficeCraft's file opened in LibreOffice";

/// Losses traced to an app's own importer or exporter (or to a format the app has no reader
/// for), as (flow, feature, what it is). A failing check listed here is reported and does not
/// fail the test, so only a *new* loss turns the suite red; the note is what the report cites.
const KNOWN_LOSSES: &[(&str, &str, &str)] = &[
    (
        LO_ODF,
        "opens as .odp",
        "DeckCraft has no .odp reader: file.open of LibreOffice's own presentation answers \
         “not a presentation DeckCraft can read” (upstream deckcraft_pptx has no ODF importer, \
         unlike wordcraft's odt reader and gridcraft's read_ods)",
    ),
    (
        LO_ODF,
        "formulas",
        "GridCraft's .ods import keeps the cached value and drops the formula: B5 reads 1285.5 \
         where the .ods holds =SUM(B2:B3) (upstream gridcraft_xlsx::read_ods)",
    ),
    (
        LO_ODF,
        "number formats",
        "GridCraft's .ods import loses the cell's number format: a currency cell reads General, \
         a date cell keeps only yyyy-mm-dd (upstream gridcraft_xlsx::read_ods)",
    ),
    (LO_ODF, "shown text", "follows from the number formats above: $1,200.00 shows as 1200"),
    (LO_ODF, "date", "follows from the number formats above: 11/25/2026 shows as 2026-11-25"),
];

/// One comparable feature of a document, as both sides see it.
#[derive(Clone)]
struct Check {
    app: &'static str,
    flow: &'static str,
    feature: &'static str,
    ok: bool,
    detail: String,
}

impl Check {
    fn new(app: &'static str, flow: &'static str, feature: &'static str, ok: bool, detail: impl Into<String>) -> Check {
        Check { app, flow, feature, ok, detail: detail.into() }
    }
}

/// Where the run keeps its files: a fresh directory under the system temp dir.
struct Env {
    dir: PathBuf,
    checks: Vec<Check>,
}

impl Env {
    fn new() -> Env {
        let dir = std::env::temp_dir().join(format!("officecraft-roundtrip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Env { dir, checks: vec![] }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// LibreOffice's own conversion of `file` to `ext`, in a step of its own so two conversions
    /// never overwrite each other's output.
    fn convert(&self, step: &str, file: &Path, ext: &str) -> PathBuf {
        let out = self.dir.join(step);
        std::fs::create_dir_all(&out).unwrap();
        // Never the user's profile: a second LibreOffice holding it would make this a no-op.
        let profile = format!("file://{}", self.dir.join("lo-profile").display());
        let o = std::process::Command::new("soffice")
            .arg(format!("-env:UserInstallation={profile}"))
            .args(["--headless", "--norestore", "--convert-to", ext, "--outdir"])
            .arg(&out)
            .arg(file)
            .output()
            .unwrap();
        let name = format!("{}.{ext}", file.file_stem().unwrap_or_default().to_string_lossy());
        let dest = out.join(name);
        assert!(
            dest.is_file() && std::fs::metadata(&dest).map(|m| m.len() > 0).unwrap_or(false),
            "LibreOffice did not convert {} to .{ext}:\n{}\n{}",
            file.display(),
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        dest
    }

    /// LibreOffice opens the file and prints it: its page or slide count, when it has one.
    fn pdf_pages(&self, step: &str, file: &Path) -> usize {
        let pdf = self.convert(step, file, "pdf");
        let o = std::process::Command::new("pdfinfo").arg(&pdf).output().unwrap();
        let text = String::from_utf8_lossy(&o.stdout).to_string();
        text.lines()
            .find_map(|l| l.strip_prefix("Pages:"))
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or_else(|| panic!("pdfinfo gave no page count for {}:\n{}", pdf.display(), text))
    }

    fn check(&mut self, app: &'static str, flow: &'static str, feature: &'static str, ok: bool, detail: impl Into<String>) {
        self.checks.push(Check::new(app, flow, feature, ok, detail));
    }

    /// Compares one feature of two summaries; `what` is the line the report says failed.
    fn same(&mut self, app: &'static str, flow: &'static str, feature: &'static str, a: &str, b: &str, what: &str) {
        let ok = a == b;
        let detail = if ok { String::new() } else { format!("{what}: {a:?} became {b:?}") };
        self.check(app, flow, feature, ok, detail);
    }

    fn known_loss(c: &Check) -> Option<&'static str> {
        KNOWN_LOSSES.iter().find(|(flow, feature, _)| *flow == c.flow && *feature == c.feature).map(|(_, _, note)| *note)
    }

    fn report(&self) -> bool {
        let mut failed = vec![];
        for flow in [RT, LO_ODF, LO_OPENS] {
            println!("\n=== {flow} ===");
            for c in self.checks.iter().filter(|c| c.flow == flow) {
                println!("  {:<7} {:<22} {:<4} {}", c.app, c.feature, if c.ok { "ok" } else { "LOST" }, c.detail);
                if !c.ok {
                    failed.push(c.clone());
                }
            }
        }
        let unknown: Vec<&Check> = failed.iter().filter(|c| Env::known_loss(c).is_none()).collect();
        println!("\n{} checks, {} failed, {} of them not in KNOWN_LOSSES", self.checks.len(), failed.len(), unknown.len());
        for c in &failed {
            match Env::known_loss(c) {
                Some(note) => println!("  known upstream loss: {} / {} — {note}", c.app, c.feature),
                None => println!("  NEW LOSS: {} / {} — {}", c.app, c.feature, c.detail),
            }
        }
        unknown.is_empty()
    }
}

fn have_soffice() -> bool {
    std::process::Command::new("soffice").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

// ---------------------------------------------------------------------------------------------
// Docs
// ---------------------------------------------------------------------------------------------

fn word(s: &mut wordcraft_engine::Session, cmd: &str, p: Value) -> Value {
    match s.run(cmd, &p) {
        Ok(v) => v,
        Err(e) => panic!("{cmd} {p} failed: {e}"),
    }
}

/// A 64×48 RGB PNG, so the documents carry a real picture rather than a placeholder.
fn make_png(path: &Path) {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 64, 48);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        let mut px = Vec::with_capacity(64 * 48 * 3);
        for y in 0..48 {
            for x in 0..64 {
                px.extend_from_slice(&[(x * 4) as u8, (y * 5) as u8, 160]);
            }
        }
        w.write_image_data(&px).unwrap();
    }
    std::fs::write(path, out).unwrap();
}

/// One paragraph of `document.inspect`: its style, text, list, character marks, objects.
#[derive(Clone, Debug, Default, PartialEq)]
struct Para {
    style: String,
    text: String,
    list: Option<(u64, u64)>,
    bold: bool,
    italic: bool,
    objects: usize,
}

#[derive(Clone, Debug, Default)]
struct Doc {
    paras: Vec<Para>,
    tables: Vec<Vec<Vec<String>>>,
    /// Each paragraph's list as a reader sees it: its level and the list's own format there
    /// ("level 0 bullet" / "level 0 decimal %1."). The numbering *id* is not compared: saving and
    /// opening a document renumbers the lists, which changes nothing a reader can see.
    list_desc: Vec<Option<String>>,
    pages: u64,
}

impl Doc {
    /// The paragraphs' text with the in-line object character removed, since a picture is
    /// compared on its own.
    fn text(&self) -> Vec<String> {
        self.paras.iter().map(|p| p.text.replace(['\u{FFFC}', '\u{C}'], "").trim_end().to_string()).collect()
    }
    fn styles(&self) -> Vec<String> {
        self.paras.iter().map(|p| p.style.clone()).collect()
    }
    fn lists(&self) -> &[Option<String>] {
        &self.list_desc
    }
    fn bold_paras(&self) -> Vec<usize> {
        self.paras.iter().enumerate().filter(|(_, p)| p.bold).map(|(i, _)| i).collect()
    }
    fn italic_paras(&self) -> Vec<usize> {
        self.paras.iter().enumerate().filter(|(_, p)| p.italic).map(|(i, _)| i).collect()
    }
    fn pictures(&self) -> usize {
        self.paras.iter().map(|p| p.objects).sum()
    }
}

fn read_doc(s: &mut wordcraft_engine::Session) -> Doc {
    let v = word(&mut *s, "document.inspect", json!({"text": true}));
    let mut d = Doc { pages: v.get("pages").and_then(Value::as_u64).unwrap_or(0), ..Doc::default() };
    for b in v.get("blocks").and_then(Value::as_array).into_iter().flatten() {
        match b.get("type").and_then(Value::as_str) {
            Some("table") => {
                let cells: Vec<Vec<String>> = b
                    .get("cells")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|row| row.as_array().into_iter().flatten().map(|c| c.as_str().unwrap_or_default().to_string()).collect())
                    .collect();
                d.tables.push(cells);
            }
            _ => {
                let list = b.get("list").and_then(|l| {
                    Some((l.get("num")?.as_u64()?, l.get("level")?.as_u64()?))
                });
                let runs: Vec<&Value> = b.get("runs").and_then(Value::as_array).into_iter().flatten().collect();
                let props = |k: &str| runs.iter().any(|r| r.get("props").and_then(|p| p.get(k)).and_then(Value::as_bool).unwrap_or(false));
                d.paras.push(Para {
                    style: b.get("style").and_then(Value::as_str).unwrap_or_default().to_string(),
                    text: b.get("text").and_then(Value::as_str).unwrap_or_default().to_string(),
                    list,
                    bold: props("bold"),
                    italic: props("italic"),
                    objects: b.get("objects").and_then(Value::as_array).map_or(0, |o| o.len()),
                });
            }
        }
    }
    // What each list is, level by level, through the list's own definition.
    let mut definitions: std::collections::HashMap<u64, Value> = std::collections::HashMap::new();
    for p in &d.paras {
        let desc = p.list.map(|(num, level)| {
            let def = definitions.entry(num).or_insert_with(|| word(s, "list.get", json!({"num": num})));
            let lv = def.get("levels").and_then(|l| l.get(level as usize));
            let format = lv.and_then(|x| x.get("format")).and_then(Value::as_str).unwrap_or("?");
            let text = lv.and_then(|x| x.get("text")).and_then(Value::as_str).unwrap_or("");
            format!("level {level} {format} {text:?}")
        });
        d.list_desc.push(desc);
    }
    d
}

/// The Docs document the report names: a title, headings, bold and italic, a bulleted and a
/// numbered list, a 3×3 table, a picture and a page break.
///
/// Every paragraph is appended at the end of the document with the caret first collapsed there:
/// a command that leaves a selection open (formatting, a list) would make the next Enter delete
/// the selected text, which is the user's business but not this harness's.
fn build_doc(env: &Env, png: &Path) -> PathBuf {
    let mut s = wordcraft_engine::Session::new(wordcraft_doc::Document::new());
    let para = |s: &mut wordcraft_engine::Session, text: &str, style: &str| {
        word(s, "caret.docEnd", json!({}));
        word(s, "text.newParagraph", json!({}));
        word(s, "text.insert", json!({"text": text}));
        if style != "Normal" {
            word(s, "para.style", json!({"style": style}));
        }
    };

    word(&mut s, "text.insert", json!({"text": "Round Trip Report"}));
    word(&mut s, "para.style", json!({"style": "Title"}));
    para(&mut s, "Introduction", "Heading1");
    para(&mut s, "This has bold words and italic words in it.", "Normal");
    para(&mut s, "Bulleted list", "Heading2");

    // Bold and italic one word each, by selecting it and formatting it. The word "italic" has
    // to be still there when it is formatted: a stale selection must not swallow it.
    let text = "This has bold words and italic words in it.";
    let (b, i) = (text.find("bold").unwrap(), text.find("italic").unwrap());
    word(&mut s, "select.range", json!({"anchor": {"story": "body", "path": [2], "off": b}, "focus": {"story": "body", "path": [2], "off": b + 4}}));
    word(&mut s, "format.bold", json!({}));
    word(&mut s, "select.range", json!({"anchor": {"story": "body", "path": [2], "off": i}, "focus": {"story": "body", "path": [2], "off": i + 6}}));
    word(&mut s, "format.italic", json!({}));

    for text in ["First bullet", "Second bullet", "Third bullet"] {
        para(&mut s, text, "Normal");
    }
    // Paragraphs 4..=6 are the bullets.
    word(&mut s, "select.range", json!({"anchor": {"story": "body", "path": [4], "off": 0}, "focus": {"story": "body", "path": [6], "off": "Third bullet".len()}}));
    word(&mut s, "para.bullets", json!({}));

    para(&mut s, "Numbered list", "Heading2");
    for text in ["Step one", "Step two", "Step three"] {
        para(&mut s, text, "Normal");
    }
    // Paragraphs 8..=10 are the numbered items.
    word(&mut s, "select.range", json!({"anchor": {"story": "body", "path": [8], "off": 0}, "focus": {"story": "body", "path": [10], "off": "Step three".len()}}));
    word(&mut s, "para.numbering", json!({}));

    // A heading, then the table at the end of the document; its cells are filled one by one.
    para(&mut s, "Table", "Heading1");
    word(&mut s, "caret.docEnd", json!({}));
    let inserted = word(&mut s, "insert.table", json!({"rows": 3, "cols": 3}));
    let table = inserted.get("table").and_then(Value::as_array).and_then(|p| p.first()).and_then(Value::as_u64).expect("a table path");
    for (r, row) in [["A1", "B1", "C1"], ["A2", "B2", "C2"], ["A3", "B3", "C3"]].iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            let at = json!({"story": "body", "path": [table, r as u64, c as u64, 0], "off": 0});
            word(&mut s, "select.range", json!({"anchor": at.clone(), "focus": at}));
            word(&mut s, "text.insert", json!({"text": cell}));
        }
    }

    // The picture, in a paragraph of its own after the table.
    para(&mut s, "Picture: ", "Normal");
    word(&mut s, "insert.picture", json!({"path": png.to_string_lossy(), "width": 64.0}));
    word(&mut s, "text.insert", json!({"text": " right there."}));

    // A paragraph, a page break, and text on the second page.
    para(&mut s, "Last paragraph on the first page.", "Normal");
    word(&mut s, "insert.pageBreak", json!({}));
    para(&mut s, "Second page text.", "Normal");

    let path = env.path("report.docx");
    word(&mut s, "file.saveAs", json!({"path": path.to_string_lossy()}));
    path
}

fn open_doc(path: &Path) -> wordcraft_engine::Session {
    let mut s = wordcraft_engine::Session::new(wordcraft_doc::Document::new());
    word(&mut s, "file.open", json!({"path": path.to_string_lossy()}));
    s
}

fn check_docs(env: &mut Env, original: &Doc, got: &Doc, flow: &'static str) {
    let app = "Docs";
    env.same(app, flow, "paragraph text", &format!("{:?}", original.text()), &format!("{:?}", got.text()), "the paragraphs");
    env.same(app, flow, "paragraph styles", &format!("{:?}", original.styles()), &format!("{:?}", got.styles()), "the styles");
    env.same(app, flow, "list levels", &format!("{:?}", original.lists()), &format!("{:?}", got.lists()), "the lists");
    env.same(app, flow, "bold run", &format!("{:?}", original.bold_paras()), &format!("{:?}", got.bold_paras()), "the bold text");
    env.same(app, flow, "italic run", &format!("{:?}", original.italic_paras()), &format!("{:?}", got.italic_paras()), "the italic text");
    env.same(app, flow, "table cells", &format!("{:?}", original.tables), &format!("{:?}", got.tables), "the table");
    env.same(app, flow, "picture", &original.pictures().to_string(), &got.pictures().to_string(), "the picture");
    // A page break is compared by the pages it makes: ODT and OOXML store it differently.
    env.same(app, flow, "page break", &format!("{} pages", original.pages), &format!("{} pages", got.pages), "the pagination");
}

// ---------------------------------------------------------------------------------------------
// Sheets
// ---------------------------------------------------------------------------------------------

fn grid(s: &mut gridcraft_engine::Session, cmd: &str, p: Value) -> Value {
    let shown = p.clone();
    match s.execute(cmd, p) {
        Ok(v) => v,
        Err(e) => panic!("{cmd} {shown} failed: {e}"),
    }
}

#[derive(Clone, Debug, Default)]
struct Book {
    sheets: Vec<String>,
    values: Vec<Vec<Vec<Value>>>,
    formulas: Vec<Vec<Vec<Value>>>,
    /// (cell, number format) for the formatted cells the report names.
    formats: Vec<(String, String)>,
    /// (cell, the text the app shows) for the same cells: what a reader actually sees, which a
    /// format code spelled differently still gets right.
    displays: Vec<(String, String)>,
    dates: Vec<(String, String)>,
}

/// The number format in a `cell.get`'s style, wherever the style struct keeps it.
fn num_fmt(v: &Value) -> String {
    fn walk(v: &Value) -> Option<String> {
        match v {
            Value::Object(m) => {
                for k in ["numFmt", "num_fmt", "numberFormat", "number_format"] {
                    if let Some(Value::String(s)) = m.get(k) {
                        return Some(s.clone());
                    }
                }
                m.values().find_map(walk)
            }
            Value::Array(a) => a.iter().find_map(walk),
            _ => None,
        }
    }
    walk(v.get("style").unwrap_or(&Value::Null)).unwrap_or_default()
}

fn read_book(s: &mut gridcraft_engine::Session) -> Book {
    let inspect = grid(s, "document.inspect", json!({}));
    let sheets: Vec<String> = inspect
        .get("sheets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| s.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();
    let mut b = Book { sheets: sheets.clone(), ..Book::default() };
    for name in &sheets {
        let read = grid(s, "sheet.read", json!({"sheet": name, "formulas": false}));
        let formulas = grid(s, "sheet.read", json!({"sheet": name, "formulas": true}));
        let rows = |v: &Value| -> Vec<Vec<Value>> {
            v.get("values").and_then(Value::as_array).into_iter().flatten().map(|r| r.as_array().cloned().unwrap_or_default()).collect()
        };
        b.values.push(rows(&read));
        b.formulas.push(rows(&formulas));
    }
    // The named cells: a currency amount, a date, a plain number and the SUM.
    for cell in ["B2", "B3", "B4", "B5"] {
        let c = grid(s, "cell.get", json!({"cell": cell, "sheet": &sheets[0]}));
        b.formats.push((cell.to_string(), num_fmt(&c)));
        b.displays.push((cell.to_string(), c.get("text").and_then(Value::as_str).unwrap_or_default().to_string()));
        if cell == "B4" {
            b.dates.push((cell.to_string(), c.get("text").and_then(Value::as_str).unwrap_or_default().to_string()));
        }
    }
    b
}

/// A budget sheet: numbers, text, a date, a currency format, a SUM, and a second sheet.
fn build_book(env: &Env) -> PathBuf {
    let mut s = gridcraft_engine::Session::new();
    s.new_workbook();
    grid(&mut s, "sheet.rename", json!({"name": "Budget"}));
    grid(
        &mut s,
        "range.setValues",
        json!({"range": "A1", "values": [
            ["Item", "Amount", "Note"],
            ["Rent", 1200, "monthly"],
            ["Power", 85.5, "monthly"],
            ["Date", "2026-11-25", ""],
            ["Total", "=SUM(B2:B3)", ""]
        ]}),
    );
    grid(&mut s, "home.numberFormat", json!({"range": "B2", "format": "Currency"}));
    grid(&mut s, "home.numberFormat", json!({"range": "B3:B3", "code": "0.00"}));
    grid(&mut s, "home.numberFormat", json!({"range": "B4", "format": "Short Date"}));
    grid(&mut s, "home.insertSheet", json!({"name": "Notes"}));
    grid(&mut s, "sheet.activate", json!({"sheet": "Notes"}));
    grid(&mut s, "range.setValues", json!({"range": "A1", "values": [["Second sheet", 42]]}));
    let path = env.path("budget.xlsx");
    grid(&mut s, "file.saveAs", json!({"path": path.to_string_lossy()}));
    path
}

fn open_book(path: &Path) -> gridcraft_engine::Session {
    let mut s = gridcraft_engine::Session::new();
    grid(&mut s, "file.open", json!({"path": path.to_string_lossy()}));
    s
}

fn check_book(env: &mut Env, original: &Book, got: &Book, flow: &'static str) {
    let app = "Sheets";
    let f = |x: &Vec<Vec<Vec<Value>>>| format!("{x:?}");
    env.same(app, flow, "sheet names", &format!("{:?}", original.sheets), &format!("{:?}", got.sheets), "the sheets");
    env.same(app, flow, "cell values", &f(&original.values), &f(&got.values), "the values");
    env.same(app, flow, "formulas", &f(&original.formulas), &f(&got.formulas), "the formulas");
    // An .odt→.docx trip through LibreOffice writes `"$"#,##0.00` as `\$#,##0.00`: the same format
    // spelled with an escape, so the codes are compared without quotes and escapes, and the
    // texts the app shows are compared as they are.
    let quoted = |f: &Vec<(String, String)>| format!("{:?}", f.iter().map(|(c, s)| (c.clone(), s.replace(['"', '\\'], ""))).collect::<Vec<_>>());
    env.same(app, flow, "number formats", &quoted(&original.formats), &quoted(&got.formats), "the number formats");
    env.same(app, flow, "shown text", &format!("{:?}", original.displays), &format!("{:?}", got.displays), "the cells as shown");
    env.same(app, flow, "date", &format!("{:?}", original.dates), &format!("{:?}", got.dates), "the date as shown");
    env.same(app, flow, "second sheet", &format!("{:?}", original.values.get(1)), &format!("{:?}", got.values.get(1)), "the second sheet");
    env.check(app, flow, "opens", !got.sheets.is_empty(), format!("{} sheet(s)", got.sheets.len()));
}

// ---------------------------------------------------------------------------------------------
// Slides
// ---------------------------------------------------------------------------------------------

fn deck(s: &mut deckcraft_engine::Session, cmd: &str, p: Value) -> Value {
    match s.execute(cmd, &p) {
        Ok(v) => v,
        Err(e) => panic!("{cmd} {p} failed: {e}"),
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Slide {
    title: String,
    notes: String,
    texts: Vec<String>,
    table: Vec<Vec<String>>,
    pictures: usize,
}

#[derive(Clone, Debug, Default)]
struct Deck {
    slides: Vec<Slide>,
}

fn read_deck(s: &mut deckcraft_engine::Session) -> Deck {
    let inspect = deck(s, "document.inspect", json!({}));
    let count = inspect.get("slides").and_then(Value::as_array).map_or(0, Vec::len);
    let mut d = Deck::default();
    for i in 0..count {
        let sh = inspect.get("slides").and_then(|v| v.get(i)).cloned().unwrap_or(Value::Null);
        let one = deck(s, "slide.inspect", json!({"index": i}));
        let mut slide = Slide {
            title: sh.get("title").and_then(Value::as_str).unwrap_or_default().to_string(),
            notes: sh.get("notes").and_then(Value::as_str).unwrap_or_default().to_string(),
            ..Slide::default()
        };
        for shape in one.get("shapes").and_then(Value::as_array).into_iter().flatten() {
            if let Some(t) = shape.get("text").and_then(Value::as_str) {
                slide.texts.push(t.to_string());
            }
            if shape.get("kind").and_then(Value::as_str) == Some("picture") || shape.get("picture").is_some() {
                slide.pictures += 1;
            }
            if let Some(t) = shape.get("table") {
                let cells = t
                    .get("cells")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|r| r.as_array().into_iter().flatten().map(|c| c.as_str().unwrap_or_default().to_string()).collect())
                    .collect();
                slide.table = cells;
            }
        }
        d.slides.push(slide);
    }
    d
}

/// The id of a slide's title (or body) placeholder.
fn placeholder_id(s: &mut deckcraft_engine::Session, index: usize, title: bool) -> Option<u64> {
    let insp = deck(s, "slide.inspect", json!({"index": index}));
    insp.get("shapes").and_then(Value::as_array).into_iter().flatten().find_map(|sh| {
        let ph = sh.get("placeholder").and_then(Value::as_str).unwrap_or_default().to_ascii_lowercase();
        let is_title = ph == "title" || ph == "ctrtitle";
        (is_title == title).then(|| sh.get("id").and_then(Value::as_u64)).flatten()
    })
}

/// Four slides: a title slide, a bullets slide, a table slide and a picture slide, with notes.
fn build_deck(env: &Env, png: &Path) -> PathBuf {
    let mut s = deckcraft_engine::Session::new();
    deck(&mut s, "file.new", json!({"blank": true}));
    deck(&mut s, "slide.new", json!({"layout": "title"}));
    deck(&mut s, "slide.notes", json!({"index": 0, "text": "Speaker notes on the title slide."}));
    let title = placeholder_id(&mut s, 0, true).expect("the title slide has a title placeholder");
    deck(&mut s, "text.set", json!({"id": title, "text": "Round Trip"}));
    if let Some(sub) = placeholder_id(&mut s, 0, false) {
        deck(&mut s, "text.set", json!({"id": sub, "text": "A deck built by OfficeCraft"}));
    }

    deck(&mut s, "slide.new", json!({"layout": "titleAndContent", "title": "Bullets", "body": "First point\nSecond point\nThird point"}));

    deck(&mut s, "slide.new", json!({"layout": "titleOnly", "title": "Table"}));
    deck(
        &mut s,
        "insert.table",
        json!({"rows": 3, "cols": 3, "rect": [80.0, 160.0, 560.0, 150.0], "data": [["A1", "B1", "C1"], ["A2", "B2", "C2"], ["A3", "B3", "C3"]]}),
    );

    deck(&mut s, "slide.new", json!({"layout": "titleOnly", "title": "Picture"}));
    deck(&mut s, "insert.picture", json!({"path": png.to_string_lossy(), "rect": [200.0, 150.0, 300.0, 200.0]}));
    deck(&mut s, "slide.notes", json!({"index": 3, "text": "Notes on the picture slide."}));

    let path = env.path("deck.pptx");
    deck(&mut s, "file.saveAs", json!({"path": path.to_string_lossy()}));
    path
}

fn open_deck(path: &Path) -> Result<deckcraft_engine::Session, String> {
    let mut s = deckcraft_engine::Session::new();
    s.execute("file.open", &json!({"path": path.to_string_lossy()})).map_err(|e| e.to_string())?;
    Ok(s)
}

fn check_deck(env: &mut Env, original: &Deck, got: &Deck, flow: &'static str) {
    let app = "Slides";
    env.same(app, flow, "slide count", &original.slides.len().to_string(), &got.slides.len().to_string(), "the slides");
    let titles: Vec<&str> = got.slides.iter().map(|s| s.title.as_str()).collect();
    let want: Vec<&str> = original.slides.iter().map(|s| s.title.as_str()).collect();
    env.same(app, flow, "slide titles", &format!("{want:?}"), &format!("{titles:?}"), "the titles");
    let notes: Vec<&str> = got.slides.iter().map(|s| s.notes.as_str()).collect();
    let want_notes: Vec<&str> = original.slides.iter().map(|s| s.notes.as_str()).collect();
    env.same(app, flow, "speaker notes", &format!("{want_notes:?}"), &format!("{notes:?}"), "the notes");
    for (i, slide) in original.slides.iter().enumerate() {
        let name: &'static str = match i {
            1 => "bullets text",
            2 => "slide table",
            3 => "slide picture",
            _ => continue,
        };
        let g = got.slides.get(i);
        let (a, b) = match i {
            1 => (slide.texts.clone(), g.map(|s| s.texts.clone()).unwrap_or_default()),
            2 => (vec![format!("{:?}", slide.table)], g.map(|s| vec![format!("{:?}", s.table)]).unwrap_or_default()),
            _ => (vec![slide.pictures.to_string()], g.map(|s| s.pictures.to_string()).map(|x| vec![x]).unwrap_or_default()),
        };
        env.same(app, flow, name, &format!("{a:?}"), &format!("{b:?}"), "the slide's contents");
    }
}

// ---------------------------------------------------------------------------------------------
// The harness
// ---------------------------------------------------------------------------------------------

#[test]
#[ignore = "needs LibreOffice and takes a minute"]
fn roundtrip_through_libreoffice() {
    if !have_soffice() {
        println!("SKIP: soffice is not on PATH, so there is nothing to round-trip through.");
        return;
    }
    let dump = std::env::var_os("OC_ROUNDTRIP_DUMP").is_some();
    let mut env = Env::new();
    let png = env.path("picture.png");
    make_png(&png);

    // -- Docs ---------------------------------------------------------------------------------
    let docx = build_doc(&env, &png);
    let original = read_doc(&mut open_doc(&docx));
    if dump {
        println!("DOC source: {:?}", original);
    }
    let docx_pages = env.pdf_pages("docs-pdf", &docx);
    env.check("Docs", LO_OPENS, "opens in LibreOffice", docx_pages > 0, format!("{docx_pages} page(s)"));
    env.same("Docs", LO_OPENS, "pages", &original.pages.to_string(), &docx_pages.to_string(), "WordCraft's page count and LibreOffice's");

    let odt = env.convert("docs-to-odt", &docx, "odt");
    let from_odt = read_doc(&mut open_doc(&odt));
    check_docs(&mut env, &original, &from_odt, LO_ODF);

    let back = env.convert("docs-to-docx", &odt, "docx");
    let round = read_doc(&mut open_doc(&back));
    if dump {
        println!("DOC round trip: {:?}", round);
    }
    check_docs(&mut env, &original, &round, RT);

    // -- Sheets -------------------------------------------------------------------------------
    let xlsx = build_book(&env);
    let original = read_book(&mut open_book(&xlsx));
    if dump {
        println!("SHEET source: {original:?}");
    }
    let xlsx_pages = env.pdf_pages("sheets-pdf", &xlsx);
    env.check("Sheets", LO_OPENS, "opens in LibreOffice", xlsx_pages > 0, format!("{xlsx_pages} page(s)"));

    let ods = env.convert("sheets-to-ods", &xlsx, "ods");
    let from_ods = read_book(&mut open_book(&ods));
    check_book(&mut env, &original, &from_ods, LO_ODF);

    let back = env.convert("sheets-to-xlsx", &ods, "xlsx");
    let round = read_book(&mut open_book(&back));
    if dump {
        println!("SHEET round trip: {round:?}");
    }
    check_book(&mut env, &original, &round, RT);

    // -- Slides -------------------------------------------------------------------------------
    let pptx = build_deck(&env, &png);
    let original = read_deck(&mut open_deck(&pptx).expect("DeckCraft opens the .pptx it just wrote"));
    if dump {
        println!("SLIDE source: {original:?}");
    }
    let pptx_pages = env.pdf_pages("slides-pdf", &pptx);
    env.check("Slides", LO_OPENS, "opens in LibreOffice", pptx_pages > 0, format!("{pptx_pages} slide(s)"));

    let odp = env.convert("slides-to-odp", &pptx, "odp");
    env.same("Slides", LO_ODF, "opens as .odp", "opened", if open_deck(&odp).is_ok() { "opened" } else { "not opened" }, "DeckCraft opening an .odp");
    match open_deck(&odp) {
        Ok(mut s) => {
            let from_odp = read_deck(&mut s);
            check_deck(&mut env, &original, &from_odp, LO_ODF);
        }
        Err(_) => println!("NOTE: DeckCraft cannot open an .odp at all, so the .odp comparison is skipped."),
    }

    let back = env.convert("slides-to-pptx", &odp, "pptx");
    let round = read_deck(&mut open_deck(&back).expect("DeckCraft opens its own .pptx back"));
    if dump {
        println!("SLIDE round trip: {round:?}");
    }
    check_deck(&mut env, &original, &round, RT);

    let ok = env.report();
    assert!(ok, "the round trip lost something that is not a known upstream loss (see the report above)");
}

/// The extensions Home must recognise: LibreOffice's own formats among them, since that is what
/// the round trips above produce.
#[test]
fn home_recognises_libreoffice_formats() {
    use crate::Kind;
    for (name, kind) in [("report.odt", Kind::Word), ("budget.ods", Kind::Grid), ("report.docx", Kind::Word), ("budget.xlsx", Kind::Grid)] {
        assert!(crate::kind_for(name) == Some(kind), "Home does not open {name}");
    }
    assert!(crate::OPEN_EXTS.contains(&"ods"), "the file picker does not offer .ods");
    assert!(crate::OPEN_EXTS.contains(&"odt"), "the file picker does not offer .odt");
}
