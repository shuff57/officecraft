//! The Graph tab: viSHual itself (`viz_native`, the egui port of the web app) inside an OfficeCraft
//! tab. The engine, the logic and the views live in the viSHual repo; this file only hosts them and
//! gives the assistant its view of the tab.
//!
//! The AI key is OfficeCraft's: viSHual's chat box hands its prompts to the suite's assistant
//! (`take_prompts`), so there is one endpoint, key and model for the whole suite.

use eframe::egui;
use serde_json::{json, Value};
use viz_native::{Request, VishualApp};

pub struct GraphApp {
    app: VishualApp,
    prompts: Vec<String>,
}

impl GraphApp {
    pub fn new() -> GraphApp {
        let mut app = VishualApp::new();
        if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))) {
            app.open_gallery_store(dir.join("officecraft").join("vishual-gallery.json"));
        }
        GraphApp { app, prompts: Vec::new() }
    }

    /// Screenshot hook: `OFFICECRAFT_SHOT_VISHUAL=chat,dark`.
    pub fn shot_options(&mut self, spec: &str) {
        for part in spec.split(',') {
            match part.trim() {
                "chat" => self.app.show_chat(),
                "dark" => self.app.set_dark(true),
                "seed" => self.app.seed_demo(),
                _ => {}
            }
        }
    }

    /// The document changed since it was opened or saved.
    pub fn changed(&self) -> bool {
        self.app.changed
    }

    pub fn open_path(&mut self, path: &str) -> Result<(), String> {
        self.app.open_path(path)
    }

    #[allow(dead_code)]
    pub fn save_to(&mut self, path: &str) -> Result<(), String> {
        self.app.save_to(path)
    }

    pub fn doc_json(&mut self) -> String {
        self.app.doc_json()
    }

    /// Prompts typed into viSHual's own chat box, for the suite's assistant to answer.
    pub fn take_prompts(&mut self) -> Vec<String> {
        // viSHual's chat makes a new graph per request and files it in the gallery
        std::mem::take(&mut self.prompts)
            .into_iter()
            .map(|p| {
                format!(
                    "Make a new graph for this request: {p}\n\nRun newGraph, add the items with addItem / setSlider / setMode, check the result, \
                     then run sendToGallery with a short name so it appears as a card in the gallery."
                )
            })
            .collect()
    }

    /// The assistant's view of this tab: `document.inspect`, `engine.commands`, `engine.execute`.
    pub fn control(&mut self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "document.inspect" => {
                let doc: Value = serde_json::from_str(&self.doc_json()).map_err(|e| e.to_string())?;
                Ok(json!({"document": doc, "diagnostics": self.app.diagnostics()}))
            }
            "engine.commands" => Ok(command_catalog()),
            "engine.execute" => {
                let name = params.get("command").and_then(Value::as_str).ok_or("needs a `command` id")?;
                match name {
                    "newGraph" => {
                        self.app.new_graph();
                        return Ok(json!({"ok": true}));
                    }
                    "sendToGallery" => {
                        let title = params.get("params").and_then(|p| p.get("name")).and_then(Value::as_str).unwrap_or("Untitled graph");
                        let key = self.app.file_graph(title);
                        return Ok(json!({"ok": true, "card": key, "graphs_in_gallery": self.app.gallery_len()}));
                    }
                    _ => {}
                }
                let mut c = params.get("params").cloned().unwrap_or_else(|| json!({}));
                let Some(obj) = c.as_object_mut() else {
                    return Err("`params` must be an object".into());
                };
                obj.insert("t".into(), json!(name));
                let events = self.app.dispatch(c);
                let errors: Vec<&str> = events
                    .iter()
                    .filter(|e| e.get("t").and_then(Value::as_str) == Some("error"))
                    .filter_map(|e| e.get("message").and_then(Value::as_str))
                    .collect();
                if let Some(first) = errors.first() {
                    return Err((*first).to_string());
                }
                Ok(json!({"ok": true, "diagnostics": self.app.diagnostics()}))
            }
            other => Err(format!("the Graph tab has no `{other}` method")),
        }
    }

    pub fn logic(&mut self, _ctx: &egui::Context) {}

    pub fn raw_input_hook(&mut self, _raw: &mut egui::RawInput) {}

    /// Coming back to the tab: the fonts are per context, so put viSHual's display serif in.
    pub fn context_changed(&mut self, ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();
        let sans = ["/usr/share/fonts/google-noto/NotoSans-Regular.ttf", "/usr/share/fonts/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"]
            .iter()
            .find_map(|p| std::fs::read(p).ok());
        if let Some(bytes) = sans {
            fonts.font_data.insert("viz_sans".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
            if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                list.insert(0, "viz_sans".into());
            }
        }
        viz_native::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        ctx.set_visuals(egui::Visuals::light());
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.app.ui(ui);
        for r in self.app.take_requests() {
            match r {
                Request::Generate(p) => self.prompts.push(p),
            }
        }
    }
}

/// The commands the assistant may run, with their parameters (the engine's JSON command API).
fn command_catalog() -> Value {
    json!([
        {"id": "setExpr", "label": "Set an expression", "params": {"id": "item id", "latex": "text such as y=x^2, (1,2), r=1+cos(theta), x^2+y^2<4"}, "enabled": true},
        {"id": "addItem", "label": "Add an item", "params": {"id": "new unique id", "kind": "equation|expression|points|slider|note|table", "latex": "its text"}, "enabled": true},
        {"id": "removeItem", "label": "Delete an item", "params": {"id": "item id"}, "enabled": true},
        {"id": "setHidden", "label": "Hide or show an item", "params": {"id": "item id", "hidden": "true|false"}, "enabled": true},
        {"id": "setColor", "label": "Set an item's colour", "params": {"id": "item id", "color": "#rrggbb"}, "enabled": true},
        {"id": "setStyle", "label": "Style an item", "params": {"id": "item id", "style": "{lineWidth, lineStyle: solid|dashed|dotted, pointStyle: dot|circle|cross|square|plus|triangle|diamond|star, pointSize, opacity, fillOpacity, label, showLabel, dragMode: none|x|y|xy}"}, "enabled": true},
        {"id": "setSlider", "label": "Create or move a slider", "params": {"name": "variable name", "value": "number", "min": "number", "max": "number", "step": "number"}, "enabled": true},
        {"id": "setMode", "label": "1D, 2D or 3D", "params": {"mode": "1d|2d|3d"}, "enabled": true},
        {"id": "setView", "label": "Window and axes", "params": {"window": "{min:[x,y,z], max:[x,y,z]}", "grid": "bool", "axes": "bool", "freeAspect": "bool (keep the typed y range)", "xScale": "linear|log", "yScale": "linear|log"}, "enabled": true},
        {"id": "setAngle", "label": "Radians or degrees", "params": {"angle": "rad|deg"}, "enabled": true},
        {"id": "reset", "label": "Reset the camera", "params": {}, "enabled": true},
        {"id": "newGraph", "label": "Start a new, empty graph (the workspace is cleared)", "params": {}, "enabled": true},
        {"id": "sendToGallery", "label": "File the workspace graph in the gallery as a card", "params": {"name": "short title"}, "enabled": true}
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_assistant_drives_the_tab() {
        let mut g = GraphApp::new();
        let r = g.control("engine.execute", json!({"command": "addItem", "params": {"id": "ai1", "kind": "equation", "latex": "y=x"}}));
        assert!(r.is_ok(), "{r:?}");
        let v = g.control("document.inspect", json!({})).map(|v| v.to_string());
        assert!(v.is_ok_and(|s| s.contains("y=x")));
        assert!(g.changed());
        assert!(g.control("engine.commands", json!({})).is_ok_and(|c| c.as_array().is_some_and(|a| a.len() > 5)));
        let bad = g.control("engine.execute", json!({"command": "setMode", "params": {"mode": "9d"}}));
        assert!(bad.is_err());
        assert!(g.control("nope", json!({})).is_err());
    }

    #[test]
    fn the_assistant_makes_a_graph_and_files_it() {
        let mut g = GraphApp::new();
        let before = g.app.gallery_len();
        g.control("engine.execute", json!({"command": "newGraph", "params": {}})).unwrap();
        g.control("engine.execute", json!({"command": "addItem", "params": {"id": "q1", "kind": "equation", "latex": "y=x^2"}})).unwrap();
        let r = g.control("engine.execute", json!({"command": "sendToGallery", "params": {"name": "A parabola"}})).unwrap();
        assert!(r["card"].as_str().is_some_and(|k| !k.is_empty()));
        assert_eq!(r["graphs_in_gallery"].as_u64(), Some(before as u64 + 1));
        assert!(g.take_prompts().is_empty());
    }

    #[test]
    fn a_graph_round_trips_through_a_file() {
        let mut g = GraphApp::new();
        g.control("engine.execute", json!({"command": "addItem", "params": {"id": "a", "kind": "equation", "latex": "y=2x"}})).ok();
        let path = std::env::temp_dir().join(format!("officecraft-graph-{}.vshl", std::process::id()));
        let p = path.to_string_lossy().to_string();
        assert!(g.save_to(&p).is_ok());
        let mut h = GraphApp::new();
        assert!(h.open_path(&p).is_ok());
        assert!(h.doc_json().contains("y=2x"));
        assert!(!h.changed());
        let _ = std::fs::remove_file(path);
        assert!(h.open_path("/no/such/file.vshl").is_err());
    }
}
