//! The browser shell: the eframe web runner, the per-app web `Services` (the same ones each
//! `*-web` app uses: the file picker feeds an inbox, Save downloads), drag and drop and the
//! unsaved-changes guard.
//!
//! A page has no file system. Files arrive as bytes, open in their own tab and stay in the page;
//! Save and Export hand a copy to the browser's download.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use eframe::egui;
use wasm_bindgen::JsCast as _;

use crate::{Inbox, Suite};

const CANVAS_ID: &str = "officecraft_canvas";
const LOADING_ID: &str = "officecraft_loading";

const DOC_EXTS: &[&str] = &["docx", "docm", "dotx", "dotm", "doc", "dot", "odt", "rtf", "txt", "md", "html", "htm", "tex", "json"];
const SHEET_EXTS: &[&str] = &["xlsx", "xlsm", "xls", "xlsb", "ods", "csv", "tsv", "txt", "json"];
const PRESENTATION_EXTS: &[&str] = &["deckcraft", "pptx", "potx", "ppsx"];
const PICTURE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];
const AUDIO_EXTS: &[&str] = &["mp3", "wav", "ogg", "opus", "m4a", "flac"];
const VIDEO_EXTS: &[&str] = &["mp4", "webm", "mkv", "mov", "m4v"];
const RECIPIENT_EXTS: &[&str] = &["csv", "tsv", "txt"];

thread_local! {
    /// The page's egui context, so a file the browser finishes reading wakes the next frame.
    static CTX: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
    /// True while any open document has unsaved work (what the `beforeunload` guard reads).
    static DIRTY: Rc<Cell<bool>> = Rc::new(Cell::new(false));
}

fn repaint() {
    CTX.with(|c| {
        if let Some(ctx) = c.borrow().as_ref() {
            ctx.request_repaint();
        }
    });
}

/// Called each frame by the shell: whether any tab has unsaved work.
pub fn set_dirty(dirty: bool) {
    DIRTY.with(|d| d.set(dirty));
}

pub fn start() {
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    wasm_bindgen_futures::spawn_local(async {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            log::error!("no document");
            return;
        };
        let Some(canvas) = document.get_element_by_id(CANVAS_ID).and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok()) else {
            log::error!("missing <canvas id=\"{CANVAS_ID}\">");
            return;
        };
        if let Err(e) = guard_unload() {
            log::error!("no unsaved-changes guard: {e}");
        }
        let mut options = eframe::WebOptions::default();
        if query().contains("webgl")
            && let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup
        {
            create.instance_descriptor.backends = eframe::wgpu::Backends::GL;
        }
        let result = eframe::WebRunner::new()
            .start(
                canvas,
                options,
                Box::new(|cc| {
                    CTX.with(|c| *c.borrow_mut() = Some(cc.egui_ctx.clone()));
                    if let Some(rs) = &cc.wgpu_render_state {
                        log::info!("officecraft-web: wgpu backend {:?}", rs.adapter.get_info().backend);
                    }
                    let mut suite = Suite::new();
                    suite.persist = false;
                    Ok(Box::new(suite))
                }),
            )
            .await;
        if let Some(el) = document.get_element_by_id(LOADING_ID) {
            match result {
                Ok(()) => el.remove(),
                Err(e) => el.set_inner_html(&format!("<p>OfficeCraft failed to start: {e:?}</p><p>A browser with WebGPU or WebGL2 is required.</p>")),
            }
        }
    });
}

fn query() -> String {
    web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default()
}

/// Browsers show their own confirmation (a page can only ask for it), so there is no Save
/// button in it: Cancel, then Save.
fn guard_unload() -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let on_unload = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::BeforeUnloadEvent)>::new(move |e: web_sys::BeforeUnloadEvent| {
        if DIRTY.with(|d| d.get()) {
            e.prevent_default();
            // Older browsers ask only when `returnValue` is set.
            e.set_return_value("unsaved changes");
        }
    });
    window.add_event_listener_with_callback("beforeunload", on_unload.as_ref().unchecked_ref()).map_err(|e| format!("{e:?}"))?;
    // The listener lives as long as the page.
    on_unload.forget();
    Ok(())
}

/// Asks the browser for files and queues each one's bytes in `inbox`.
pub fn pick_files(exts: &'static [&'static str], inbox: Inbox) {
    wasm_bindgen_futures::spawn_local(async move {
        let Some(files) = rfd::AsyncFileDialog::new().add_filter("Office files", exts).pick_files().await else {
            return;
        };
        for file in files {
            let bytes = file.read().await;
            inbox.lock().unwrap_or_else(|e| e.into_inner()).push((file.file_name(), bytes));
        }
        repaint();
    });
}

/// A dropped file's bytes are only readable asynchronously.
pub fn read_dropped(file: egui::DroppedFileHandle, name: String, inbox: Inbox, ctx: egui::Context) {
    wasm_bindgen_futures::spawn_local(async move {
        match file.bytes_async().await {
            Ok(bytes) => {
                inbox.lock().unwrap_or_else(|e| e.into_inner()).push((name, bytes));
                ctx.request_repaint();
            }
            Err(e) => log::error!("couldn't read dropped file {name}: {e}"),
        }
    });
}

/// Starts an asynchronous pick with `dialog` and queues the file's bytes in `inbox`.
fn pick_into(dialog: rfd::AsyncFileDialog, inbox: Inbox) {
    wasm_bindgen_futures::spawn_local(async move {
        let Some(file) = dialog.pick_file().await else {
            return;
        };
        let bytes = file.read().await;
        inbox.lock().unwrap_or_else(|e| e.into_inner()).push((file.file_name(), bytes));
        repaint();
    });
}

pub fn word_services() -> wordcraft_ui_egui::Services {
    let inbox = Inbox::default();
    let open_inbox = inbox.clone();
    wordcraft_ui_egui::Services {
        open_async: Some(Box::new(move |purpose: &str| {
            let dialog = match purpose {
                "picture" => rfd::AsyncFileDialog::new().add_filter("Pictures", PICTURE_EXTS),
                "recipients" => rfd::AsyncFileDialog::new().add_filter("Recipient lists", RECIPIENT_EXTS),
                _ => rfd::AsyncFileDialog::new().add_filter("Documents", DOC_EXTS),
            };
            pick_into(dialog, open_inbox.clone());
        })),
        // Exports ask for a name; the browser decides where the download goes.
        pick_save: Some(Box::new(|name: &str| Some(name.to_string()))),
        download: Some(Box::new(download)),
        print: Some(Box::new(print_pdf)),
        inbox: Some(inbox),
        ..Default::default()
    }
}

pub fn grid_services() -> gridcraft_ui_egui::Services {
    let inbox = Inbox::default();
    let open_inbox = inbox.clone();
    gridcraft_ui_egui::Services {
        copy_html: Some(Box::new(copy_html)),
        open_async: Some(Box::new(move || {
            pick_into(rfd::AsyncFileDialog::new().add_filter("Spreadsheets", SHEET_EXTS), open_inbox.clone());
        })),
        pick_picture_async: Some(Box::new(|inbox| {
            pick_into(rfd::AsyncFileDialog::new().add_filter("Pictures (PNG, JPEG)", &["png", "jpg", "jpeg"]), inbox);
        })),
        download: Some(Box::new(|name: &str, bytes: &[u8]| {
            if let Err(e) = download(name, bytes) {
                log::error!("download of {name} failed: {e}");
            }
        })),
        print: Some(Box::new(|bytes: &[u8]| {
            if let Err(e) = print_pdf(bytes) {
                log::error!("print failed: {e}");
            }
        })),
        open_url: Some(Box::new(|url: &str| {
            if let Some(w) = web_sys::window() {
                let _ = w.open_with_url_and_target(url, "_blank");
            }
        })),
        inbox: Some(inbox),
        ..Default::default()
    }
}

pub fn deck_services() -> deckcraft_ui_egui::Services {
    let inbox = Inbox::default();
    let open_inbox = inbox.clone();
    deckcraft_ui_egui::Services {
        open_async: Some(Box::new(move |purpose: &str| {
            let dialog = match purpose {
                "picture" => rfd::AsyncFileDialog::new().add_filter("Pictures", PICTURE_EXTS),
                "audio" => rfd::AsyncFileDialog::new().add_filter("Audio", AUDIO_EXTS),
                "video" => rfd::AsyncFileDialog::new().add_filter("Video", VIDEO_EXTS),
                _ => rfd::AsyncFileDialog::new().add_filter("Presentations", PRESENTATION_EXTS),
            };
            pick_into(dialog, open_inbox.clone());
        })),
        write: Some(Box::new(|name: &str, bytes: &[u8]| download(name, bytes))),
        download: Some(Box::new(|name: &str, bytes: &[u8]| {
            if let Err(e) = download(name, bytes) {
                log::error!("download of {name} failed: {e}");
            }
        })),
        inbox: Some(inbox),
        ..Default::default()
    }
}

/// Start inside the copy gesture; deferring `write` can lose browser user activation.
fn copy_html(html: &str, text: &str) -> Result<(), String> {
    let js = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let window = web_sys::window().ok_or("no window")?;
    let clipboard = window.navigator().clipboard();
    if clipboard.is_undefined() || clipboard.is_null() {
        return Err("browser clipboard is unavailable (a secure context is required)".into());
    }
    if !js_sys::Reflect::get(&clipboard, &"write".into()).map_err(js)?.is_function() {
        return Err("browser does not support rich clipboard writes".into());
    }
    let data = js_sys::Object::new();
    for (mime, value) in [("text/html", html), ("text/plain", text)] {
        let opts = web_sys::BlobPropertyBag::new();
        opts.set_type(mime);
        let parts = js_sys::Array::of1(&wasm_bindgen::JsValue::from_str(value));
        let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts).map_err(js)?;
        js_sys::Reflect::set(&data, &mime.into(), &blob).map_err(js)?;
    }
    let item = web_sys::ClipboardItem::new_with_record_from_str_to_blob_promise(&data).map_err(js)?;
    let promise = clipboard.write(&js_sys::Array::of1(&item));
    let text = text.to_owned();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(e) = wasm_bindgen_futures::JsFuture::from(promise).await {
            log::warn!("Rich clipboard write failed; trying plain text: {e:?}");
            if let Err(e) = wasm_bindgen_futures::JsFuture::from(clipboard.write_text(&text)).await {
                log::error!("Plain-text clipboard fallback also failed: {e:?}");
            }
        }
    });
    Ok(())
}

/// Trigger a browser download of `bytes` named after the last component of `path`.
fn download(path: &str, bytes: &[u8]) -> Result<(), String> {
    let js = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "document".into());
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime_for(&name));
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(js)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(js)?;
    let a: web_sys::HtmlAnchorElement = document.create_element("a").map_err(js)?.dyn_into().map_err(|_| "not an anchor")?;
    a.set_href(&url);
    a.set_download(&name);
    a.style().set_property("display", "none").map_err(js)?;
    let body = document.body().ok_or("no body")?;
    body.append_child(&a).map_err(js)?;
    a.click();
    a.remove();
    // Revoke after the click has been dispatched; the download keeps its own reference.
    let revoke = wasm_bindgen::closure::Closure::once_into_js(move || {
        web_sys::Url::revoke_object_url(&url).ok();
    });
    window.set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 10_000).map_err(js)?;
    Ok(())
}

/// The hidden iframe the last print loaded its PDF into.
const PRINT_FRAME_ID: &str = "officecraft-print-frame";

/// Opens `bytes` (a PDF) in a hidden iframe and calls its own `print()` once it has loaded: the
/// standard way to reach the browser's print dialog without a download or a blockable popup.
fn print_pdf(bytes: &[u8]) -> Result<(), String> {
    let js = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;

    if let Some(old) = document.get_element_by_id(PRINT_FRAME_ID) {
        if let Ok(frame) = old.clone().dyn_into::<web_sys::HtmlIFrameElement>() {
            let src = frame.src();
            if src.starts_with("blob:") {
                web_sys::Url::revoke_object_url(&src).ok();
            }
        }
        old.remove();
    }

    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/pdf");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(js)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(js)?;

    let iframe: web_sys::HtmlIFrameElement =
        match document.create_element("iframe").map_err(js).and_then(|e| e.dyn_into().map_err(|_| "not an iframe".to_string())) {
            Ok(f) => f,
            Err(e) => {
                web_sys::Url::revoke_object_url(&url).ok();
                return Err(e);
            }
        };
    iframe.set_id(PRINT_FRAME_ID);
    let attached = iframe
        .style()
        .set_property("display", "none")
        .map_err(js)
        .and_then(|()| document.body().ok_or_else(|| "no body".to_string()))
        .and_then(|body| body.append_child(&iframe).map_err(js));
    if let Err(e) = attached {
        iframe.remove();
        web_sys::Url::revoke_object_url(&url).ok();
        return Err(e);
    }

    // Set the handler BEFORE `src` so a fast or cached load can't fire before we're listening.
    let iframe_for_load = iframe.clone();
    let on_load = wasm_bindgen::closure::Closure::once_into_js(move || {
        if let Some(w) = iframe_for_load.content_window() {
            // Some browsers print fine but report a benign cross-origin-style error here.
            let _ = w.print();
        }
    });
    iframe.set_onload(Some(on_load.unchecked_ref()));
    iframe.set_src(&url);
    Ok(())
}

fn mime_for(name: &str) -> &'static str {
    match name.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("pdf") => "application/pdf",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("pptx") => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        Some("odt") => "application/vnd.oasis.opendocument.text",
        Some("rtf") => "application/rtf",
        Some("html") => "text/html",
        Some("csv") => "text/csv",
        Some("md" | "txt") => "text/plain",
        Some("tex") => "application/x-tex",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}
