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
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};

/// The first port tried; the next few are tried when it is taken.
pub const BASE_PORT: u16 = 7990;
/// Ports tried after the first.
const PORT_TRIES: u16 = 20;
/// The largest request body read.
const MAX_BODY: usize = 8 * 1024 * 1024;
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
}

struct Server {
    port: u16,
    calls: Mutex<Vec<Call>>,
    tokens: Mutex<HashSet<String>>,
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
                let mut tokens = HashSet::new();
                if let Some(t) = saved_token() {
                    tokens.insert(t);
                }
                if SERVER.set(Server { port, calls: Mutex::new(Vec::new()), tokens: Mutex::new(tokens), ctx: ctx.clone() }).is_err() {
                    // Another frame started it first.
                    return SERVER.get().map(|s| s.port).ok_or_else(|| "the server did not start".to_string());
                }
                std::thread::Builder::new()
                    .name("officecraft-mcp".into())
                    .spawn(move || {
                        for stream in listener.incoming().flatten() {
                            let _ = std::thread::Builder::new().name("officecraft-mcp-conn".into()).spawn(move || serve(stream));
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

/// Tool calls waiting for the window, oldest first.
pub fn take_calls() -> Vec<Call> {
    SERVER.get().map(|s| std::mem::take(&mut *s.calls.lock().unwrap_or_else(|e| e.into_inner()))).unwrap_or_default()
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
    let old = crate::platform::read("mcp-token").map(|t| t.trim().to_string());
    let t = random_token()?;
    crate::platform::write_private("mcp-token", &t)?;
    if let Some(s) = SERVER.get() {
        let mut set = s.tokens.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(o) = old {
            set.remove(&o);
        }
        set.insert(t.clone());
    }
    Ok(t)
}

fn random_token() -> Result<String, String> {
    let mut b = [0u8; 24];
    getrandom::fill(&mut b).map_err(|e| e.to_string())?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Claude Code's `--mcp-config` for this server with `token`.
pub fn client_config(port: u16, token: &str) -> Value {
    json!({"mcpServers": {"officecraft": {"type": "http", "url": url(port), "headers": {"Authorization": format!("Bearer {token}")}}}})
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

fn read_request(stream: &TcpStream) -> Result<Request, String> {
    stream.set_read_timeout(Some(Duration::from_secs(30))).map_err(|e| e.to_string())?;
    let mut r = BufReader::new(stream);
    let mut line = String::new();
    r.read_line(&mut line).map_err(|e| e.to_string())?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).map_err(|e| e.to_string())? == 0 {
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
    let len = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse::<usize>().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return Err("request too large".into());
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body).map_err(|e| e.to_string())?;
    Ok(Request { method, path, headers, body })
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

fn authorized(s: &Server, auth: Option<&str>) -> bool {
    let Some(t) = auth.and_then(|a| a.strip_prefix("Bearer ").or_else(|| a.strip_prefix("bearer "))) else { return false };
    let set = s.tokens.lock().unwrap_or_else(|e| e.into_inner());
    // Compared in full whatever matches first, so the time taken says nothing about the token.
    set.iter().fold(false, |found, k| found | constant_eq(k.as_bytes(), t.trim().as_bytes()))
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn serve(stream: TcpStream) {
    let Some(server) = SERVER.get() else { return };
    let req = match read_request(&stream) {
        Ok(r) => r,
        Err(_) => return respond(stream, "400 Bad Request", ""),
    };
    if req.path.split('?').next().map(|p| p.trim_end_matches('/')) != Some("/mcp") {
        return respond(stream, "404 Not Found", "");
    }
    if !origin_ok(req.header("origin")) {
        return respond(stream, "403 Forbidden", "");
    }
    if !authorized(server, req.header("authorization")) {
        return respond(stream, "401 Unauthorized", r#"{"error":"a valid bearer token is required"}"#);
    }
    match req.method.as_str() {
        "POST" => {}
        // No server-to-client stream, and no session to end.
        "GET" => return respond(stream, "405 Method Not Allowed", ""),
        "DELETE" => return respond(stream, "200 OK", ""),
        _ => return respond(stream, "405 Method Not Allowed", ""),
    }
    let Ok(msg) = serde_json::from_slice::<Value>(&req.body) else {
        return respond(stream, "400 Bad Request", &rpc_error(Value::Null, -32700, "parse error").to_string());
    };
    let replies: Vec<Value> = match &msg {
        Value::Array(batch) => batch.iter().filter_map(|m| handle(server, m)).collect(),
        m => handle(server, m).into_iter().collect(),
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
fn handle(server: &Server, m: &Value) -> Option<Value> {
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
            let (text, is_error) = match call_window(server, tool, args) {
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
fn call_window(server: &Server, tool: String, args: Value) -> Result<Value, String> {
    let (tx, rx) = channel();
    server.calls.lock().unwrap_or_else(|e| e.into_inner()).push(Call { tool, args, reply: tx });
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
        json!({"name": "send_to", "description": "Convert a tab's document with OfficeCraft's Send to: docs → slides (from the outline), slides → docs, sheets → docs or slides (as a table), graph → docs or slides (as a picture). Into a new tab, or into an open tab with `into`. Or to a PDF file with `pdf_path`, opened in a PDF tab.",
               "inputSchema": {"type": "object", "properties": {"tab": tab, "to": {"type": "string", "enum": ["docs", "slides", "pdf"]}, "into": {"type": "integer", "description": "an open tab to add to, instead of a new one"}, "pdf_path": {"type": "string", "description": "for to=pdf: absolute path of the PDF to write"}}, "required": ["tab", "to"]}}),
        json!({"name": "save", "description": "Save a tab's document: to its own file, or to `path` (Save As; the extension picks the format, e.g. .docx, .xlsx, .pptx). An unsaved new document needs a path.",
               "inputSchema": {"type": "object", "properties": {"tab": tab, "path": {"type": "string"}}, "required": ["tab"]}}),
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
        let s = Server { port: 0, calls: Mutex::new(Vec::new()), tokens: Mutex::new(HashSet::new()), ctx };
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
        let s = Server { port: 0, calls: Mutex::new(Vec::new()), tokens: Mutex::new(HashSet::from(["t0ken".to_string()])), ctx };
        assert!(authorized(&s, Some("Bearer t0ken")));
        assert!(!authorized(&s, Some("Bearer t0ke")));
        assert!(!authorized(&s, Some("t0ken")));
        assert!(!authorized(&s, None));
    }
}
