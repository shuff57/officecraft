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
        GraphApp { app: VishualApp::new(), prompts: Vec::new() }
    }

    /// Screenshot hook: `OFFICECRAFT_SHOT_VISHUAL=chat,dark`.
    pub fn shot_options(&mut self, spec: &str) {
        for part in spec.split(',') {
            match part.trim() {
                "chat" => self.app.show_chat(),
                "dark" => self.app.set_dark(true),
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

    pub fn doc_json(&self) -> String {
        self.app.doc_json()
    }

    /// Prompts typed into viSHual's own chat box, for the suite's assistant to answer.
    pub fn take_prompts(&mut self) -> Vec<String> {
        std::mem::take(&mut self.prompts)
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

    pub fn context_changed(&mut self) {
        // Nothing of ours lives in the shared egui context except the texture, which is kept.
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
        {"id": "reset", "label": "Reset the camera", "params": {}, "enabled": true}
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
