//! The Graph tab: viSHual's engine (`math_playground`) inside an OfficeCraft tab.
//!
//! The engine is driven the way the web and native shells drive it: JSON commands in, events
//! out, one `frame` per UI frame. Its own headless wgpu renderer draws the scene to RGBA bytes
//! (a separate device from egui's, so the two wgpu versions never meet), which are shown as an
//! egui texture; the tick and item labels are painted over it with egui text.

use eframe::egui;
use math_playground_lib::app::{App, ScreenLabel};
use math_playground_lib::headless::Headless;
use serde_json::{json, Value};

/// One expression row of the sidebar.
struct Row {
    id: String,
    text: String,
}

pub struct GraphApp {
    app: App,
    /// The offscreen GPU renderer; `Err` holds why there is none (no adapter, ...).
    gpu: Result<Box<Headless>, String>,
    rows: Vec<Row>,
    next_id: u32,
    texture: Option<egui::TextureHandle>,
    /// Canvas size in physical pixels the engine and the texture currently have.
    size: (u32, u32),
    dirty: bool,
    started: std::time::Instant,
    diags: std::collections::BTreeMap<String, String>,
    status: String,
    dark: bool,
    /// Last saved or opened path, and whether the document changed since.
    pub path: Option<String>,
    pub changed: bool,
    pressed: Option<u8>,
}

/// A starter graph, so the tab is not empty.
const STARTER: &[&str] = &["y=x^2", "y=\\sin(x)"];

impl GraphApp {
    pub fn new() -> GraphApp {
        let gpu = Headless::new().map(Box::new);
        let mut g = GraphApp {
            app: App::new((800, 600)),
            gpu,
            rows: Vec::new(),
            next_id: 1,
            texture: None,
            size: (800, 600),
            dirty: true,
            started: std::time::Instant::now(),
            diags: Default::default(),
            status: String::new(),
            dark: false,
            path: None,
            changed: false,
            pressed: None,
        };
        // A default document may carry one empty item; replace it with the starter rows.
        if g.app.doc.items.iter().any(|i| i.id == "e0") {
            g.cmd(json!({"t": "removeItem", "id": "e0"}));
        }
        for s in STARTER {
            g.add_row(s);
        }
        g.changed = false;
        g
    }

    /// Runs one engine command and folds its events into the shell state.
    fn cmd(&mut self, c: Value) -> Vec<Value> {
        let out = self.app.dispatch(&c.to_string());
        self.dirty = true;
        let events: Vec<Value> = serde_json::from_str(&out).unwrap_or_default();
        self.absorb(&events);
        events
    }

    fn absorb(&mut self, events: &[Value]) {
        for e in events {
            match e.get("t").and_then(Value::as_str) {
                Some("diagnostics") => {
                    self.diags.clear();
                    if let Some(items) = e.get("items").and_then(Value::as_array) {
                        for it in items {
                            if let (Some(id), Some(m)) = (
                                it.get("id").and_then(Value::as_str),
                                it.get("message").and_then(Value::as_str),
                            ) {
                                self.diags.insert(id.to_string(), m.to_string());
                            }
                        }
                    }
                }
                Some("error") => {
                    self.status = e.get("message").and_then(Value::as_str).unwrap_or("error").to_string();
                }
                Some("itemEdited") => {
                    // A drag rewrote a point's text: show it in its row.
                    if let (Some(id), Some(l)) = (e.get("id").and_then(Value::as_str), e.get("latex").and_then(Value::as_str)) {
                        if let Some(r) = self.rows.iter_mut().find(|r| r.id == id) {
                            r.text = l.to_string();
                        }
                    }
                    self.changed = true;
                }
                Some("sliderValue") => self.changed = true,
                _ => {}
            }
        }
    }

    fn add_row(&mut self, text: &str) {
        let id = format!("g{}", self.next_id);
        self.next_id += 1;
        self.cmd(json!({"t": "addItem", "id": id, "kind": "equation", "latex": text}));
        self.rows.push(Row { id, text: text.to_string() });
        self.changed = true;
    }

    /// Replaces the whole document (opening a file) and rebuilds the rows from its items.
    pub fn load_json(&mut self, json_text: &str) -> Result<(), String> {
        let events = self.cmd(json!({"t": "loadDoc", "json": json_text}));
        if let Some(e) = events.iter().find(|e| e.get("t").and_then(Value::as_str) == Some("error")) {
            return Err(e.get("message").and_then(Value::as_str).unwrap_or("could not load the graph").to_string());
        }
        self.rows = self
            .app
            .doc
            .items
            .iter()
            .filter(|i| !matches!(i.kind, math_core::doc::ItemKind::Table | math_core::doc::ItemKind::Folder))
            .map(|i| Row { id: i.id.clone(), text: i.latex.clone() })
            .collect();
        self.changed = false;
        Ok(())
    }

    pub fn open_path(&mut self, path: &str) -> Result<(), String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        self.load_json(&text)?;
        self.path = Some(path.to_string());
        Ok(())
    }

    pub fn doc_json(&self) -> String {
        math_core::doc::to_json(&self.app.doc)
    }

    pub fn save_to(&mut self, path: &str) -> Result<(), String> {
        std::fs::write(path, self.doc_json()).map_err(|e| e.to_string())?;
        self.path = Some(path.to_string());
        self.changed = false;
        Ok(())
    }

    /// The assistant's view of this tab: `document.inspect`, `engine.commands`, `engine.execute`.
    pub fn control(&mut self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "document.inspect" => {
                let doc: Value = serde_json::from_str(&self.doc_json()).map_err(|e| e.to_string())?;
                Ok(json!({"document": doc, "diagnostics": self.diags}))
            }
            "engine.commands" => Ok(command_catalog()),
            "engine.execute" => {
                let name = params.get("command").and_then(Value::as_str).ok_or("needs a `command` id")?;
                let mut c = params.get("params").cloned().unwrap_or_else(|| json!({}));
                let Some(obj) = c.as_object_mut() else {
                    return Err("`params` must be an object".into());
                };
                obj.insert("t".into(), json!(name));
                let events = self.cmd(c);
                // Keep the sidebar in step with what the assistant did.
                self.rows = self
                    .app
                    .doc
                    .items
                    .iter()
                    .filter(|i| !matches!(i.kind, math_core::doc::ItemKind::Table | math_core::doc::ItemKind::Folder))
                    .map(|i| Row { id: i.id.clone(), text: i.latex.clone() })
                    .collect();
                self.changed = true;
                let errors: Vec<&str> = events
                    .iter()
                    .filter(|e| e.get("t").and_then(Value::as_str) == Some("error"))
                    .filter_map(|e| e.get("message").and_then(Value::as_str))
                    .collect();
                if let Some(first) = errors.first() {
                    return Err((*first).to_string());
                }
                Ok(json!({"ok": true, "diagnostics": self.diags}))
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
        egui::Panel::left("graph_side").resizable(true).default_size(300.0).size_range(220.0..=480.0).show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default().show(ui, |ui| self.canvas(ui));
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Open…").clicked() {
                let picked = rfd::FileDialog::new().add_filter("viSHual", &["vshl", "vishual", "json"]).pick_file();
                if let Some(p) = picked {
                    self.status = match self.open_path(&p.to_string_lossy()) {
                        Ok(()) => String::new(),
                        Err(e) => format!("Could not open: {e}"),
                    };
                }
            }
            if ui.button("Save…").clicked() {
                let name = self
                    .path
                    .as_deref()
                    .and_then(|p| std::path::Path::new(p).file_name().and_then(|n| n.to_str()).map(str::to_string))
                    .unwrap_or_else(|| "graph.vshl".into());
                let picked = rfd::FileDialog::new().set_file_name(name).add_filter("viSHual", &["vshl"]).save_file();
                if let Some(p) = picked {
                    self.status = match self.save_to(&p.to_string_lossy()) {
                        Ok(()) => String::new(),
                        Err(e) => format!("Could not save: {e}"),
                    };
                }
            }
        });
        ui.horizontal(|ui| {
            for (label, mode, m) in [
                ("1D", "1d", math_core::view::Mode::D1),
                ("2D", "2d", math_core::view::Mode::D2),
                ("3D", "3d", math_core::view::Mode::D3),
            ] {
                if ui.selectable_label(self.app.mode() == m, label).clicked() {
                    self.cmd(json!({"t": "setMode", "mode": mode}));
                }
            }
            if ui.button("Reset view").clicked() {
                self.cmd(json!({"t": "reset"}));
            }
            let dark = self.dark;
            if ui.selectable_label(dark, "Dark").clicked() {
                self.dark = !dark;
                self.cmd(json!({"t": "setTheme", "dark": self.dark}));
            }
        });
        ui.separator();
        let mut remove: Option<String> = None;
        let mut edits: Vec<(String, String)> = Vec::new();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (n, r) in self.rows.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("{}", n + 1));
                    let resp = ui.add(egui::TextEdit::singleline(&mut r.text).desired_width(ui.available_width() - 28.0).hint_text("y = x^2, (1, 2), a x + b ..."));
                    if resp.changed() {
                        edits.push((r.id.clone(), r.text.clone()));
                    }
                    if ui.small_button("×").on_hover_text("Delete").clicked() {
                        remove = Some(r.id.clone());
                    }
                });
                if let Some(m) = self.diags.get(&r.id) {
                    ui.colored_label(egui::Color32::from_rgb(190, 60, 50), m);
                }
            }
            if ui.button("+ Expression").clicked() {
                self.add_row("");
            }
            // Sliders the expressions use: one row each.
            let names: Vec<String> = self.app.doc.sliders.keys().cloned().collect();
            if !names.is_empty() {
                ui.separator();
            }
            for name in names {
                let Some(cfg) = self.app.doc.sliders.get(&name).cloned() else { continue };
                let mut v = cfg.value;
                ui.horizontal(|ui| {
                    ui.label(format!("{name} ="));
                    if ui.add(egui::Slider::new(&mut v, cfg.min..=cfg.max)).changed() {
                        self.cmd(json!({"t": "setSlider", "name": name, "value": v}));
                        self.changed = true;
                    }
                });
            }
        });
        for (id, text) in edits {
            self.cmd(json!({"t": "setExpr", "id": id, "latex": text}));
            self.changed = true;
        }
        if let Some(id) = remove {
            self.cmd(json!({"t": "removeItem", "id": id}));
            self.rows.retain(|r| r.id != id);
            self.changed = true;
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let ppp = ui.ctx().pixels_per_point();
        let phys = (
            ((rect.width() * ppp).round() as u32).clamp(16, 4096),
            ((rect.height() * ppp).round() as u32).clamp(16, 4096),
        );
        if phys != self.size {
            self.size = phys;
            self.cmd(json!({"t": "resize", "width": phys.0, "height": phys.1}));
        }
        let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        let to_px = |p: egui::Pos2| ((p.x - rect.min.x) as f64 * ppp as f64, (p.y - rect.min.y) as f64 * ppp as f64);

        // Pointer: a drag pans / orbits or moves a point, as in the other shells.
        let (down, up, pos, shift, scroll) = ui.input(|i| {
            (
                i.pointer.button_pressed(egui::PointerButton::Primary),
                i.pointer.button_released(egui::PointerButton::Primary),
                i.pointer.hover_pos().or(i.pointer.interact_pos()),
                i.modifiers.shift,
                i.smooth_scroll_delta.y,
            )
        });
        if let Some(p) = pos {
            let inside = rect.contains(p);
            let (x, y) = to_px(p);
            if down && inside && resp.hovered() {
                self.pressed = Some(0);
                self.cmd(json!({"t": "pointer", "phase": "down", "x": x, "y": y, "button": 0, "shift": shift}));
            } else if self.pressed.is_some() && resp.dragged() {
                self.cmd(json!({"t": "pointer", "phase": "move", "x": x, "y": y}));
            } else if self.pressed.is_none() && inside && resp.hovered() {
                // hover: tooltips and cursor hints are the engine's; ignore the answer
                let _ = self.app.dispatch(&json!({"t": "pointer", "phase": "move", "x": x, "y": y}).to_string());
            }
            if up && self.pressed.is_some() {
                self.pressed = None;
                self.cmd(json!({"t": "pointer", "phase": "up", "x": x, "y": y}));
                self.changed = true;
            }
            if inside && resp.hovered() && scroll != 0.0 {
                self.cmd(json!({"t": "wheel", "x": x, "y": y, "dy": -(scroll as f64)}));
            }
        }

        let now = self.started.elapsed().as_secs_f64() * 1000.0;
        let redraw = self.app.frame(now);
        let events: Vec<Value> = serde_json::from_str(&self.app.drain_json()).unwrap_or_default();
        self.absorb(&events);
        let animating = self.app.rig.is_animating() || self.app.busy();
        if redraw || self.dirty || animating || self.texture.is_none() {
            self.render(ui.ctx());
            self.dirty = false;
        }
        if animating || redraw {
            ui.ctx().request_repaint();
        }

        let painter = ui.painter_at(rect);
        match (&self.texture, &self.gpu) {
            (Some(t), _) => {
                painter.image(t.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            }
            (None, Err(why)) => {
                painter.text(rect.center(), egui::Align2::CENTER_CENTER, format!("The graph renderer is unavailable: {why}"), egui::FontId::proportional(15.0), egui::Color32::GRAY);
            }
            _ => {}
        }
        self.paint_labels(&painter, rect, ppp);
        if !self.status.is_empty() {
            let msg = self.status.clone();
            painter.text(rect.left_bottom() + egui::vec2(8.0, -8.0), egui::Align2::LEFT_BOTTOM, msg, egui::FontId::proportional(12.0), egui::Color32::from_rgb(190, 60, 50));
        }
    }

    fn render(&mut self, ctx: &egui::Context) {
        let Ok(gpu) = &mut self.gpu else { return };
        let size = self.size;
        let layers = self.app.layers();
        let inset = self.app.inset();
        let rgba = gpu.render_rgba_with_inset(size, &self.app.rig, &layers, self.app.background(), inset.as_ref());
        let image = egui::ColorImage::from_rgba_unmultiplied([size.0 as usize, size.1 as usize], &rgba);
        match &mut self.texture {
            Some(t) => t.set(image, egui::TextureOptions::LINEAR),
            None => self.texture = Some(ctx.load_texture("officecraft-graph", image, egui::TextureOptions::LINEAR)),
        }
    }

    /// Tick numbers, axis names and item labels, placed where the engine projected them.
    fn paint_labels(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        let ink = if self.dark { egui::Color32::from_gray(225) } else { egui::Color32::from_gray(40) };
        let mut labels: Vec<ScreenLabel> = self.app.screen_labels();
        labels.extend(self.app.inset_screen_labels());
        for l in labels.iter().filter(|l| l.visible && !l.text.is_empty()) {
            let (ox, oy) = l.offset.map_or((0.0, 0.0), |o| (o[0] as f32, o[1] as f32));
            let p = rect.min + egui::vec2(l.x as f32 / ppp + ox, l.y as f32 / ppp + oy);
            let scale = l.size.unwrap_or(1.0) as f32;
            let font = egui::FontId::proportional(12.0 * scale);
            // Axis 0 numbers sit under their tick, axis 1 to the left of it; the rest centred.
            let (anchor, nudge) = match l.axis {
                0 => (egui::Align2::CENTER_TOP, egui::vec2(0.0, 4.0)),
                1 => (egui::Align2::RIGHT_CENTER, egui::vec2(-5.0, 0.0)),
                _ => (egui::Align2::LEFT_BOTTOM, egui::vec2(6.0, -4.0)),
            };
            let color = ink.gamma_multiply(l.alpha.clamp(0.0, 1.0) as f32);
            if rect.contains(p) {
                painter.text(p + nudge, anchor, &l.text, font, color);
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

    /// Runs the tab for a couple of frames in a headless egui context.
    fn frames(app: &mut GraphApp, n: usize) -> egui::Context {
        let ctx = egui::Context::default();
        for i in 0..n {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 600.0))),
                time: Some(i as f64 / 60.0),
                ..Default::default()
            };
            let mut out = ctx.run_ui(raw, |ui| app.ui(ui));
            // The textures were "uploaded": the context insists the deltas are looked at.
            out.textures_delta.clear();
        }
        ctx
    }

    #[test]
    fn the_tab_draws_a_texture_and_follows_expressions() {
        let mut g = GraphApp::new();
        if g.gpu.is_err() {
            eprintln!("graph test skipped: no GPU adapter");
            return; // nothing to draw with on this machine
        }
        frames(&mut g, 3);
        let t = g.texture.as_ref().map(|t| t.size());
        assert!(t.is_some_and(|s| s[0] > 100 && s[1] > 100), "{t:?}");
        // the rows are the starter expressions, and editing one reaches the engine
        assert_eq!(g.rows.len(), STARTER.len());
        let id = g.rows[0].id.clone();
        g.cmd(json!({"t": "setExpr", "id": id, "latex": "y=3x"}));
        assert!(g.app.doc.items.iter().any(|i| i.latex == "y=3x"));
        // the assistant's surface: inspect, catalog, execute
        let v = g.control("document.inspect", json!({})).map(|v| v.to_string());
        assert!(v.is_ok_and(|s| s.contains("y=3x")));
        assert!(g.control("engine.commands", json!({})).is_ok_and(|c| c.as_array().is_some_and(|a| a.len() > 5)));
        let r = g.control("engine.execute", json!({"command": "addItem", "params": {"id": "ai1", "kind": "equation", "latex": "y=x"}}));
        assert!(r.is_ok(), "{r:?}");
        assert!(g.rows.iter().any(|r| r.id == "ai1"));
        let bad = g.control("engine.execute", json!({"command": "setMode", "params": {"mode": "9d"}}));
        assert!(bad.is_err());
        assert!(g.control("nope", json!({})).is_err());
    }

    #[test]
    fn a_graph_round_trips_through_a_file() {
        let mut g = GraphApp::new();
        let path = std::env::temp_dir().join(format!("officecraft-graph-{}.vshl", std::process::id()));
        let p = path.to_string_lossy().to_string();
        g.save_to(&p).ok();
        let mut h = GraphApp::new();
        h.cmd(json!({"t": "removeItem", "id": h.rows.first().map(|r| r.id.clone()).unwrap_or_default()}));
        assert!(h.open_path(&p).is_ok());
        assert_eq!(h.rows.len(), g.rows.len());
        assert!(!h.changed);
        let _ = std::fs::remove_file(path);
        assert!(h.open_path("/no/such/file.vshl").is_err());
    }
}
