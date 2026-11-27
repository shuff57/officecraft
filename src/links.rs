//! Live links: a Sheets range sent to Docs or Slides as a table that follows the sheet.
//!
//! The link lives in the table itself, in its alternative text, so it is saved with the file and
//! needs no registry: `officecraft-link:` followed by JSON naming the source (a saved file's path,
//! or the tab's name), the sheet, the range and a hash of what the table was made from. Each
//! second the shell reads the sources of the tables it finds and, where the hash differs,
//! replaces the table with the sheet's current values.

use serde_json::{Value, json};
use wordcraft_engine::Session;
use wordcraft_engine::doc::{Block, Path, Pos, StoryRef};

/// Starts a linked table's alternative text.
pub const PREFIX: &str = "officecraft-link:";

/// What a linked table is made from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    /// A saved file's path, or an unsaved tab's name.
    pub source: String,
    pub sheet: String,
    pub range: String,
    /// Hash of the range's shown values when the table was made.
    pub hash: u64,
}

impl Tag {
    pub fn encode(&self) -> String {
        format!("{PREFIX}{}", json!({ "source": self.source, "sheet": self.sheet, "range": self.range, "hash": self.hash.to_string() }))
    }

    pub fn parse(text: &str) -> Option<Tag> {
        let v: Value = serde_json::from_str(text.strip_prefix(PREFIX)?).ok()?;
        Some(Tag {
            source: v.get("source")?.as_str()?.to_string(),
            sheet: v.get("sheet")?.as_str()?.to_string(),
            range: v.get("range")?.as_str()?.to_string(),
            hash: v.get("hash")?.as_str()?.parse().ok()?,
        })
    }

    /// The plain title a screen reader says for the table.
    pub fn title(&self) -> String {
        format!("{}, {} (linked to a sheet)", self.sheet, self.range)
    }
}

/// A hash of a `sheet.read` result's values (FNV-1a over their JSON), the same for equal values.
pub fn hash_values(read: &Value) -> u64 {
    let text = read.get("values").map(Value::to_string).unwrap_or_default();
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

// ---- Docs ----

/// The linked tables at the top of the body: (block index, tag).
pub fn word_links(s: &Session) -> Vec<(usize, Tag)> {
    let Some(blocks) = s.doc.story(StoryRef::Body) else { return Vec::new() };
    blocks
        .iter()
        .enumerate()
        .filter_map(|(i, b)| match &**b {
            Block::Table(t) => t.props.description.as_deref().and_then(Tag::parse).map(|tag| (i, tag)),
            Block::Para(_) => None,
        })
        .collect()
}


fn in_first_cell(block: usize) -> Pos {
    Pos { story: StoryRef::Body, path: Path(vec![block as u32, 0, 0, 0]), off: 0 }
}

fn block_count(s: &Session) -> usize {
    s.doc.story(StoryRef::Body).map_or(0, |b| b.len())
}

/// Writes `tag` on the table at top-level block `block`.
fn word_tag(s: &mut Session, block: usize, tag: &Tag) -> Result<(), String> {
    s.sel = wordcraft_engine::Selection::caret(in_first_cell(block));
    s.run("table.altText", &json!({ "title": tag.title(), "text": tag.encode() })).map(|_| ()).map_err(|e| e.to_string())
}

/// Marks the table pasted since `before` was taken as linked: the one that is not among them.
pub fn word_mark_new(s: &mut Session, before: &[usize], tag: &Tag) -> Result<(), String> {
    let now = word_tables(s);
    let block = now.iter().find(|(_, id)| !before.contains(id)).map(|(b, _)| *b).ok_or("the table was not made")?;
    word_tag(s, block, tag)
}

/// Identities of the top-level tables now, for [`word_mark_new`] to tell a pasted one from them
/// (a block keeps its identity while it is not edited, wherever it moves to).
pub fn word_table_ids(s: &Session) -> Vec<usize> {
    word_tables(s).into_iter().map(|(_, id)| id).collect()
}

fn word_tables(s: &Session) -> Vec<(usize, usize)> {
    s.doc
        .story(StoryRef::Body)
        .map(|blocks| blocks.iter().enumerate().filter(|(_, b)| matches!(&***b, Block::Table(_))).map(|(i, b)| (i, std::sync::Arc::as_ptr(b) as usize)).collect())
        .unwrap_or_default()
}

/// Replaces the linked table at `block` with the pasted `text`/`html` and tags the new one.
/// The caret stays where the person had it.
pub fn word_refresh(s: &mut Session, block: usize, text: &str, html: &str, tag: &Tag) -> Result<(), String> {
    let saved = s.sel.clone();
    let before_blocks = block_count(s);
    s.sel = wordcraft_engine::Selection::caret(in_first_cell(block));
    let result = (|| {
        s.run("table.deleteTable", &json!({})).map_err(|e| e.to_string())?;
        let others = word_table_ids(s);
        s.run("edit.paste", &json!({ "text": text, "html": html })).map_err(|e| e.to_string())?;
        word_mark_new(s, &others, tag)
    })();
    // Put the caret back; blocks the refresh added or removed above it shift it by the difference.
    let shift = block_count(s) as i64 - before_blocks as i64;
    let mut restore = saved;
    for p in [&mut restore.anchor, &mut restore.focus] {
        if p.story == StoryRef::Body
            && let Some(first) = p.path.0.first_mut()
            && *first as usize > block
        {
            *first = (i64::from(*first) + shift).max(0) as u32;
        }
    }
    s.sel = restore;
    s.clamp_selection();
    result
}

// ---- Slides ----

/// A linked table on a slide.
#[derive(Clone, Debug, PartialEq)]
pub struct DeckLink {
    pub slide: usize,
    pub id: u32,
    /// x, y, width, height on the slide, points.
    pub rect: [f64; 4],
    pub rows: usize,
    pub tag: Tag,
}

/// The linked tables on the slides (top-level shapes), in slide order.
pub fn deck_links(s: &deckcraft_engine::Session) -> Vec<DeckLink> {
    let Ok(st) = s.doc() else { return Vec::new() };
    let mut out = Vec::new();
    for (slide, sl) in st.doc.slides.iter().enumerate() {
        for sh in &sl.shapes {
            let deckcraft_engine::model::ShapeKind::Table(t) = &sh.kind else { continue };
            if !sh.descr.starts_with(PREFIX) {
                continue;
            }
            let (Some(tag), Some(x)) = (Tag::parse(&sh.descr), sh.xfrm) else { continue };
            let rows = t.n_rows();
            out.push(DeckLink { slide, id: sh.id.0, rect: [x.x, x.y, x.w, x.h], rows, tag });
        }
    }
    out
}

/// A `sheet.read` result's values as rows of text (empty cells as empty strings).
pub fn values_as_strings(read: &Value) -> Vec<Vec<String>> {
    read.get("values")
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
        .unwrap_or_default()
}

/// Writes `tag` on table shape `id`, through `run` (a Slides command).
pub fn deck_tag_via(mut run: impl FnMut(&str, Value) -> Result<Value, String>, id: u64, tag: &Tag) -> Result<(), String> {
    run("shape.altText", json!({ "ids": [id], "text": tag.encode() })).map(|_| ())
}

/// Writes `tag` on table shape `id`.
pub fn deck_tag(s: &mut deckcraft_engine::Session, id: u64, tag: &Tag) -> Result<(), String> {
    s.execute("shape.altText", &json!({ "ids": [id], "text": tag.encode() })).map(|_| ()).map_err(|e| e.to_string())
}

/// Replaces the linked table with a new one made from `data` (rows of shown values), in the same
/// place and as wide, then tags it. The slide and shapes the person had selected stay selected.
pub fn deck_refresh(s: &mut deckcraft_engine::Session, link: &DeckLink, data: &[Vec<String>], tag: &Tag) -> Result<(), String> {
    let saved = s.doc().map_err(|e| e.to_string())?.selection.clone();
    let result = (|| {
        s.select(|_, sel| {
            sel.slide = link.slide;
            sel.shapes.clear();
            sel.text = None;
            sel.cells = None;
        })
        .map_err(|e| e.to_string())?;
        let (rows, cols) = (data.len(), data.iter().map(Vec::len).max().unwrap_or(0));
        if rows == 0 || cols == 0 {
            return Err("the range is empty".to_string());
        }
        let row_h = (link.rect[3] / link.rows.max(1) as f64).max(8.0);
        let rect = [link.rect[0], link.rect[1], link.rect[2], row_h * rows as f64];
        // Made first, so a failure leaves the old table where it was.
        let made = s.execute("insert.table", &json!({ "rows": rows, "cols": cols, "data": data, "rect": rect })).map_err(|e| e.to_string())?;
        let new = made.get("id").and_then(Value::as_u64).ok_or("the table was not made")?;
        s.execute("edit.delete", &json!({ "ids": [link.id] })).map_err(|e| e.to_string())?;
        deck_tag(s, new, tag)
    })();
    let gone = link.id;
    let _ = s.select(|_, sel| {
        *sel = saved;
        sel.shapes.retain(|x| x.0 != gone);
        if sel.cells.is_some_and(|(t, ..)| t.0 == gone) {
            sel.cells = None;
        }
        if sel.text.as_ref().is_some_and(|t| t.shape.0 == gone) {
            sel.text = None;
        }
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordcraft_engine::doc::Document;

    fn tag(hash: u64) -> Tag {
        Tag { source: "budget.xlsx".into(), sheet: "Sheet1".into(), range: "A1:B2".into(), hash }
    }

    fn html(rows: &[&[&str]]) -> (String, String) {
        let text = rows.iter().map(|r| r.join("\t")).collect::<Vec<_>>().join("\n");
        let body: String = rows.iter().map(|r| format!("<tr>{}</tr>", r.iter().map(|c| format!("<td>{c}</td>")).collect::<String>())).collect();
        (text, format!("<table>{body}</table>"))
    }

    fn texts(s: &Session) -> Vec<String> {
        s.doc.story(StoryRef::Body).unwrap().iter().map(|b| match &**b {
            Block::Para(p) => format!("P:{}", p.plain_text()),
            Block::Table(t) => format!("T:{}x{}", t.rows.len(), t.rows.first().map_or(0, |r| r.cells.len())),
        }).collect()
    }

    fn grid(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter().map(|r| r.iter().map(|c| c.to_string()).collect()).collect()
    }

    #[test]
    fn a_slide_table_is_refreshed_in_place_and_the_selection_comes_back() {
        let mut s = deckcraft_engine::Session::new();
        s.execute("file.new", &json!({})).unwrap();
        s.execute("slide.new", &json!({"layout": "titleOnly", "title": "Budget"})).unwrap();
        let data = grid(&[&["a", "1"], &["b", "2"]]);
        let made = s.execute("insert.table", &json!({"rows": 2, "cols": 2, "data": data, "rect": [50, 120, 400, 80]})).unwrap();
        let id = made["id"].as_u64().unwrap() as u32;
        deck_tag(&mut s, u64::from(id), &tag(1)).unwrap();
        // The person is on another slide with nothing picked.
        s.execute("slide.new", &json!({"layout": "titleOnly", "title": "Other"})).unwrap();
        let on = s.doc().unwrap().selection.slide;
        let links = deck_links(&s);
        assert_eq!(links.len(), 1);
        assert_eq!((links[0].slide, links[0].id, links[0].rows, links[0].tag.clone()), (1, id, 2, tag(1)));
        assert_eq!(links[0].rect, [50.0, 120.0, 400.0, 80.0]);

        let data = grid(&[&["a", "1", "x"], &["b", "2", "y"], &["c", "3", "z"]]);
        deck_refresh(&mut s, &links[0], &data, &tag(2)).unwrap();
        let after = deck_links(&s);
        assert_eq!(after.len(), 1, "exactly one linked table is left");
        assert_ne!(after[0].id, id);
        assert_eq!((after[0].slide, after[0].rows, after[0].tag.clone()), (1, 3, tag(2)));
        assert_eq!((after[0].rect[0], after[0].rect[1], after[0].rect[2]), (50.0, 120.0, 400.0));
        assert_eq!(s.doc().unwrap().selection.slide, on, "the slide the person was on");
        let shown = s.execute("slide.inspect", &json!({"index": 1})).unwrap();
        let table = shown["shapes"].as_array().unwrap().iter().find(|x| x["kind"] == "table").unwrap();
        assert_eq!(table["table"]["cells"][2][2], "z");
    }

    #[test]
    fn a_failed_slide_refresh_leaves_the_old_table() {
        let mut s = deckcraft_engine::Session::new();
        s.execute("file.new", &json!({})).unwrap();
        s.execute("slide.new", &json!({"layout": "titleOnly", "title": "Budget"})).unwrap();
        let made = s.execute("insert.table", &json!({"rows": 1, "cols": 1, "data": [["a"]], "rect": [50, 120, 400, 40]})).unwrap();
        deck_tag(&mut s, made["id"].as_u64().unwrap(), &tag(1)).unwrap();
        let links = deck_links(&s);
        assert!(deck_refresh(&mut s, &links[0], &[], &tag(2)).is_err());
        assert_eq!(deck_links(&s), links);
    }

    #[test]
    fn links_survive_saving_and_reopening() {
        let dir = std::env::temp_dir().join(format!("officecraft-links-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Docs.
        let mut s = Session::new(Document::from_text("Intro"));
        let (text, h) = html(&[&["a", "1"], &["b", "2"]]);
        let before = word_table_ids(&s);
        s.run("edit.paste", &json!({ "text": text, "html": h })).unwrap();
        word_mark_new(&mut s, &before, &tag(7)).unwrap();
        let path = dir.join("linked.docx");
        s.run("file.saveAs", &json!({ "path": path.to_string_lossy() })).unwrap();
        let mut again = Session::new(Document::from_text(""));
        again.run("file.open", &json!({ "path": path.to_string_lossy() })).unwrap();
        let found = word_links(&again);
        assert_eq!(found.len(), 1, "the docx kept the link");
        assert_eq!(found[0].1, tag(7));
        // Slides.
        let mut d = deckcraft_engine::Session::new();
        d.execute("file.new", &json!({})).unwrap();
        d.execute("slide.new", &json!({"layout": "titleOnly", "title": "T"})).unwrap();
        let made = d.execute("insert.table", &json!({"rows": 1, "cols": 1, "data": [["a"]], "rect": [50, 120, 400, 40]})).unwrap();
        deck_tag(&mut d, made["id"].as_u64().unwrap(), &tag(8)).unwrap();
        let path = dir.join("linked.pptx");
        d.execute("file.saveAs", &json!({ "path": path.to_string_lossy() })).unwrap();
        let mut again = deckcraft_engine::Session::new();
        again.execute("file.open", &json!({ "path": path.to_string_lossy() })).unwrap();
        let found = deck_links(&again);
        assert_eq!(found.len(), 1, "the pptx kept the link");
        assert_eq!(found[0].tag, tag(8));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tag_round_trips() {
        let t = tag(42);
        assert_eq!(Tag::parse(&t.encode()), Some(t));
        assert_eq!(Tag::parse("a plain description"), None);
    }

    #[test]
    fn same_values_hash_the_same() {
        let a = json!({"values": [["a", "1"]]});
        assert_eq!(hash_values(&a), hash_values(&json!({"values": [["a", "1"]], "range": "A1:B1"})));
        assert_ne!(hash_values(&a), hash_values(&json!({"values": [["a", "2"]]})));
    }

    #[test]
    fn a_pasted_table_is_linked_and_refreshed_in_place() {
        let mut s = Session::new(Document::from_text("Before\nAfter"));
        s.sel = wordcraft_engine::Selection::caret(s.doc.end_of(StoryRef::Body).clone());
        let first = s.doc.start_of(StoryRef::Body);
        let end_of_first = Pos { off: s.doc.para(StoryRef::Body, &first.path).unwrap().len(), ..first };
        s.sel = wordcraft_engine::Selection::caret(end_of_first);
        s.run("insert.paragraph", &json!({})).ok();
        let (text, h) = html(&[&["a", "1"], &["b", "2"]]);
        let before = word_table_ids(&s);
        s.run("edit.paste", &json!({ "text": text, "html": h })).unwrap();
        word_mark_new(&mut s, &before, &tag(1)).unwrap();
        let links = word_links(&s);
        assert_eq!(links.len(), 1, "{:?}", texts(&s));
        assert_eq!(links[0].1, tag(1));
        let shape_before = texts(&s);
        assert!(shape_before.iter().any(|t| t == "T:2x2"), "{shape_before:?}");

        // The source grew by a row and a column.
        let (text, h) = html(&[&["a", "1", "x"], &["b", "2", "y"], &["c", "3", "z"]]);
        word_refresh(&mut s, links[0].0, &text, &h, &tag(2)).unwrap();
        let after = texts(&s);
        assert!(after.iter().any(|t| t == "T:3x3"), "{after:?}");
        assert_eq!(after.iter().filter(|t| t.starts_with("T:")).count(), 1, "{after:?}");
        assert_eq!(word_links(&s).len(), 1);
        assert_eq!(word_links(&s)[0].1, tag(2));
        // The text around it is still there, in the same order.
        let paras: Vec<&String> = after.iter().filter(|t| t.starts_with("P:") && t.len() > 2).collect();
        assert_eq!(paras, ["P:Before", "P:After"], "{after:?}");
        let table_at = after.iter().position(|t| t.starts_with("T:")).unwrap();
        let before_at = after.iter().position(|t| t == "P:Before").unwrap();
        let after_at = after.iter().position(|t| t == "P:After").unwrap();
        assert!(before_at < table_at && table_at < after_at, "{after:?}");
    }
}
