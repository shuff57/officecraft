//! In-app updates: asks `scripts/update.sh` which of the followed app repos have new commits and
//! rebuilds OfficeCraft against them. The script does the work (so it also runs without the UI);
//! this window only runs it on a thread and shows what it prints.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};

/// One line of `update.sh check`.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub name: String,
    pub state: String,
    pub follows: String,
    pub revision: String,
}

fn parse_status(line: &str) -> Option<Status> {
    let mut p = line.split('\t');
    Some(Status { name: p.next()?.into(), state: p.next()?.into(), follows: p.next()?.into(), revision: p.next().unwrap_or("").into() })
}

enum Msg {
    Checked(Result<Vec<Status>, String>),
    Line(String),
    Finished(bool),
}

#[derive(PartialEq)]
enum Phase {
    Idle,
    Checking,
    Updating,
    Done(bool),
}

pub struct Updater {
    pub open: bool,
    phase: Phase,
    statuses: Vec<Status>,
    error: Option<String>,
    log: Vec<String>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    script: Option<PathBuf>,
}

/// `~/.local/share/officecraft/install.env` is written by install.sh; it names the checkout.
fn find_script() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("OFFICECRAFT_UPDATE_SCRIPT") {
        return Some(PathBuf::from(p));
    }
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    let env = std::fs::read_to_string(data.join("officecraft/install.env")).ok()?;
    let dir = env.lines().find_map(|l| l.strip_prefix("SOURCE_DIR="))?.trim().trim_matches('\'');
    let script = PathBuf::from(dir).join("scripts/update.sh");
    script.is_file().then_some(script)
}

impl Updater {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Updater { open: false, phase: Phase::Idle, statuses: Vec::new(), error: None, log: Vec::new(), tx, rx, script: find_script() }
    }

    fn run(&self, ctx: &egui::Context, verb: &'static str) {
        let Some(script) = self.script.clone() else { return };
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let child = Command::new("bash").arg(&script).arg(verb).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
            let mut child = match child {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(if verb == "check" { Msg::Checked(Err(e.to_string())) } else { Msg::Finished(false) });
                    ctx.request_repaint();
                    return;
                }
            };
            // Merge stderr into the same stream: cargo prints its progress there.
            let err = child.stderr.take().map(|s| {
                let tx = tx.clone();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    for l in BufReader::new(s).lines().map_while(Result::ok) {
                        let _ = tx.send(Msg::Line(l));
                        ctx.request_repaint();
                    }
                })
            });
            let mut out = Vec::new();
            if let Some(s) = child.stdout.take() {
                for l in BufReader::new(s).lines().map_while(Result::ok) {
                    if verb == "check" {
                        out.push(l);
                    } else {
                        let _ = tx.send(Msg::Line(l));
                        ctx.request_repaint();
                    }
                }
            }
            if let Some(h) = err {
                let _ = h.join();
            }
            let ok = child.wait().map(|s| s.success()).unwrap_or(false);
            let _ = tx.send(if verb == "check" {
                Msg::Checked(if ok { Ok(out.iter().filter_map(|l| parse_status(l)).collect()) } else { Err("update check failed (see the terminal output)".into()) })
            } else {
                Msg::Finished(ok)
            });
            ctx.request_repaint();
        });
    }

    /// Draws the window when open; returns true when the user asked to restart into the new build.
    pub fn ui(&mut self, ctx: &egui::Context) -> bool {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Checked(Ok(s)) => {
                    self.statuses = s;
                    self.error = None;
                    self.phase = Phase::Idle;
                }
                Msg::Checked(Err(e)) => {
                    self.error = Some(e);
                    self.phase = Phase::Idle;
                }
                Msg::Line(l) => {
                    self.log.push(l);
                    if self.log.len() > 400 {
                        self.log.drain(..100);
                    }
                }
                Msg::Finished(ok) => self.phase = Phase::Done(ok),
            }
        }
        let mut restart = false;
        if !self.open {
            return false;
        }
        let mut open = true;
        egui::Window::new("Updates").open(&mut open).collapsible(false).default_width(560.0).show(ctx, |ui| {
            ui.weak(format!("This build: {}", crate::platform::build_label()));
            if self.script.is_none() {
                ui.label("Updates need the source checkout this build came from.");
                ui.label("Run ./install.sh once from an OfficeCraft checkout (it records where that is).");
                return;
            }
            ui.label("Rebuilds OfficeCraft against the newest commits of the repos below. Choose which repo and branch each app follows in ~/.config/officecraft/sources.");
            ui.add_space(6.0);
            let busy = matches!(self.phase, Phase::Checking | Phase::Updating);
            egui::Grid::new("update-status").num_columns(3).spacing([14.0, 4.0]).show(ui, |ui| {
                for s in &self.statuses {
                    ui.label(egui::RichText::new(&s.name).strong());
                    let (text, color) = match s.state.as_str() {
                        "current" => ("up to date", egui::Color32::from_rgb(0x1f, 0x8a, 0x5b)),
                        "update" => ("update available", egui::Color32::from_rgb(0xc8, 0x6a, 0x1a)),
                        "new" => ("not built from this source yet", egui::Color32::from_rgb(0xc8, 0x6a, 0x1a)),
                        _ => ("unreachable", egui::Color32::from_rgb(0xc0, 0x39, 0x2b)),
                    };
                    ui.colored_label(color, text);
                    ui.weak(format!("{}  {}", s.follows, s.revision));
                    ui.end_row();
                }
            });
            if self.statuses.is_empty() && self.phase == Phase::Idle {
                ui.weak("Press Check to see what changed.");
            }
            if let Some(e) = &self.error {
                ui.colored_label(egui::Color32::from_rgb(0xc0, 0x39, 0x2b), e);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Button::new("Check")).clicked() {
                    self.phase = Phase::Checking;
                    self.run(ui.ctx(), "check");
                }
                if ui.add_enabled(!busy, egui::Button::new("Update and rebuild")).on_hover_text("Builds a release binary; this takes several minutes").clicked() {
                    self.phase = Phase::Updating;
                    self.log.clear();
                    self.run(ui.ctx(), "apply");
                }
                match self.phase {
                    Phase::Checking => {
                        ui.spinner();
                        ui.label("Checking...");
                    }
                    Phase::Updating => {
                        ui.spinner();
                        ui.label("Building...");
                    }
                    Phase::Done(true) => {
                        ui.colored_label(egui::Color32::from_rgb(0x1f, 0x8a, 0x5b), "Installed.");
                        if ui.button("Restart OfficeCraft").clicked() {
                            restart = true;
                        }
                    }
                    Phase::Done(false) => {
                        ui.colored_label(egui::Color32::from_rgb(0xc0, 0x39, 0x2b), "Build failed; the running version is unchanged.");
                    }
                    Phase::Idle => {}
                }
            });
            if !self.log.is_empty() {
                ui.add_space(6.0);
                egui::ScrollArea::vertical().max_height(220.0).stick_to_bottom(true).show(ui, |ui| {
                    for l in &self.log {
                        ui.label(egui::RichText::new(l).monospace().size(11.0));
                    }
                });
            }
        });
        if !open {
            self.open = false;
        }
        restart
    }
}

/// Starts the freshly installed binary; the caller then closes this window.
pub fn spawn_new_instance() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = Command::new(exe).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_check_lines() {
        let s = parse_status("wordcraft\tupdate\thttps://github.com/x/wordcraft main\tabcd1234").unwrap();
        assert_eq!((s.name.as_str(), s.state.as_str(), s.revision.as_str()), ("wordcraft", "update", "abcd1234"));
        assert!(parse_status("junk").is_none());
    }
}
