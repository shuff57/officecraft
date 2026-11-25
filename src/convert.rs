//! Send to…: one app's document made into another's.
//!
//! The conversions read a document through its app's own inspection commands and write the
//! result through the other app's commands, the way the assistant does, so nothing here knows a
//! file format. What can be worked out from the JSON alone lives here and is tested here; the
//! shell (`Suite::send_to`) does the reading, writing and opening of tabs.
//!
//! - Docs → Slides: the document's outline. The top heading level present becomes the slides,
//!   what follows each heading its bullets (`build_slides` arguments).
//! - Slides → Docs: each slide's title a heading, its text bullets, its notes body text
//!   (`insert_blocks` arguments).
//! - Sheets → Docs: the used range as a table, through the clipboard HTML WordCraft already
//!   pastes (shell only). Sheets → Slides: the used range's shown values as a slide table.
//! - Docs, Sheets or Slides → PDF: the app's own PDF export, opened in a PDF tab (shell only).

use serde_json::{Value, json};

/// Bullets on one slide before the rest go on a "(continued)" slide.
const MAX_BULLETS: usize = 6;
/// A bullet longer than this keeps its first sentence, or is cut with an ellipsis.
const MAX_BULLET_CHARS: usize = 140;
/// Table rows a Docs table brings to a slide as bullets.
const MAX_TABLE_ROWS: usize = 6;

/// A paragraph style as a level: `Some(0)` for the title, `Some(n)` for Heading n, `None` for
/// anything else. Imported files name styles by id ("Heading1"), new ones by name ("Heading 1").
fn heading_level(style: &str) -> Option<u8> {
    let s: String = style.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    if s == "title" {
        return Some(0);
    }
    s.strip_prefix("heading").and_then(|n| n.parse::<u8>().ok()).filter(|n| (1..=9).contains(n))
}

/// A paragraph as one bullet: its first sentence when it is long, cut at a word if still too long.
fn bullet(text: &str) -> String {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= MAX_BULLET_CHARS {
        return t;
    }
    // The first sentence, when there is one short enough.
    if let Some(end) = t.char_indices().find(|&(i, c)| matches!(c, '.' | '!' | '?') && t[i + c.len_utf8()..].starts_with(' ')).map(|(i, c)| i + c.len_utf8())
        && t[..end].chars().count() <= MAX_BULLET_CHARS
        && end > 0
    {
        return t[..end].to_string();
    }
    let mut out = String::new();
    for w in t.split(' ') {
        if out.chars().count() + w.chars().count() + 1 > MAX_BULLET_CHARS - 1 {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out.push('…');
    out
}

/// A file name without its folder and extension: the fallback title.
pub fn stem(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match base.rsplit_once('.') {
        Some((s, ext)) if !s.is_empty() && !ext.contains(' ') && ext.len() <= 5 => s.to_string(),
        _ => base.to_string(),
    }
}

/// One block of a WordCraft `document.inspect`, as the outline sees it.
enum Item {
    Heading(u8, String),
    Text(String),
    Rows(Vec<Vec<String>>),
}

fn items(inspect: &Value) -> Vec<Item> {
    let mut out = Vec::new();
    for b in inspect.get("blocks").and_then(Value::as_array).into_iter().flatten() {
        if b.get("type").and_then(Value::as_str) == Some("table") {
            let rows: Vec<Vec<String>> = b
                .get("cells")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .map(|r| r.as_array().map(|c| c.iter().map(|x| x.as_str().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ")).collect()).unwrap_or_default())
                        .filter(|r: &Vec<String>| r.iter().any(|c| !c.is_empty()))
                        .collect()
                })
                .unwrap_or_default();
            if !rows.is_empty() {
                out.push(Item::Rows(rows));
            }
            continue;
        }
        let text = b.get("text").and_then(Value::as_str).unwrap_or("").trim();
        if text.is_empty() {
            continue;
        }
        let style = b.get("style").and_then(Value::as_str).unwrap_or("Normal");
        // A list item never makes a slide, whatever its style says.
        match heading_level(style) {
            Some(l) if b.get("list").is_none() => out.push(Item::Heading(l, text.to_string())),
            _ => out.push(Item::Text(text.to_string())),
        }
    }
    out
}

/// A slide being filled in.
struct Slide {
    title: String,
    bullets: Vec<String>,
}

/// Docs → Slides: a WordCraft `document.inspect` (with text) as `build_slides` arguments
/// (`{"title", "slides": [{"title", "bullets", "layout"}]}`). `name` titles the deck when the
/// document has no Title paragraph. An error when there is no text to make slides of.
pub fn doc_to_slides(inspect: &Value, name: &str) -> Result<Value, String> {
    let mut items = items(inspect);
    if items.is_empty() {
        return Err("the document has no text to make slides of".into());
    }
    let title = items.iter().find_map(|i| match i {
        Item::Heading(0, t) => Some(t.clone()),
        _ => None,
    });
    // No Title paragraph: a short first line that is not a sentence ("Trip plan") is the title
    // typed in body text; it is used up as the title.
    let title = match title {
        Some(t) => t,
        None => match items.first() {
            Some(Item::Text(t)) if t.chars().count() <= 60 && !t.ends_with(['.', '!', '?', ':', ';', ',']) && items.len() > 1 => {
                let t = t.clone();
                items[0] = Item::Heading(0, t.clone());
                t
            }
            _ => stem(name),
        },
    };
    let title = title.trim().to_string();
    // The slides are the top heading level the document uses; the level under it is bullets.
    let top = items.iter().filter_map(|i| match i {
        Item::Heading(l, _) if *l > 0 => Some(*l),
        _ => None,
    });
    let top = top.min();
    let mut slides: Vec<Slide> = Vec::new();
    // What comes before the first heading (or the whole text, without headings) goes on slides
    // named after the deck.
    let mut current: Option<Slide> = None;
    let mut title_used = false;
    for it in &items {
        match it {
            Item::Heading(0, t) if !title_used && *t == title => {
                title_used = true;
            }
            Item::Heading(l, t) if Some(*l) == top => {
                if let Some(s) = current.take() {
                    slides.push(s);
                }
                current = Some(Slide { title: t.clone(), bullets: Vec::new() });
            }
            Item::Heading(_, t) | Item::Text(t) => {
                current.get_or_insert_with(|| Slide { title: title.clone(), bullets: Vec::new() }).bullets.push(bullet(t));
            }
            Item::Rows(rows) => {
                let s = current.get_or_insert_with(|| Slide { title: title.clone(), bullets: Vec::new() });
                for r in rows.iter().take(MAX_TABLE_ROWS) {
                    s.bullets.push(bullet(&r.iter().filter(|c| !c.is_empty()).cloned().collect::<Vec<_>>().join(" · ")));
                }
            }
        }
    }
    if let Some(s) = current.take() {
        slides.push(s);
    }
    let mut out = vec![json!({"title": title, "layout": "title"})];
    for s in slides {
        if s.bullets.is_empty() {
            out.push(json!({"title": s.title, "layout": "sectionHeader"}));
            continue;
        }
        // A long section runs on over as many slides as it needs.
        for (n, chunk) in s.bullets.chunks(MAX_BULLETS).enumerate() {
            let t = if n == 0 { s.title.clone() } else { format!("{} (continued)", s.title) };
            out.push(json!({"title": t, "bullets": chunk, "layout": "titleAndContent"}));
        }
    }
    Ok(json!({"title": title, "slides": out}))
}

/// Slides → Docs: each DeckCraft `slide.inspect` as `insert_blocks` blocks. The first slide's
/// title is the document's Title when it is a title slide, every other title a Heading 1; text
/// boxes become bullets (a subtitle stays body text), tables become rows of text, speaker notes
/// body text after the slide's bullets. Hidden slides are left out.
pub fn slides_to_blocks(slides: &[Value]) -> Result<Value, String> {
    let mut blocks = Vec::new();
    let mut block = |text: &str, style: &str| {
        let t = text.trim();
        if !t.is_empty() {
            blocks.push(json!({"text": t, "style": style}));
        }
    };
    for (i, sl) in slides.iter().enumerate() {
        if sl.get("hidden").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let shapes = sl.get("shapes").and_then(Value::as_array).cloned().unwrap_or_default();
        let ph = |s: &Value| s.get("placeholder").and_then(Value::as_str).unwrap_or("").to_string();
        let title = sl.get("title").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let is_title_slide = i == 0 && shapes.iter().any(|s| ph(s) == "ctrTitle");
        let heading = if title.is_empty() { format!("Slide {}", i + 1) } else { title.clone() };
        block(&heading, if is_title_slide { "Title" } else { "Heading 1" });
        for s in &shapes {
            let kind = ph(s);
            if s.get("hidden").and_then(Value::as_bool) == Some(true) || kind == "title" || kind == "ctrTitle" {
                continue;
            }
            if let Some(rows) = s.get("table").and_then(|t| t.get("cells")).and_then(Value::as_array) {
                for r in rows {
                    let cells: Vec<&str> = r.as_array().map(|c| c.iter().filter_map(Value::as_str).map(str::trim).filter(|c| !c.is_empty()).collect()).unwrap_or_default();
                    block(&cells.join(" · "), "Normal");
                }
                continue;
            }
            let Some(text) = s.get("text").and_then(Value::as_str) else { continue };
            // Slide numbers, dates and footers are the deck's furniture, not its content.
            if matches!(kind.as_str(), "sldNum" | "dt" | "ftr") {
                continue;
            }
            let style = if kind == "subTitle" { "Normal" } else { "Bullet" };
            for line in text.lines() {
                block(line, style);
            }
        }
        if let Some(notes) = sl.get("notes").and_then(Value::as_str) {
            for line in notes.lines() {
                block(line, "Normal");
            }
        }
    }
    if blocks.is_empty() {
        return Err("the presentation has no text to make a document of".into());
    }
    Ok(json!({"blocks": blocks, "at": "end"}))
}

/// Rows and columns of a sheet a slide table takes; the rest is left out (and said so).
pub const SLIDE_TABLE_MAX: (usize, usize) = (20, 10);

/// Sheets → Slides: a GridCraft `sheet.read` (formatted) as DeckCraft `insert.table` parameters
/// for a slide of `size` (points) with a title at the top. `(params, cut)`: `cut` when the range
/// had more rows or columns than [`SLIDE_TABLE_MAX`].
pub fn slide_table(read: &Value, size: (f64, f64)) -> Result<(Value, bool), String> {
    let rows: Vec<Vec<String>> = read
        .get("values")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|r| {
                    r.as_array()
                        .map(|c| {
                            c.iter()
                                .map(|v| match v {
                                    Value::String(s) => s.clone(),
                                    Value::Null => String::new(),
                                    other => other.to_string(),
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if rows.is_empty() || cols == 0 || rows.iter().all(|r| r.iter().all(|c| c.trim().is_empty())) {
        return Err("the sheet is empty".into());
    }
    let (max_r, max_c) = SLIDE_TABLE_MAX;
    let cut = rows.len() > max_r || cols > max_c;
    let data: Vec<Vec<String>> = rows.into_iter().take(max_r).map(|mut r| {
        r.resize(cols.min(max_c), String::new());
        r.truncate(max_c);
        r
    }).collect();
    let (n_r, n_c) = (data.len(), cols.min(max_c));
    let (w, h) = size;
    // Under the title, across most of the slide; rows shrink to fit, up to a normal row's height.
    let top = h * 0.24;
    let row_h = ((h * 0.70) / n_r as f64).min(40.0);
    let rect = [w * 0.06, top, w * 0.88, row_h * n_r as f64];
    Ok((json!({"rows": n_r, "cols": n_c, "data": data, "rect": rect}), cut))
}

/// The name a converted copy is offered under: `report.docx` → `report.pdf`.
pub fn renamed(name: &str, ext: &str) -> String {
    format!("{}.{ext}", stem(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(style: &str, text: &str) -> Value {
        json!({"type": "paragraph", "style": style, "text": text})
    }

    #[test]
    fn heading_styles_are_read_by_name_and_by_id() {
        assert_eq!(heading_level("Heading 1"), Some(1));
        assert_eq!(heading_level("Heading2"), Some(2));
        assert_eq!(heading_level("heading 3"), Some(3));
        assert_eq!(heading_level("Title"), Some(0));
        assert_eq!(heading_level("Normal"), None);
        assert_eq!(heading_level("Heading"), None);
        assert_eq!(heading_level("HeadingX"), None);
    }

    #[test]
    fn a_document_outline_becomes_slides() {
        let doc = json!({"blocks": [
            para("Title", "Field Trip"),
            para("Normal", "An overview for parents."),
            para("Heading 1", "Where we go"),
            para("Normal", "The science museum downtown."),
            json!({"type": "paragraph", "style": "Normal", "text": "Bus leaves at 8", "list": {"num": 1, "level": 0}}),
            para("Heading 2", "Lunch"),
            para("Heading 1", "Cost"),
            json!({"type": "table", "cells": [["Item", "Price"], ["Ticket", "$12"], ["", ""]]}),
            para("Heading 1", "Questions"),
        ]});
        let deck = doc_to_slides(&doc, "trip.docx").unwrap();
        assert_eq!(deck["title"], "Field Trip");
        let slides = deck["slides"].as_array().unwrap();
        let titles: Vec<&str> = slides.iter().map(|s| s["title"].as_str().unwrap()).collect();
        assert_eq!(titles, ["Field Trip", "Field Trip", "Where we go", "Cost", "Questions"]);
        assert_eq!(slides[0]["layout"], "title");
        // What came before the first heading, on a slide named after the deck.
        assert_eq!(slides[1]["bullets"], json!(["An overview for parents."]));
        // A Heading 2 under a Heading 1 is a bullet, as are list items.
        assert_eq!(slides[2]["bullets"], json!(["The science museum downtown.", "Bus leaves at 8", "Lunch"]));
        // Table rows as bullets; the empty row is dropped.
        assert_eq!(slides[3]["bullets"], json!(["Item · Price", "Ticket · $12"]));
        // A heading with nothing under it: a section header.
        assert_eq!(slides[4]["layout"], "sectionHeader");
    }

    #[test]
    fn without_headings_the_text_runs_over_slides_named_after_the_file() {
        let blocks: Vec<Value> = (1..=8).map(|n| para("Normal", &format!("Point {n}."))).collect();
        let deck = doc_to_slides(&json!({"blocks": blocks}), "/home/a/Notes.v2.docx").unwrap();
        let slides = deck["slides"].as_array().unwrap();
        assert_eq!(deck["title"], "Notes.v2");
        assert_eq!(slides.len(), 3, "title, then 6 + 2 bullets: {slides:?}");
        assert_eq!(slides[1]["bullets"].as_array().unwrap().len(), MAX_BULLETS);
        assert_eq!(slides[2]["title"], "Notes.v2 (continued)");
    }

    #[test]
    fn a_short_first_line_is_the_title() {
        let doc = json!({"blocks": [para("Normal", "Trip plan"), para("Normal", "We visit the museum."), para("Normal", "Lunch in the park.")]});
        let deck = doc_to_slides(&doc, "Docs 1").unwrap();
        assert_eq!(deck["title"], "Trip plan");
        let slides = deck["slides"].as_array().unwrap();
        assert_eq!(slides.len(), 2);
        assert_eq!(slides[1]["bullets"], json!(["We visit the museum.", "Lunch in the park."]));
        // A sentence is not a title; nor is the only line.
        let doc = json!({"blocks": [para("Normal", "We visit the museum."), para("Normal", "Lunch.")]});
        assert_eq!(doc_to_slides(&doc, "Docs 1").unwrap()["title"], "Docs 1");
        let doc = json!({"blocks": [para("Normal", "Just one line")]});
        assert_eq!(doc_to_slides(&doc, "Notes").unwrap()["title"], "Notes");
    }

    #[test]
    fn the_top_heading_level_present_makes_the_slides() {
        let doc = json!({"blocks": [para("Heading 2", "A"), para("Heading 3", "a1"), para("Heading 2", "B")]});
        let slides = doc_to_slides(&doc, "x").unwrap()["slides"].as_array().unwrap().clone();
        let titles: Vec<&str> = slides.iter().map(|s| s["title"].as_str().unwrap()).collect();
        assert_eq!(titles, ["x", "A", "B"]);
        assert_eq!(slides[1]["bullets"], json!(["a1"]));
    }

    #[test]
    fn an_empty_document_is_an_error() {
        assert!(doc_to_slides(&json!({"blocks": [para("Normal", "  ")]}), "x").is_err());
        assert!(doc_to_slides(&json!({}), "x").is_err());
    }

    #[test]
    fn long_paragraphs_keep_their_first_sentence() {
        let long = format!("Short first sentence. {}", "word ".repeat(60));
        assert_eq!(bullet(&long), "Short first sentence.");
        let run_on = "word ".repeat(60);
        let b = bullet(&run_on);
        assert!(b.ends_with('…') && b.chars().count() <= MAX_BULLET_CHARS, "{b}");
        assert_eq!(bullet("  two   spaces "), "two spaces");
        // Not a sentence end: a decimal point.
        let decimal = format!("Pi is 3.14 and {}", "more ".repeat(40));
        assert!(bullet(&decimal).ends_with('…'));
    }

    #[test]
    fn a_deck_becomes_a_document() {
        let slides = [
            json!({"title": "Field Trip", "shapes": [
                {"placeholder": "ctrTitle", "text": "Field Trip"},
                {"placeholder": "subTitle", "text": "For parents"},
                {"placeholder": "sldNum", "text": "1"}]}),
            json!({"title": "Where", "notes": "Mention the parking.", "shapes": [
                {"placeholder": "title", "text": "Where"},
                {"placeholder": "body", "text": "Museum\nPark\n"}]}),
            json!({"title": "Secret", "hidden": true, "shapes": [{"placeholder": "body", "text": "x"}]}),
            json!({"title": "", "shapes": [{"table": {"cells": [["Item", "Price"], ["Ticket", "$12"]]}}]}),
        ];
        let out = slides_to_blocks(&slides).unwrap();
        let got: Vec<(String, String)> = out["blocks"].as_array().unwrap().iter().map(|b| (b["style"].as_str().unwrap().to_string(), b["text"].as_str().unwrap().to_string())).collect();
        let want = [
            ("Title", "Field Trip"),
            ("Normal", "For parents"),
            ("Heading 1", "Where"),
            ("Bullet", "Museum"),
            ("Bullet", "Park"),
            ("Normal", "Mention the parking."),
            ("Heading 1", "Slide 4"),
            ("Normal", "Item · Price"),
            ("Normal", "Ticket · $12"),
        ];
        assert_eq!(got, want.map(|(a, b)| (a.to_string(), b.to_string())));
    }

    #[test]
    fn a_sheet_range_becomes_a_slide_table() {
        let read = json!({"range": "A1:B3", "values": [["Item", "Price"], ["Ticket", 12], ["Lunch", null]]});
        let (p, cut) = slide_table(&read, (960.0, 540.0)).unwrap();
        assert!(!cut);
        assert_eq!(p["rows"], 3);
        assert_eq!(p["cols"], 2);
        assert_eq!(p["data"], json!([["Item", "Price"], ["Ticket", "12"], ["Lunch", ""]]));
        let r = p["rect"].as_array().unwrap();
        assert!(r[1].as_f64().unwrap() > 100.0, "below the title: {r:?}");
        assert!(r[1].as_f64().unwrap() + r[3].as_f64().unwrap() <= 540.0);
        // A big range is cut to fit, and says so.
        let big: Vec<Vec<Value>> = (0..30).map(|i| (0..14).map(|j| json!(format!("{i},{j}"))).collect()).collect();
        let (p, cut) = slide_table(&json!({"values": big}), (960.0, 540.0)).unwrap();
        assert!(cut);
        assert_eq!((p["rows"].as_u64(), p["cols"].as_u64()), (Some(20), Some(10)));
        let r = p["rect"].as_array().unwrap();
        assert!(r[1].as_f64().unwrap() + r[3].as_f64().unwrap() <= 540.0, "fits the slide: {r:?}");
        // Nothing in it.
        assert!(slide_table(&json!({"values": [["", ""]]}), (960.0, 540.0)).is_err());
        assert!(slide_table(&json!({"range": null, "values": []}), (960.0, 540.0)).is_err());
    }

    #[test]
    fn names_lose_their_folder_and_extension() {
        assert_eq!(stem("/a/b/report.docx"), "report");
        assert_eq!(stem("C:\\x\\Budget 2026.xlsx"), "Budget 2026");
        assert_eq!(stem("Docs 1"), "Docs 1");
        assert_eq!(stem(".hidden"), ".hidden");
        assert_eq!(renamed("deck.pptx", "pdf"), "deck.pdf");
    }
}
