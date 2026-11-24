//! The formatted clipboard the apps share (desktop build), so a copy in one tab pastes with its
//! formatting in another: a Sheets range becomes a table in Docs or Slides, a Docs passage keeps
//! its bold, lists and links.
//!
//! Each app copies HTML beside its plain text through the hooks wired here, and reads HTML back
//! when it pastes. The HTML goes onto the system clipboard (so other programs get it too), and a
//! copy stays in this process: where the desktop can't carry HTML (a Wayland compositor without
//! the data-control protocol), a paste of the same text inside OfficeCraft still gets it.

use std::sync::Mutex;

/// The largest HTML read from the clipboard; a bigger one pastes as plain text.
const MAX_HTML: usize = 32 * 1024 * 1024;

/// The last formatted copy made in OfficeCraft: (plain text, HTML).
static LAST: Mutex<Option<(String, String)>> = Mutex::new(None);

thread_local! {
    /// The clipboard copies are set through, kept open: on X11 its owner serves the contents.
    static WRITER: std::cell::RefCell<Option<arboard::Clipboard>> = const { std::cell::RefCell::new(None) };
}

fn open() -> Option<arboard::Clipboard> {
    arboard::Clipboard::new().map_err(|e| log::warn!("clipboard: {e}")).ok()
}

/// Line endings and trailing space differ between the clipboard and the app's own text.
fn same_text(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.replace("\r\n", "\n").trim_end().to_string();
    norm(a) == norm(b)
}

/// Puts `html` with its plain-text alternative `text` on the clipboard. `true` when the system
/// clipboard has both; otherwise the app copies the text alone, and OfficeCraft's own tabs still
/// paste the HTML.
pub fn set_html(html: &str, text: &str) -> bool {
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some((text.to_string(), html.to_string()));
    WRITER.with(|w| {
        let Ok(mut slot) = w.try_borrow_mut() else { return false };
        if slot.is_none() {
            *slot = open();
        }
        match slot.as_mut().map(|c| c.set().html(html, Some(text))) {
            Some(Ok(())) => true,
            Some(Err(e)) => {
                log::warn!("clipboard: {e}");
                *slot = None;
                false
            }
            None => false,
        }
    })
}

/// The clipboard's text.
pub fn text() -> Option<String> {
    open()?.get_text().ok()
}

/// The HTML to paste: OfficeCraft's own last copy while the clipboard still holds its text,
/// else the HTML another program put there.
pub fn html() -> Option<String> {
    let current = text();
    if let Some((t, h)) = LAST.lock().unwrap_or_else(|e| e.into_inner()).clone()
        && current.as_deref().is_some_and(|c| same_text(c, &t))
    {
        return Some(h);
    }
    open()?.get().html().ok().filter(|h| h.len() <= MAX_HTML)
}

#[cfg(test)]
mod tests {
    use super::same_text;

    #[test]
    fn clipboard_text_matches_across_line_endings() {
        assert!(same_text("a\tb\r\nc\td\r\n", "a\tb\nc\td"));
        assert!(!same_text("a\tb", "a\tc"));
    }
}
