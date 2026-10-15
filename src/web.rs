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
/// The loading animation in index.html runs a 3.5 s cycle; the app is revealed on a cycle boundary.
const LOADER_CYCLE_MS: f64 = 3500.0;
const LOADER_FADE_MS: i32 = 350;

const DOC_EXTS: &[&str] = &["docx", "docm", "dotx", "dotm", "doc", "dot", "odt", "rtf", "txt", "md", "html", "htm", "tex", "json"];
const SHEET_EXTS: &[&str] = &["xlsx", "xlsm", "xls", "xlsb", "ods", "csv", "tsv", "txt", "json"];
const PRESENTATION_EXTS: &[&str] = &["deckcraft", "pptx", "potx", "ppsx"];
const PICTURE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];
const AUDIO_EXTS: &[&str] = &["mp3", "wav", "ogg", "opus", "m4a", "flac"];
const VIDEO_EXTS: &[&str] = &["mp4", "webm", "mkv", "mov", "m4v"];
const RECIPIENT_EXTS: &[&str] = &["csv", "tsv", "txt"];

thread_local! {
    /// Handles of files opened through the File System Access picker, by file name, so Save can
    /// write back to the same file instead of downloading a copy.
    /// `None` marks a name opened from two different files: Save downloads rather than guess.
    static HANDLES: RefCell<std::collections::HashMap<String, Option<wasm_bindgen::JsValue>>> = RefCell::new(std::collections::HashMap::new());
    /// Folders the visitor added, by the key shown in the sidebar, as directory handles.
    static DIRS: RefCell<std::collections::HashMap<String, wasm_bindgen::JsValue>> = RefCell::new(std::collections::HashMap::new());
    /// Folders picked through a plain `<input webkitdirectory>` (browsers without the directory
    /// picker, e.g. Brave): the `File`s of that moment's snapshot by relative path, read-only.
    static DIRFILES: RefCell<std::collections::HashMap<String, std::collections::HashMap<String, wasm_bindgen::JsValue>>> = RefCell::new(std::collections::HashMap::new());
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
        // The Graph tab (viSHual) draws through its own WebGL2 renderer, which has to exist before
        // the first frame: without this the tab says "the graph renderer is unavailable".
        if let Err(e) = vishual_ui_egui::init_web_gpu().await {
            log::error!("graph renderer: {e}");
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
                Ok(()) => {
                    finish_loader_cycle().await;
                    el.remove();
                }
                Err(e) => el.set_inner_html(&format!("<p>OfficeCraft failed to start: {e:?}</p><p>A browser with WebGPU or WebGL2 is required.</p>")),
            }
        }
    });
}

/// Holds the loading screen until its animation reaches the end of the cycle it is in, then fades
/// it out, so the icon always finishes a full pass before the app appears. Skipped when the
/// visitor prefers reduced motion (the icon is then a still image).
async fn finish_loader_cycle() {
    let Some(window) = web_sys::window() else { return };
    let reduced = window.match_media("(prefers-reduced-motion: reduce)").ok().flatten().is_some_and(|m| m.matches());
    if reduced {
        return;
    }
    // `performance.now()` shares its origin with the document timeline the CSS animation runs on.
    let now = window.performance().map_or(0.0, |p| p.now());
    let remaining = LOADER_CYCLE_MS - now % LOADER_CYCLE_MS;
    sleep_ms(&window, remaining as i32).await;
    if let Some(el) = window.document().and_then(|d| d.get_element_by_id(LOADING_ID)).and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
        let _ = el.style().set_property("opacity", "0");
        sleep_ms(&window, LOADER_FADE_MS).await;
    }
}

async fn sleep_ms(window: &web_sys::Window, ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms.max(0));
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
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
    if can_write_in_place() {
        pick_doc("Office files", exts, true, inbox);
        return;
    }
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

type Js = wasm_bindgen::JsValue;

fn js_err(e: Js) -> String {
    format!("{e:?}")
}

/// Whether the browser offers the File System Access API (Chromium; needs a secure context such
/// as https or localhost). Without it, Open goes through a plain file input and Save downloads.
pub fn can_write_in_place() -> bool {
    web_sys::window().is_some_and(|w| js_sys::Reflect::get(&w, &"showOpenFilePicker".into()).is_ok_and(|f| f.is_function()))
}

fn call(obj: &Js, method: &str, args: &[Js]) -> Result<Js, String> {
    let f: js_sys::Function = js_sys::Reflect::get(obj, &method.into()).map_err(js_err)?.dyn_into().map_err(|_| format!("{method} is not a function"))?;
    let list = js_sys::Array::new();
    for a in args {
        list.push(a);
    }
    f.apply(obj, &list).map_err(js_err)
}

async fn await_js(v: Js) -> Result<Js, String> {
    wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(&v)).await.map_err(js_err)
}

fn set_prop(obj: &js_sys::Object, key: &str, value: &Js) -> Result<(), String> {
    js_sys::Reflect::set(obj, &key.into(), value).map(|_| ()).map_err(js_err)
}

/// Shows the File System Access picker and returns each chosen file's name and bytes, keeping
/// the handles so a later Save can write back. A cancelled picker returns an empty list.
async fn pick_via_handles(description: &str, exts: &[&str], multiple: bool) -> Result<Vec<(String, Vec<u8>)>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let accept = js_sys::Object::new();
    for ext in exts {
        let key: Js = mime_for(&format!("file.{ext}")).into();
        let list = match js_sys::Reflect::get(&accept, &key) {
            Ok(v) if v.is_instance_of::<js_sys::Array>() => v.unchecked_into::<js_sys::Array>(),
            _ => {
                let a = js_sys::Array::new();
                js_sys::Reflect::set(&accept, &key, &a).map_err(js_err)?;
                a
            }
        };
        list.push(&Js::from_str(&format!(".{ext}")));
    }
    let kind = js_sys::Object::new();
    set_prop(&kind, "description", &Js::from_str(description))?;
    set_prop(&kind, "accept", &accept)?;
    let opts = js_sys::Object::new();
    set_prop(&opts, "multiple", &Js::from_bool(multiple))?;
    set_prop(&opts, "types", &js_sys::Array::of1(&kind))?;
    let handles = match await_js(call(&window, "showOpenFilePicker", &[opts.into()])?).await {
        Ok(h) => h,
        Err(e) if e.contains("AbortError") => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for handle in js_sys::Array::from(&handles).iter() {
        let name = js_sys::Reflect::get(&handle, &"name".into()).ok().and_then(|n| n.as_string()).unwrap_or_default();
        let file = await_js(call(&handle, "getFile", &[])?).await?;
        let buf = await_js(call(&file, "arrayBuffer", &[])?).await?;
        out.push((name.clone(), js_sys::Uint8Array::new(&buf).to_vec()));
        remember_handle(name, handle).await;
    }
    Ok(out)
}

/// Records `handle` as the file Save should write back to for `name`. A name that now maps to two
/// different files is marked ambiguous (Save downloads) so the wrong file is never overwritten.
async fn remember_handle(name: String, handle: Js) {
    let existing = HANDLES.with(|m| m.borrow().get(&name).cloned());
    let value = match existing {
        None => Some(handle),
        Some(None) => None,
        Some(Some(old)) => {
            let same = match call(&old, "isSameEntry", &[handle.clone()]) {
                Ok(p) => await_js(p).await.ok().and_then(|v| v.as_bool()).unwrap_or(false),
                Err(_) => false,
            };
            if same { Some(handle) } else { None }
        }
    };
    HANDLES.with(|m| m.borrow_mut().insert(name, value));
}

/// Folder browsing works in every browser: live handles where `showDirectoryPicker` exists,
/// otherwise a snapshot picked through a directory file input (Save then downloads a copy).
pub fn can_browse_folders() -> bool {
    web_sys::window().is_some()
}

fn has_directory_picker() -> bool {
    web_sys::window().is_some_and(|w| js_sys::Reflect::get(&w, &"showDirectoryPicker".into()).is_ok_and(|f| f.is_function()))
}

const DIR_INPUT_ID: &str = "officecraft-dir-input";

/// The fallback folder picker: a hidden `<input type=file webkitdirectory>`. Its `File`s are kept
/// by relative path and listed like a scanned folder; picking the same folder again refreshes it.
fn add_folder_from_input(scans: crate::FolderScans) -> Result<(), String> {
    let document = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    if let Some(old) = document.get_element_by_id(DIR_INPUT_ID) {
        old.remove();
    }
    let input = document.create_element("input").map_err(js_err)?;
    input.set_id(DIR_INPUT_ID);
    for (k, v) in [("type", "file"), ("webkitdirectory", ""), ("multiple", ""), ("style", "display:none")] {
        input.set_attribute(k, v).map_err(js_err)?;
    }
    document.body().ok_or("no body")?.append_child(&input).map_err(js_err)?;
    let target = input.clone();
    let on_change = wasm_bindgen::closure::Closure::once_into_js(move || {
        let files = js_sys::Reflect::get(&target, &"files".into()).unwrap_or(Js::UNDEFINED);
        let count = prop_f64(&files, "length") as u32;
        let mut root = String::new();
        let mut by_rel = std::collections::HashMap::new();
        let mut rows: Vec<crate::Row> = Vec::new();
        for i in 0..count {
            let Ok(file) = call(&files, "item", &[Js::from_f64(f64::from(i))]) else { continue };
            let rel_path = prop_string(&file, "webkitRelativePath");
            let parts: Vec<&str> = rel_path.split('/').collect();
            // "Root/sub/file.ext": two folders below the root at most, nothing hidden or vendored.
            if parts.len() < 2 || parts.len() > 4 || parts[1..].iter().any(|p| p.starts_with('.') || *p == "node_modules") {
                continue;
            }
            root = parts[0].to_string();
            let name = parts[parts.len() - 1].to_string();
            if crate::kind_for(&name).is_none() {
                continue;
            }
            let rel = parts[1..].join("/");
            let modified = std::time::UNIX_EPOCH + std::time::Duration::from_millis(prop_f64(&file, "lastModified") as u64);
            rows.push(crate::Row {
                path: format!("{root}/{rel}"),
                kind: crate::kind_for(&name),
                folder: if parts.len() > 2 { parts[parts.len() - 2].to_string() } else { root.clone() },
                name,
                modified: Some(modified),
                size: prop_f64(&file, "size") as u64,
            });
            by_rel.insert(rel, file);
        }
        target.remove();
        if root.is_empty() {
            return;
        }
        rows.sort_by(|a, b| b.modified.cmp(&a.modified));
        rows.truncate(500);
        DIRFILES.with(|m| m.borrow_mut().insert(root.clone(), by_rel));
        scans.lock().unwrap_or_else(|e| e.into_inner()).push(crate::FolderScan { key: root, rows, added: true });
        repaint();
    });
    call(&input, "addEventListener", &[Js::from_str("change"), on_change])?;
    call(&input, "click", &[])?;
    Ok(())
}

fn prop_string(obj: &Js, key: &str) -> String {
    js_sys::Reflect::get(obj, &key.into()).ok().and_then(|v| v.as_string()).unwrap_or_default()
}

fn prop_f64(obj: &Js, key: &str) -> f64 {
    js_sys::Reflect::get(obj, &key.into()).ok().and_then(|v| v.as_f64()).unwrap_or(0.0)
}

/// Supported files in `root` and the folders two levels below it, newest first (the same shape as
/// the desktop listing). Paths are `key/relative/path`.
async fn scan_dir(root: &Js, key: &str) -> Result<Vec<crate::Row>, String> {
    const MAX_ROWS: usize = 500;
    let mut rows: Vec<crate::Row> = Vec::new();
    let mut stack = vec![(root.clone(), String::new(), key.to_string(), 0u8)];
    'walk: while let Some((dir, prefix, dir_name, depth)) = stack.pop() {
        let iter = call(&dir, "values", &[])?;
        loop {
            let next = await_js(call(&iter, "next", &[])?).await?;
            if js_sys::Reflect::get(&next, &"done".into()).ok().and_then(|d| d.as_bool()).unwrap_or(true) {
                break;
            }
            let entry = js_sys::Reflect::get(&next, &"value".into()).map_err(js_err)?;
            let name = prop_string(&entry, "name");
            if name.starts_with('.') {
                continue;
            }
            match prop_string(&entry, "kind").as_str() {
                "directory" if depth < 2 => stack.push((entry, format!("{prefix}{name}/"), name, depth + 1)),
                "file" if crate::kind_for(&name).is_some() => {
                    let file = await_js(call(&entry, "getFile", &[])?).await?;
                    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_millis(prop_f64(&file, "lastModified") as u64);
                    rows.push(crate::Row {
                        path: format!("{key}/{prefix}{name}"),
                        kind: crate::kind_for(&name),
                        name,
                        folder: dir_name.clone(),
                        modified: Some(modified),
                        size: prop_f64(&file, "size") as u64,
                    });
                    if rows.len() >= MAX_ROWS {
                        break 'walk;
                    }
                }
                _ => {}
            }
        }
    }
    rows.sort_by(|a, b| b.modified.cmp(&a.modified));
    Ok(rows)
}

/// Shows the folder picker and queues the chosen folder's listing in `scans`.
pub fn add_folder(scans: crate::FolderScans) {
    if !has_directory_picker() {
        if let Err(e) = add_folder_from_input(scans) {
            log::error!("folder picker failed: {e}");
        }
        return;
    }
    wasm_bindgen_futures::spawn_local(async move {
        let result: Result<Option<(String, Vec<crate::Row>)>, String> = async {
            let window = web_sys::window().ok_or("no window")?;
            let handle = match await_js(call(&window, "showDirectoryPicker", &[])?).await {
                Ok(h) => h,
                Err(e) if e.contains("AbortError") => return Ok(None),
                Err(e) => return Err(e),
            };
            // Two different folders can share a name; the key must stay unique.
            let base = prop_string(&handle, "name");
            let mut key = base.clone();
            let mut n = 2;
            loop {
                let taken = DIRS.with(|m| m.borrow().get(&key).cloned());
                match taken {
                    None => break,
                    Some(old) => {
                        let same = match call(&old, "isSameEntry", &[handle.clone()]) {
                            Ok(p) => await_js(p).await.ok().and_then(|v| v.as_bool()).unwrap_or(false),
                            Err(_) => false,
                        };
                        if same {
                            break;
                        }
                        key = format!("{base} ({n})");
                        n += 1;
                    }
                }
            }
            DIRS.with(|m| m.borrow_mut().insert(key.clone(), handle.clone()));
            let rows = scan_dir(&handle, &key).await?;
            Ok(Some((key, rows)))
        }
        .await;
        match result {
            Ok(Some((key, rows))) => {
                scans.lock().unwrap_or_else(|e| e.into_inner()).push(crate::FolderScan { key, rows, added: true });
                repaint();
            }
            Ok(None) => {}
            Err(e) => log::error!("folder picker failed: {e}"),
        }
    });
}

/// Lists `key` again (files may have changed on disk) and queues the result.
pub fn rescan_folder(key: &str, scans: crate::FolderScans) {
    let Some(handle) = DIRS.with(|m| m.borrow().get(key).cloned()) else { return };
    let key = key.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        match scan_dir(&handle, &key).await {
            Ok(rows) => {
                scans.lock().unwrap_or_else(|e| e.into_inner()).push(crate::FolderScan { key, rows, added: false });
                repaint();
            }
            Err(e) => log::error!("couldn't list {key}: {e}"),
        }
    });
}

/// The directory key `path` (`key/rel/path`) belongs to, if it is one of the added folders.
fn folder_of(path: &str) -> Option<(String, String)> {
    let mut keys: Vec<String> = DIRS.with(|m| m.borrow().keys().cloned().collect());
    keys.extend(DIRFILES.with(|m| m.borrow().keys().cloned().collect::<Vec<_>>()));
    keys.iter()
        .filter_map(|k| path.strip_prefix(k.as_str()).and_then(|r| r.strip_prefix('/')).map(|rel| (k.clone(), rel.to_string())))
        .max_by_key(|(k, _)| k.len())
}

pub fn is_folder_path(path: &str) -> bool {
    folder_of(path).is_some()
}

/// Reads a file listed in an added folder and queues its bytes in `inbox`, keeping its handle so
/// Save writes back to it.
pub fn open_from_folder(path: &str, inbox: Inbox) {
    let Some((key, rel)) = folder_of(path) else { return };
    // A snapshot folder (no directory picker): the `File` itself, no handle to write back to.
    if let Some(file) = DIRFILES.with(|m| m.borrow().get(&key).and_then(|f| f.get(&rel).cloned())) {
        wasm_bindgen_futures::spawn_local(async move {
            let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
            let read = async { await_js(call(&file, "arrayBuffer", &[])?).await };
            match read.await {
                Ok(buf) => {
                    inbox.lock().unwrap_or_else(|e| e.into_inner()).push((name, js_sys::Uint8Array::new(&buf).to_vec()));
                    repaint();
                }
                Err(e) => log::error!("couldn't read {rel}: {e}"),
            }
        });
        return;
    }
    let Some(root) = DIRS.with(|m| m.borrow().get(&key).cloned()) else { return };
    wasm_bindgen_futures::spawn_local(async move {
        let result: Result<(), String> = async {
            let parts: Vec<&str> = rel.split('/').collect();
            let (file_name, dirs) = parts.split_last().ok_or("empty path")?;
            let mut dir = root;
            for d in dirs {
                dir = await_js(call(&dir, "getDirectoryHandle", &[Js::from_str(d)])?).await?;
            }
            let handle = await_js(call(&dir, "getFileHandle", &[Js::from_str(file_name)])?).await?;
            let file = await_js(call(&handle, "getFile", &[])?).await?;
            let buf = await_js(call(&file, "arrayBuffer", &[])?).await?;
            let name = prop_string(&handle, "name");
            inbox.lock().unwrap_or_else(|e| e.into_inner()).push((name.clone(), js_sys::Uint8Array::new(&buf).to_vec()));
            remember_handle(name, handle).await;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => repaint(),
            Err(e) => log::error!("couldn't open {rel}: {e}"),
        }
    });
}

/// Opens a document picker: the File System Access one where available (so Save can write back),
/// otherwise the ordinary file input.
fn pick_doc(description: &'static str, exts: &'static [&'static str], multiple: bool, inbox: Inbox) {
    if !can_write_in_place() {
        let dialog = rfd::AsyncFileDialog::new().add_filter(description, exts);
        pick_into(dialog, inbox);
        return;
    }
    wasm_bindgen_futures::spawn_local(async move {
        match pick_via_handles(description, exts, multiple).await {
            Ok(files) => {
                inbox.lock().unwrap_or_else(|e| e.into_inner()).extend(files);
                repaint();
            }
            Err(e) => log::error!("file picker failed: {e}"),
        }
    });
}

/// Save: writes back to the file the user opened through the picker when its handle is held,
/// otherwise (or if the write is refused) downloads a copy.
fn save_file(path: &str, bytes: &[u8]) -> Result<(), String> {
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let Some(handle) = HANDLES.with(|m| m.borrow().get(&name).cloned().flatten()) else {
        return download(path, bytes);
    };
    let (path, bytes) = (path.to_string(), bytes.to_vec());
    wasm_bindgen_futures::spawn_local(async move {
        match write_handle(&handle, &bytes).await {
            Ok(()) => log::info!("saved {name} in place"),
            Err(e) => {
                log::warn!("couldn't write {name} in place ({e}); downloading a copy");
                if let Err(e) = download(&path, &bytes) {
                    log::error!("download of {path} failed: {e}");
                }
            }
        }
    });
    Ok(())
}

async fn write_handle(handle: &Js, bytes: &[u8]) -> Result<(), String> {
    let writable = await_js(call(handle, "createWritable", &[])?).await?;
    await_js(call(&writable, "write", &[js_sys::Uint8Array::from(bytes).into()])?).await?;
    await_js(call(&writable, "close", &[])?).await?;
    Ok(())
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

/// A pick for DeckCraft, which numbers its requests: the file answers request `id`, and a dismissed
/// picker cancels it so a late or unknown completion is never applied to whatever is open.
fn pick_for_deck(dialog: rfd::AsyncFileDialog, inbox: deckcraft_ui_egui::Inbox, id: u64) {
    wasm_bindgen_futures::spawn_local(async move {
        let event = match dialog.pick_file().await {
            Some(file) => {
                let bytes = file.read().await;
                deckcraft_ui_egui::InboxEvent::File(deckcraft_ui_egui::InboxFile { id, name: file.file_name(), bytes })
            }
            None => deckcraft_ui_egui::InboxEvent::Cancelled { id },
        };
        inbox.lock().unwrap_or_else(|e| e.into_inner()).push(event);
        repaint();
    });
}

pub fn word_services() -> wordcraft_ui_egui::Services {
    let inbox = Inbox::default();
    let open_inbox = inbox.clone();
    wordcraft_ui_egui::Services {
        open_async: Some(Box::new(move |purpose: &str| {
            match purpose {
                "picture" => pick_into(rfd::AsyncFileDialog::new().add_filter("Pictures", PICTURE_EXTS), open_inbox.clone()),
                "recipients" => pick_into(rfd::AsyncFileDialog::new().add_filter("Recipient lists", RECIPIENT_EXTS), open_inbox.clone()),
                _ => pick_doc("Documents", DOC_EXTS, false, open_inbox.clone()),
            }
        })),
        // Exports ask for a name; the browser decides where the download goes.
        pick_save: Some(Box::new(|name: &str| Some(name.to_string()))),
        download: Some(Box::new(save_file)),
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
            pick_doc("Spreadsheets", SHEET_EXTS, false, open_inbox.clone());
        })),
        pick_picture_async: Some(Box::new(|inbox| {
            pick_into(rfd::AsyncFileDialog::new().add_filter("Pictures (PNG, JPEG)", &["png", "jpg", "jpeg"]), inbox);
        })),
        download: Some(Box::new(|name: &str, bytes: &[u8]| {
            if let Err(e) = save_file(name, bytes) {
                log::error!("save of {name} failed: {e}");
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
    let inbox = deckcraft_ui_egui::Inbox::default();
    let open_inbox = inbox.clone();
    deckcraft_ui_egui::Services {
        open_async: Some(Box::new(move |purpose: &str, id: u64| {
            match purpose {
                "picture" => pick_for_deck(rfd::AsyncFileDialog::new().add_filter("Pictures", PICTURE_EXTS), open_inbox.clone(), id),
                "audio" => pick_for_deck(rfd::AsyncFileDialog::new().add_filter("Audio", AUDIO_EXTS), open_inbox.clone(), id),
                "video" => pick_for_deck(rfd::AsyncFileDialog::new().add_filter("Video", VIDEO_EXTS), open_inbox.clone(), id),
                _ => pick_for_deck(rfd::AsyncFileDialog::new().add_filter("Presentations", PRESENTATION_EXTS), open_inbox.clone(), id),
            }
        })),
        write: Some(Box::new(|name: &str, bytes: &[u8]| save_file(name, bytes))),
        download: Some(Box::new(|name: &str, bytes: &[u8]| {
            if let Err(e) = save_file(name, bytes) {
                log::error!("save of {name} failed: {e}");
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
pub fn download(path: &str, bytes: &[u8]) -> Result<(), String> {
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
