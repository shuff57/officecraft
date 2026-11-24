//! The `postMessage` bridge to a same-origin parent page (`?host=parent`), for a page that embeds
//! OfficeCraft in an `<iframe>` and keeps its files (the OpenCloud extension in
//! `integrations/opencloud`). The same shape as the WordCraft, GridCraft and PdfCraft bridges,
//! under the `officecraft:` prefix; each message is a plain object with a `type`:
//!
//! - `officecraft:ready` (to the parent): `{ version }`, once the suite is running and listening.
//!   Send a file now.
//! - `officecraft:open` (from the parent): `{ name, bytes }`, bytes an `ArrayBuffer` or
//!   `Uint8Array`. It opens in the tab for its kind, like a file dropped on the window.
//! - `officecraft:save` (to the parent): `{ name, saveAs, bytes }` (a transferred `ArrayBuffer`)
//!   for every file OfficeCraft would otherwise write or download: Save of the opened file comes
//!   back under its own name, exports and Save As under the name the user chose. The parent
//!   stores it and reports failures itself.
//! - `officecraft:dirty` (to the parent): `{ dirty }` whenever unsaved work appears or goes away.
//! - `officecraft:failed` (to the parent): `{ error }` when the suite couldn't start.
//!
//! Only the parent window on the page's own origin is heard or answered.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;

struct Bridge {
    parent: web_sys::Window,
    origin: String,
}

thread_local! {
    static BRIDGE: RefCell<Option<Bridge>> = const { RefCell::new(None) };
    /// Files the parent sent, waiting for the next frame.
    static ARRIVED: RefCell<Vec<(String, Vec<u8>)>> = const { RefCell::new(Vec::new()) };
    /// The last dirty state the parent was told, so it hears only changes.
    static REPORTED: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Whether the page asked for host mode (`?host=parent`).
pub fn requested() -> bool {
    let search = web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default();
    search.trim_start_matches('?').split('&').any(|kv| kv == "host=parent")
}

/// Starts host mode when it was asked for and a same-origin parent frames the page. `repaint`
/// wakes the next frame when a file arrives.
pub fn connect(repaint: Rc<dyn Fn()>) -> bool {
    if !requested() {
        return false;
    }
    let Some(window) = web_sys::window() else { return false };
    let Some(parent) = window.parent().ok().flatten() else { return false };
    if js_sys::Object::is(&parent, &window) {
        return false;
    }
    let Ok(origin) = window.location().origin() else { return false };
    // An opaque origin can't be a `postMessage` target, and another origin's location throws:
    // messages only go to this origin, so the browser would drop every save without an error.
    if origin == "null" || parent.location().origin().ok().as_deref() != Some(origin.as_str()) {
        log::error!("officecraft: ?host=parent needs a parent page on {origin}");
        return false;
    }
    let (from, heard) = (parent.clone(), origin.clone());
    let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
        if event.origin() != heard || !event.source().is_some_and(|s| js_sys::Object::is(&s, &from)) {
            return;
        }
        let data = event.data();
        if string(&data, "type").as_deref() != Some("officecraft:open") {
            return;
        }
        let Some(name) = string(&data, "name").filter(|n| !n.trim().is_empty()) else { return };
        let Some(bytes) = bytes(&data) else { return };
        ARRIVED.with(|a| a.borrow_mut().push((name, bytes)));
        repaint();
    });
    if window.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref()).is_err() {
        return false;
    }
    // The listener lives as long as the page.
    on_message.forget();
    BRIDGE.with(|b| *b.borrow_mut() = Some(Bridge { parent, origin }));
    true
}

pub fn active() -> bool {
    BRIDGE.with(|b| b.borrow().is_some())
}

pub fn post_ready() {
    send(&[("type", "officecraft:ready".into()), ("version", env!("CARGO_PKG_VERSION").into())]);
}

pub fn post_failed(error: &str) {
    send(&[("type", "officecraft:failed".into()), ("error", error.into())]);
}

/// Called every frame with whether any tab has unsaved work; the parent hears changes only.
pub fn report_dirty(dirty: bool) {
    if REPORTED.with(Cell::get) != Some(dirty) && send(&[("type", "officecraft:dirty".into()), ("dirty", dirty.into())]) {
        REPORTED.with(|r| r.set(Some(dirty)));
    }
}

/// The files the parent sent since the last call.
pub fn take_arrived() -> Vec<(String, Vec<u8>)> {
    ARRIVED.with(|a| std::mem::take(&mut *a.borrow_mut()))
}

/// Hands a saved or exported file to the parent.
pub fn save(path: &str, bytes: &[u8], save_as: bool) -> Result<(), String> {
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
    let buffer = js_sys::Uint8Array::from(bytes).buffer();
    let message = message(&[
        ("type", "officecraft:save".into()),
        ("name", name.as_str().into()),
        ("saveAs", save_as.into()),
        ("bytes", buffer.clone().into()),
    ])
    .ok_or("couldn't build the message")?;
    BRIDGE.with(|b| {
        let b = b.borrow();
        let b = b.as_ref().ok_or("not in host mode")?;
        b.parent.post_message_with_transfer(&message, &b.origin, &js_sys::Array::of1(&buffer)).map_err(|e| format!("{e:?}"))
    })
}

/// Posts a message to the parent; false if it couldn't be sent.
fn send(fields: &[(&str, JsValue)]) -> bool {
    let Some(m) = message(fields) else { return false };
    BRIDGE.with(|b| b.borrow().as_ref().is_some_and(|b| b.parent.post_message(&m, &b.origin).is_ok()))
}

fn message(fields: &[(&str, JsValue)]) -> Option<js_sys::Object> {
    let m = js_sys::Object::new();
    for (key, value) in fields {
        js_sys::Reflect::set(&m, &JsValue::from_str(key), value).ok()?;
    }
    Some(m)
}

fn string(data: &JsValue, key: &str) -> Option<String> {
    js_sys::Reflect::get(data, &JsValue::from_str(key)).ok()?.as_string()
}

/// `bytes` as an `ArrayBuffer` or `Uint8Array`.
fn bytes(data: &JsValue) -> Option<Vec<u8>> {
    let bytes = js_sys::Reflect::get(data, &JsValue::from_str("bytes")).ok()?;
    if let Some(array) = bytes.dyn_ref::<js_sys::Uint8Array>() {
        Some(array.to_vec())
    } else if bytes.is_instance_of::<js_sys::ArrayBuffer>() {
        Some(js_sys::Uint8Array::new(&bytes).to_vec())
    } else {
        None
    }
}
