//! The Equations tab: equaSHion, the equation editor of the suite, as an app of its own.
//!
//! The widget is `equashion-egui`'s [`EquationEditor`] on a canvas of its own, with its template
//! palette and linear-text field. Above it sit the Copy buttons: LaTeX, MathML, OMML and plain
//! text go to the clipboard; the image (PNG, black ink on white) and the SVG are saved to a file
//! (a download in a browser). Nothing here touches the suite's fonts: the widget draws with its
//! own bundled ones, and the chrome uses egui's built-ins.

use std::sync::Arc;

use eframe::egui;
use equashion_egui::equashion_math::{Math, to_latex, to_linear, to_mathml_display, to_omml};
use equashion_egui::{EquationEditor, bundled_fonts};
use equashion_layout::{DEFAULT_FAMILY, FontDb, Rgb, layout_equation};

/// Size of the image and SVG exports, points.
const EXPORT_PT: f64 = 20.0;
/// Pixels per point of the PNG export.
const PNG_SCALE: f64 = 2.0;
/// Space around the equation in the exports, points.
const EXPORT_PAD: f64 = 6.0;
/// How long a confirmation stays under the buttons, seconds.
const NOTE_SECS: f64 = 4.0;

/// What a Copy button produces.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Export {
    Latex,
    MathMl,
    Omml,
    Text,
    Png,
    Svg,
}

impl Export {
    const ALL: [Export; 6] = [Export::Latex, Export::MathMl, Export::Omml, Export::Text, Export::Png, Export::Svg];

    fn label(self) -> &'static str {
        match self {
            Export::Latex => "Copy LaTeX",
            Export::MathMl => "Copy MathML",
            Export::Omml => "Copy OMML",
            Export::Text => "Copy text",
            Export::Png => "Save image…",
            Export::Svg => "Save SVG…",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Export::Latex => "LaTeX for Overleaf, Markdown and Anki",
            Export::MathMl => "MathML for ONLYOFFICE, LibreOffice Math and Word",
            Export::Omml => "Word's equation XML",
            Export::Text => "Plain linear text, like a/b",
            Export::Png => "A PNG, black ink on white, 144 dpi",
            Export::Svg => "An SVG, black ink",
        }
    }
}

/// The Equations tab.
pub struct EquationApp {
    editor: EquationEditor,
    fonts: Arc<FontDb>,
    /// What the last Copy or Save said, and when (egui time).
    note: Option<(String, f64)>,
}

impl EquationApp {
    pub fn new() -> Self {
        let editor = EquationEditor::with_linear(r"x=\frac{-b\pm\sqrt{b^2-4ac}}{2a}").id_salt("officecraft-equations").font_size(34.0).canvas_height(240.0);
        EquationApp { editor, fonts: bundled_fonts(), note: None }
    }

    /// Another app has replaced the shared egui fonts and style: take the built-in ones back, so
    /// this tab's chrome never names a font family another app registered.
    pub fn context_changed(&mut self, ctx: &egui::Context) {
        ctx.all_styles_mut(|s| *s = egui::Style::default());
        ctx.set_fonts(egui::FontDefinitions::default());
        ctx.set_visuals(egui::Visuals::light());
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let now = ui.input(|i| i.time);
        if self.note.as_ref().is_some_and(|(_, at)| now - at > NOTE_SECS) {
            self.note = None;
        }
        let fill = ui.visuals().panel_fill;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(16, 12))).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.label(egui::RichText::new("equaSHion").strong().size(16.0));
                ui.add_space(6.0);
                self.editor.show(ui);
                // The Copy and Save buttons sit under the editor, with what the last one did.
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    for kind in Export::ALL {
                        if ui.button(kind.label()).on_hover_text(kind.hint()).clicked() {
                            self.export(ui.ctx(), kind, now);
                        }
                    }
                });
                if let Some((text, _)) = &self.note {
                    ui.label(egui::RichText::new(text).weak().size(12.5));
                }
            });
        });
    }

    fn export(&mut self, ctx: &egui::Context, kind: Export, now: f64) {
        let m = self.editor.math().clone();
        let said = match kind {
            Export::Latex => copy(ctx, to_latex(&m), "LaTeX"),
            Export::MathMl => copy(ctx, to_mathml_display(&m, true), "MathML"),
            Export::Omml => copy(ctx, to_omml(&m), "OMML"),
            Export::Text => copy(ctx, to_linear(&m), "text"),
            Export::Png => save("equation.png", "PNG", &["png"], png_bytes(&m, &self.fonts)),
            Export::Svg => save("equation.svg", "SVG", &["svg"], svg_text(&m, &self.fonts).into_bytes()),
        };
        self.note = Some((said, now));
    }
}

fn copy(ctx: &egui::Context, text: String, what: &str) -> String {
    if text.trim().is_empty() {
        return format!("Nothing to copy: the equation is empty.");
    }
    ctx.copy_text(text);
    format!("Copied {what}.")
}

/// The equation as an SVG document, black ink.
fn svg_text(m: &Math, fonts: &FontDb) -> String {
    let l = layout_equation(fonts, m, DEFAULT_FAMILY, EXPORT_PT, m.para, Rgb::BLACK, EXPORT_PAD);
    equashion_render::scene_to_svg(&l.scene, fonts, DEFAULT_FAMILY)
}

/// The equation as a PNG: black ink on white at [`PNG_SCALE`] pixels per point, tagged with the
/// matching resolution. Empty on an encoder error.
fn png_bytes(m: &Math, fonts: &FontDb) -> Vec<u8> {
    let l = layout_equation(fonts, m, DEFAULT_FAMILY, EXPORT_PT, m.para, Rgb::BLACK, EXPORT_PAD);
    let img = equashion_render::render_scene(&l.scene, fonts, PNG_SCALE, Some(Rgb::WHITE));
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width.max(1), img.height.max(1));
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        // Pixels per metre: PNG_SCALE pixels per point, 72 points per inch.
        let ppm = (PNG_SCALE * 72.0 / 0.0254).round() as u32;
        enc.set_pixel_dims(Some(png::PixelDimensions { xppu: ppm, yppu: ppm, unit: png::Unit::Meter }));
        let Ok(mut w) = enc.write_header() else { return Vec::new() };
        if w.write_image_data(&img.to_straight()).is_err() {
            return Vec::new();
        }
    }
    out
}

/// Asks where to put `bytes` (a download in a browser) and writes them.
#[cfg(not(target_arch = "wasm32"))]
fn save(name: &str, what: &str, exts: &[&str], bytes: Vec<u8>) -> String {
    if bytes.is_empty() {
        return format!("Could not make the {what}.");
    }
    let Some(path) = rfd::FileDialog::new().set_file_name(name).add_filter(what, exts).save_file() else {
        return "Cancelled.".to_string();
    };
    match std::fs::write(&path, bytes) {
        Ok(()) => format!("Saved {}.", path.display()),
        Err(e) => format!("Could not save {}: {e}", path.display()),
    }
}

#[cfg(target_arch = "wasm32")]
fn save(name: &str, what: &str, _exts: &[&str], bytes: Vec<u8>) -> String {
    if bytes.is_empty() {
        return format!("Could not make the {what}.");
    }
    match crate::web::download(name, &bytes) {
        Ok(()) => format!("Downloaded {name}."),
        Err(e) => format!("Could not download {name}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use equashion_egui::equashion_math::from_linear;

    #[test]
    fn the_exports_are_real_files_and_real_text() {
        let m = from_linear("x=(-b+-sqrt(b^2-4ac))/2a");
        let fonts = bundled_fonts();
        let png = png_bytes(&m, &fonts);
        assert_eq!(png.get(..8), Some(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A][..]), "a PNG signature");
        assert!(png.len() > 500, "an image with ink: {} bytes", png.len());
        let svg = svg_text(&m, &fonts);
        assert!(svg.contains("<svg") && svg.contains("</svg>"), "an SVG document");
        assert!(to_latex(&m).contains("frac") && to_latex(&m).contains("sqrt"));
        assert!(to_mathml_display(&m, true).contains("<math"));
        assert!(to_omml(&m).contains("oMath"));
        assert!(!to_linear(&m).is_empty());
    }

    #[test]
    fn a_new_tab_starts_on_the_quadratic_formula() {
        let app = EquationApp::new();
        assert!(to_latex(app.editor.math()).contains("frac"));
    }
}
