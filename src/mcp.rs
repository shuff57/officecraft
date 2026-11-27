//! OfficeCraft as an MCP server (desktop), so Claude Code can work in the open documents live.
//!
//! Streamable HTTP on the loopback interface only (`http://127.0.0.1:<port>/mcp`), one JSON
//! reply per POST (no event stream). Every request carries a bearer token: a fresh one for each
//! Claude Code run the assistant starts, or the saved one (`mcp-token` in the config folder) for
//! sessions the user connects themselves. Claude Code on another computer reaches the server
//! through an SSH reverse tunnel (`ssh -R <port>:127.0.0.1:<port> host claude …`), so nothing
//! listens on the network.
//!
//! The server threads never touch a document. A tool call is queued for the window
//! (`take_calls`), which runs it between frames with the same control surface the assistant
//! uses and answers on the call's channel; PDF tabs answer a frame or two later, and the
//! server thread waits for that answer.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The first port tried; the next few are tried when it is taken.
pub const BASE_PORT: u16 = 7990;
/// Ports tried after the first.
const PORT_TRIES: u16 = 20;
/// The largest request body read.
const MAX_BODY: usize = 8 * 1024 * 1024;
/// The largest request line plus headers read.
const MAX_HEAD: usize = 16 * 1024;
/// Connections served at once; more are turned away.
const MAX_CONNECTIONS: usize = 32;
/// How long a tool call may take in the window before the client is told it timed out.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// Protocol versions this server speaks, newest first.
const VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// What the window answers a tool call with.
pub enum Reply {
    Now(Result<Value, String>),
    /// The app answers on a later frame (PDF).
    Later(Receiver<Result<Value, String>>),
}

/// A tool call waiting for the window.
pub struct Call {
    pub tool: String,
    pub args: Value,
    pub reply: Sender<Reply>,
    /// The token the request came with: a call whose run has ended (token revoked) is not run.
    token: String,
    /// When the client stops waiting: a call still queued then is dropped, never run late.
    deadline: Instant,
}

struct Server {
    port: u16,
    calls: Mutex<Vec<Call>>,
    tokens: Mutex<HashSet<String>>,
    /// The saved token, while Settings lets self-started sessions in.
    saved: Mutex<Option<String>>,
    connections: AtomicUsize,
    ctx: egui::Context,
}

static SERVER: OnceLock<Server> = OnceLock::new();

/// Where the server listens, once started.
pub fn port() -> Option<u16> {
    SERVER.get().map(|s| s.port)
}

/// The server's URL for a client on this computer (or at the far end of the tunnel).
pub fn url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

/// Starts the server (once; later calls return the port it has). An error when no port in
/// [`BASE_PORT`]`..+20` could be bound.
pub fn start(ctx: &egui::Context) -> Result<u16, String> {
    if let Some(s) = SERVER.get() {
        return Ok(s.port);
    }
    let mut last = String::new();
    for port in BASE_PORT..BASE_PORT + PORT_TRIES {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                let server = Server { port, calls: Mutex::new(Vec::new()), tokens: Mutex::new(HashSet::new()), saved: Mutex::new(None), connections: AtomicUsize::new(0), ctx: ctx.clone() };
                if SERVER.set(server).is_err() {
                    // Another frame started it first.
                    return SERVER.get().map(|s| s.port).ok_or_else(|| "the server did not start".to_string());
                }
                std::thread::Builder::new()
                    .name("officecraft-mcp".into())
                    .spawn(move || {
                        for stream in listener.incoming().flatten() {
                            let Some(s) = SERVER.get() else { break };
                            if s.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                                s.connections.fetch_sub(1, Ordering::SeqCst);
                                respond(stream, "503 Service Unavailable", "");
                                continue;
                            }
                            let spawned = std::thread::Builder::new().name("officecraft-mcp-conn".into()).spawn(move || {
                                serve(stream);
                                if let Some(s) = SERVER.get() {
                                    s.connections.fetch_sub(1, Ordering::SeqCst);
                                }
                            });
                            if spawned.is_err() {
                                s.connections.fetch_sub(1, Ordering::SeqCst);
                            }
                        }
                    })
                    .map_err(|e| e.to_string())?;
                log::info!("MCP server on {}", url(port));
                return Ok(port);
            }
            Err(e) => last = e.to_string(),
        }
    }
    Err(format!("no free port from {BASE_PORT}: {last}"))
}

/// Tool calls waiting for the window, oldest first. Calls whose client gave up waiting are
/// dropped, and calls from a run that has ended are refused, so nothing runs late.
pub fn take_calls() -> Vec<Call> {
    let Some(s) = SERVER.get() else { return Vec::new() };
    let calls = std::mem::take(&mut *s.calls.lock().unwrap_or_else(|e| e.into_inner()));
    let now = Instant::now();
    let live = s.tokens.lock().unwrap_or_else(|e| e.into_inner()).clone();
    calls
        .into_iter()
        .filter(|c| {
            if now >= c.deadline {
                return false;
            }
            if !live.contains(&c.token) {
                let _ = c.reply.send(Reply::Now(Err("this session has ended".into())));
                return false;
            }
            true
        })
        .collect()
}

/// Lets sessions the user starts themselves in with the saved token (`on`), or stops accepting
/// it. The server itself stays up; with no token it serves no one.
pub fn accept_saved(on: bool) {
    let Some(s) = SERVER.get() else { return };
    let mut saved = s.saved.lock().unwrap_or_else(|e| e.into_inner());
    let mut set = s.tokens.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(old) = saved.take() {
        set.remove(&old);
    }
    if on && let Some(t) = saved_token() {
        set.insert(t.clone());
        *saved = Some(t);
    }
}

/// A token good until [`revoke`]: one per Claude Code run.
pub fn issue_token() -> Result<String, String> {
    let t = random_token()?;
    if let Some(s) = SERVER.get() {
        s.tokens.lock().unwrap_or_else(|e| e.into_inner()).insert(t.clone());
    }
    Ok(t)
}

pub fn revoke(token: &str) {
    if let Some(s) = SERVER.get() {
        s.tokens.lock().unwrap_or_else(|e| e.into_inner()).remove(token);
    }
}

/// The saved token for sessions the user connects themselves, made on first use.
pub fn saved_token() -> Option<String> {
    if let Some(t) = crate::platform::read("mcp-token").map(|t| t.trim().to_string()).filter(|t| t.len() >= 32) {
        return Some(t);
    }
    let t = random_token().ok()?;
    crate::platform::write_private("mcp-token", &t).ok()?;
    Some(t)
}

/// A new saved token; the old one stops working at once.
pub fn new_saved_token() -> Result<String, String> {
    let t = random_token()?;
    crate::platform::write_private("mcp-token", &t)?;
    if let Some(s) = SERVER.get() {
        let mut saved = s.saved.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = saved.take() {
            let mut set = s.tokens.lock().unwrap_or_else(|e| e.into_inner());
            set.remove(&old);
            set.insert(t.clone());
            *saved = Some(t.clone());
        }
    }
    Ok(t)
}

/// A port from 20000 to 49999, for the far end of an ssh tunnel (two runs at once must not ask
/// for the same one).
pub fn random_port() -> u16 {
    let mut b = [0u8; 2];
    if getrandom::fill(&mut b).is_err() {
        // No random source: the clock still keeps two runs apart.
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        b = (n as u16 ^ (n >> 16) as u16).to_le_bytes();
    }
    20000 + u16::from_le_bytes(b) % 30000
}

/// An app command an outside agent may not run through `use_tool`: files are written only by
/// `save` and `send_to`, which refuse to replace a file. Reading file information is allowed.
/// `params` are the command's own: WordCraft's `mailings.finish` writes its merged document to a
/// `path` when given one (the only writer outside `file.` in the apps' engines; the others write
/// to fixed places such as the spelling dictionary or recovery copies).
/// `edit.repeat` and running a recorded macro are refused too: either can replay a save the user
/// made, to the user's path.
pub fn writes_files(command: &str, params: Option<&Value>) -> bool {
    (command.starts_with("file.") && !matches!(command, "file.info" | "file.render" | "file.strictReport"))
        || (command == "mailings.finish" && params.and_then(|p| p.get("path")).is_some_and(|p| !p.is_null()))
        || command == "edit.repeat"
        || (command == "tools.macros" && params.and_then(|p| p.get("run")).is_some_and(|r| !r.is_null()))
}

/// The first command named anywhere in a tool's arguments (`command`, a `batch` list's
/// `command`s, or an `id`) that [`writes_files`].
pub fn file_command_in(v: &Value) -> Option<String> {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if matches!(k.as_str(), "command" | "id" | "cmd")
                    && let Some(c) = x.as_str()
                    && writes_files(c.trim(), m.get("params"))
                {
                    return Some(c.trim().to_string());
                }
                if let Some(c) = file_command_in(x) {
                    return Some(c);
                }
            }
            None
        }
        Value::Array(a) => a.iter().find_map(file_command_in),
        _ => None,
    }
}

fn random_token() -> Result<String, String> {
    let mut b = [0u8; 24];
    getrandom::fill(&mut b).map_err(|e| e.to_string())?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

// ---- HTTP ----

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// The request line and headers, at most [`MAX_HEAD`] bytes, read within 30 s in all.
fn read_head(r: &mut BufReader<&TcpStream>) -> Result<Request, String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut budget = MAX_HEAD;
    // One line, chunk by chunk: the socket's timeout is re-armed with what is left of the 30 s
    // before every read, so trickling bytes can't hold a connection past the deadline.
    let mut next_line = |r: &mut BufReader<&TcpStream>| -> Result<String, String> {
        let mut buf = Vec::new();
        loop {
            if budget == 0 {
                return Err("request head too large".into());
            }
            let left = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()).ok_or("too slow")?;
            r.get_ref().set_read_timeout(Some(left)).map_err(|e| e.to_string())?;
            let chunk = r.fill_buf().map_err(|e| e.to_string())?;
            if chunk.is_empty() {
                break;
            }
            let take = chunk.iter().position(|b| *b == b'\n').map(|i| i + 1).unwrap_or(chunk.len()).min(budget);
            buf.extend_from_slice(&chunk[..take]);
            r.consume(take);
            budget -= take;
            if buf.ends_with(b"\n") {
                break;
            }
        }
        Ok(String::from_utf8_lossy(&buf).into_owned())
    };
    let line = next_line(r)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut headers = Vec::new();
    loop {
        let h = next_line(r)?;
        if h.is_empty() {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if headers.len() > 100 {
            return Err("too many headers".into());
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Ok(Request { method, path, headers, body: Vec::new() })
}

/// The body the head announced, at most [`MAX_BODY`] bytes, all of it within 30 s (the socket's
/// timeout is re-armed with what is left before every read, as for the head).
fn read_body(r: &mut BufReader<&TcpStream>, req: &mut Request) -> Result<(), String> {
    let len = req.header("content-length").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return Err("request too large".into());
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut body = Vec::with_capacity(len.min(64 * 1024));
    while body.len() < len {
        let left = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()).ok_or("too slow")?;
        r.get_ref().set_read_timeout(Some(left)).map_err(|e| e.to_string())?;
        let chunk = r.fill_buf().map_err(|e| e.to_string())?;
        if chunk.is_empty() {
            return Err("short body".into());
        }
        let take = chunk.len().min(len - body.len());
        body.extend_from_slice(&chunk[..take]);
        r.consume(take);
    }
    req.body = body;
    Ok(())
}

fn respond(mut stream: TcpStream, status: &str, body: &str) {
    let ctype = if body.is_empty() { "" } else { "Content-Type: application/json\r\n" };
    let head = format!("HTTP/1.1 {status}\r\n{ctype}Content-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n", body.len());
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

/// A browser page must never drive the documents (DNS rebinding): only clients that send no
/// Origin, or a loopback one, are served.
fn origin_ok(origin: Option<&str>) -> bool {
    let Some(o) = origin else { return true };
    let host = o.split_once("://").map(|(_, h)| h).unwrap_or(o);
    let host = host.split('/').next().unwrap_or("");
    let name = if host.starts_with('[') { host.split(']').next().map(|h| format!("{h}]")).unwrap_or_default() } else { host.split(':').next().unwrap_or("").to_string() };
    matches!(name.as_str(), "127.0.0.1" | "localhost" | "[::1]")
}

/// The accepted token the request carries, if any.
fn authorized(s: &Server, auth: Option<&str>) -> Option<String> {
    let t = auth.and_then(|a| a.strip_prefix("Bearer ").or_else(|| a.strip_prefix("bearer ")))?.trim();
    let set = s.tokens.lock().unwrap_or_else(|e| e.into_inner());
    // Every token is compared in full, so the time taken says nothing about which one matched.
    let mut found = None;
    for k in set.iter() {
        if constant_eq(k.as_bytes(), t.as_bytes()) {
            found = Some(k.clone());
        }
    }
    found
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn serve(stream: TcpStream) {
    let Some(server) = SERVER.get() else { return };
    if stream.set_read_timeout(Some(Duration::from_secs(30))).is_err() {
        return;
    }
    let mut r = BufReader::new(&stream);
    // Path, Origin and token are checked before any body is read.
    let mut req = match read_head(&mut r) {
        Ok(q) => q,
        Err(_) => {
            drop(r);
            return respond(stream, "400 Bad Request", "");
        }
    };
    if req.path.split('?').next().map(|p| p.trim_end_matches('/')) != Some("/mcp") {
        return respond(stream, "404 Not Found", "");
    }
    if !origin_ok(req.header("origin")) {
        return respond(stream, "403 Forbidden", "");
    }
    let Some(token) = authorized(server, req.header("authorization")) else {
        drop(r);
        return respond(stream, "401 Unauthorized", r#"{"error":"a valid bearer token is required"}"#);
    };
    match req.method.as_str() {
        "POST" => {}
        // No server-to-client stream, and no session to end.
        "DELETE" => {
            drop(r);
            return respond(stream, "200 OK", "");
        }
        _ => {
            drop(r);
            return respond(stream, "405 Method Not Allowed", "");
        }
    }
    // The body gets its own 30 s in all, not what the head left over (`read_body`).
    let body_read = read_body(&mut r, &mut req);
    drop(r);
    if body_read.is_err() {
        return respond(stream, "400 Bad Request", "");
    }
    let Ok(msg) = serde_json::from_slice::<Value>(&req.body) else {
        return respond(stream, "400 Bad Request", &rpc_error(Value::Null, -32700, "parse error").to_string());
    };
    let replies: Vec<Value> = match &msg {
        Value::Array(batch) => batch.iter().filter_map(|m| handle(server, m, &token)).collect(),
        m => handle(server, m, &token).into_iter().collect(),
    };
    match (msg.is_array(), replies.len()) {
        (_, 0) => respond(stream, "202 Accepted", ""),
        (false, _) => respond(stream, "200 OK", &replies[0].to_string()),
        (true, _) => respond(stream, "200 OK", &Value::Array(replies).to_string()),
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// One JSON-RPC message; `None` for a notification.
fn handle(server: &Server, m: &Value, token: &str) -> Option<Value> {
    let id = m.get("id").cloned()?;
    let method = m.get("method").and_then(Value::as_str).unwrap_or("");
    let params = m.get("params").cloned().unwrap_or(Value::Null);
    let ok = |result: Value| Some(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(VERSIONS[0]);
            let version = VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(VERSIONS[0]);
            ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "officecraft", "title": "OfficeCraft", "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({"tools": tool_definitions()})),
        "tools/call" => {
            let tool = params.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            if !tool_definitions().iter().any(|d| d["name"] == tool.as_str()) {
                return Some(rpc_error(id, -32602, &format!("unknown tool {tool}")));
            }
            let (text, is_error) = match call_window(server, tool, args, token) {
                Ok(v) => (match v {
                    Value::String(s) => s,
                    other => other.to_string(),
                }, false),
                Err(e) => (e, true),
            };
            ok(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
        }
        _ => Some(rpc_error(id, -32601, &format!("method not found: {method}"))),
    }
}

/// Hands a tool call to the window and waits for its answer.
fn call_window(server: &Server, tool: String, args: Value, token: &str) -> Result<Value, String> {
    let (tx, rx) = channel();
    let call = Call { tool, args, reply: tx, token: token.to_string(), deadline: Instant::now() + CALL_TIMEOUT };
    server.calls.lock().unwrap_or_else(|e| e.into_inner()).push(call);
    server.ctx.request_repaint();
    match rx.recv_timeout(CALL_TIMEOUT) {
        Ok(Reply::Now(r)) => r,
        Ok(Reply::Later(later)) => later.recv_timeout(CALL_TIMEOUT).map_err(|_| "the app did not answer in time".to_string())?,
        Err(_) => Err("OfficeCraft did not answer in time (is its window open?)".into()),
    }
}

const INSTRUCTIONS: &str = "OfficeCraft is an office suite open on the user's screen: Docs (WordCraft), Sheets \
(GridCraft), Slides (DeckCraft), PDF, Graph (viSHual). Each open document is a tab. Start with list_tabs, then \
app_tools for the tab you work on, and make changes with use_tool: the user sees every change as it happens. \
Read before you write (an inspect tool), change only what was asked, and save only when asked.";

/// The tools, as MCP lists them.
pub fn tool_definitions() -> Vec<Value> {
    let tab = json!({"type": "integer", "description": "the tab's id, from list_tabs"});
    vec![
        json!({"name": "list_tabs", "description": "The documents open in OfficeCraft: each tab's id, app, name, file path, whether it has unsaved changes, and which one the user is looking at.",
               "inputSchema": {"type": "object", "properties": {}}}),
        json!({"name": "app_tools", "description": "The editing tools of the app in a tab (WordCraft, GridCraft, DeckCraft, PdfCraft or viSHual), with their parameters. Call it once per app before use_tool.",
               "inputSchema": {"type": "object", "properties": {"tab": tab}, "required": ["tab"]}}),
        json!({"name": "use_tool", "description": "Run one of a tab's app tools (see app_tools) on that tab's document. Changes appear on the user's screen at once.",
               "inputSchema": {"type": "object", "properties": {"tab": tab, "tool": {"type": "string"}, "arguments": {"type": "object"}}, "required": ["tab", "tool"]}}),
        json!({"name": "new_tab", "description": "Open a new, empty document in a new tab and show it. Returns its tab id.",
               "inputSchema": {"type": "object", "properties": {"app": {"type": "string", "enum": ["docs", "sheets", "slides", "graph"]}, "name": {"type": "string", "description": "the tab's name (optional)"}}, "required": ["app"]}}),
        json!({"name": "open_file", "description": "Open a file (.docx, .xlsx, .pptx, .pdf, .odt, .csv and the like) on the computer OfficeCraft runs on, in its own tab. Returns its tab id.",
               "inputSchema": {"type": "object", "properties": {"path": {"type": "string", "description": "absolute path"}}, "required": ["path"]}}),
        json!({"name": "show_tab", "description": "Bring a tab to the front for the user.",
               "inputSchema": {"type": "object", "properties": {"tab": tab}, "required": ["tab"]}}),
        json!({"name": "send_to", "description": "Convert a tab's document with OfficeCraft's Send to: docs → slides (from the outline), slides → docs, sheets → docs or slides (as a table; with `linked` the table keeps following the sheet while it is open), graph → docs or slides (as a picture). Into a new tab, or into an open tab with `into`. Or to a PDF file with `pdf_path`, opened in a PDF tab.",
               "inputSchema": {"type": "object", "properties": {"tab": tab, "to": {"type": "string", "enum": ["docs", "slides", "pdf"]}, "into": {"type": "integer", "description": "an open tab to add to, instead of a new one"}, "pdf_path": {"type": "string", "description": "for to=pdf: absolute path of the PDF to write"}, "linked": {"type": "boolean", "description": "sheets only: keep the table up to date with the sheet"}, "overwrite": {"type": "boolean", "description": "replace an existing file at pdf_path (only when the user asked)"}}, "required": ["tab", "to"]}}),
        json!({"name": "save", "description": "Save a tab's document: to its own file, or to `path` (Save As; the extension picks the format, e.g. .docx, .xlsx, .pptx). An unsaved new document needs a path. An existing file at `path` is not replaced unless `overwrite` is true.",
               "inputSchema": {"type": "object", "properties": {"tab": tab, "path": {"type": "string"}, "overwrite": {"type": "boolean", "description": "replace an existing file at path (only when the user asked)"}}, "required": ["tab"]}}),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_origins_are_served() {
        assert!(origin_ok(None));
        assert!(origin_ok(Some("http://127.0.0.1:7990")));
        assert!(origin_ok(Some("http://localhost")));
        assert!(origin_ok(Some("http://[::1]:7990")));
        assert!(!origin_ok(Some("https://evil.example")));
        assert!(!origin_ok(Some("http://127.0.0.1.evil.example")));
        assert!(!origin_ok(Some("null")));
    }

    #[test]
    fn file_writing_commands_are_found_anywhere_in_the_arguments() {
        assert_eq!(file_command_in(&json!({"command": "file.saveAs", "params": {"path": "/x"}})).as_deref(), Some("file.saveAs"));
        assert_eq!(file_command_in(&json!({"commands": [{"command": "text.insert"}, {"command": " file.exportPdf"}]})).as_deref(), Some("file.exportPdf"));
        assert_eq!(file_command_in(&json!({"id": "file.save"})).as_deref(), Some("file.save"));
        assert!(file_command_in(&json!({"command": "file.info"})).is_none());
        assert!(file_command_in(&json!({"command": "text.insert", "params": {"text": "file.save"}})).is_none(), "text that mentions a command is not a command");
        assert!(file_command_in(&json!({"blocks": [{"text": "a"}]})).is_none());
        // Mail merge writes a file only when given a path.
        assert_eq!(file_command_in(&json!({"command": "mailings.finish", "params": {"path": "/home/u/thesis.docx"}})).as_deref(), Some("mailings.finish"));
        assert_eq!(file_command_in(&json!({"commands": [{"command": "mailings.finish", "params": {"path": "/x.docx"}}]})).as_deref(), Some("mailings.finish"));
        assert!(file_command_in(&json!({"command": "mailings.finish", "params": {}})).is_none());
        assert!(file_command_in(&json!({"command": "mailings.finish", "params": {"path": null}})).is_none());
        // Replaying a save the user made.
        assert_eq!(file_command_in(&json!({"command": "edit.repeat"})).as_deref(), Some("edit.repeat"));
        assert_eq!(file_command_in(&json!({"command": "tools.macros", "params": {"run": "Save it"}})).as_deref(), Some("tools.macros"));
        assert!(file_command_in(&json!({"command": "tools.macros", "params": {}})).is_none());
    }

    #[test]
    fn tokens_compare_in_full() {
        assert!(constant_eq(b"abc", b"abc"));
        assert!(!constant_eq(b"abc", b"abd"));
        assert!(!constant_eq(b"abc", b"ab"));
    }

    #[test]
    fn tokens_are_long_and_random() {
        let a = random_token().unwrap();
        let b = random_token().unwrap();
        assert_eq!(a.len(), 48);
        assert_ne!(a, b);
    }

    #[test]
    fn every_tool_has_a_schema() {
        for d in tool_definitions() {
            assert!(d["name"].is_string() && d["description"].is_string(), "{d}");
            assert_eq!(d["inputSchema"]["type"], "object", "{d}");
        }
    }

    #[test]
    fn notifications_get_no_reply_and_unknown_methods_an_error() {
        let ctx = egui::Context::default();
        let s = Server { port: 0, calls: Mutex::new(Vec::new()), tokens: Mutex::new(HashSet::new()), saved: Mutex::new(None), connections: AtomicUsize::new(0), ctx };
        let handle = |s: &Server, m: &Value| handle(s, m, "t");
        assert!(handle(&s, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        let r = handle(&s, &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}})).unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["serverInfo"]["name"], "officecraft");
        let r = handle(&s, &json!({"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {"protocolVersion": "1999-01-01"}})).unwrap();
        assert_eq!(r["result"]["protocolVersion"], VERSIONS[0]);
        let r = handle(&s, &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"})).unwrap();
        assert!(r["result"]["tools"].as_array().unwrap().len() >= 6);
        let r = handle(&s, &json!({"jsonrpc": "2.0", "id": 4, "method": "nope"})).unwrap();
        assert_eq!(r["error"]["code"], -32601);
        let r = handle(&s, &json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "rm_rf"}})).unwrap();
        assert_eq!(r["error"]["code"], -32602);
    }

    #[test]
    fn a_bearer_token_from_the_set_is_required() {
        let ctx = egui::Context::default();
        let s = Server { port: 0, calls: Mutex::new(Vec::new()), tokens: Mutex::new(HashSet::from(["t0ken".to_string()])), saved: Mutex::new(None), connections: AtomicUsize::new(0), ctx };
        assert_eq!(authorized(&s, Some("Bearer t0ken")).as_deref(), Some("t0ken"));
        assert!(authorized(&s, Some("Bearer t0ke")).is_none());
        assert!(authorized(&s, Some("t0ken")).is_none());
        assert!(authorized(&s, None).is_none());
    }

    #[test]
    fn tunnel_ports_are_in_range() {
        for _ in 0..50 {
            assert!((20000..50000).contains(&random_port()));
        }
    }
}
