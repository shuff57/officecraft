//! Stamps the build with the commit it came from (`OFFICECRAFT_BUILD`), so the desktop app and the
//! browser build can be compared at a glance. Falls back to "dev" outside a git checkout.
use std::process::Command;

fn main() {
    let rev = std::env::var("GITHUB_SHA")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(7).collect::<String>())
        .or_else(|| {
            let out = Command::new("git").args(["rev-parse", "--short=7", "HEAD"]).output().ok()?;
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        })
        .unwrap_or_else(|| "dev".into());
    let dirty = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    println!("cargo:rustc-env=OFFICECRAFT_BUILD={rev}{}", if dirty { "+" } else { "" });
    println!("cargo:rustc-env=OFFICECRAFT_APPS={}", app_revisions());
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
}

/// The commit of each app crate the build resolved, read from Cargo.lock ("local" when a
/// `[patch]` path replaced it), so the desktop and browser builds can be compared app by app.
fn app_revisions() -> String {
    let lock = std::fs::read_to_string("Cargo.lock").unwrap_or_default();
    let apps = [
        ("WordCraft", "wordcraft-engine"),
        ("GridCraft", "gridcraft-engine"),
        ("DeckCraft", "deckcraft-engine"),
        ("PdfCraft", "pdfcraft-ui-egui"),
        ("viSHual", "vishual-ui-egui"),
        ("equaSHion", "equashion-egui"),
    ];
    let mut out = Vec::new();
    for (label, krate) in apps {
        let mut rev = "?".to_string();
        let mut name = "";
        for line in lock.lines() {
            if let Some(n) = line.strip_prefix("name = \"") {
                name = n.trim_end_matches('"');
                if name == krate && rev == "?" {
                    rev = "local".into();
                }
            } else if name == krate && line.starts_with("source = \"git+") {
                // Prefer the pinned commit after the final `#`; the first matching entry wins.
                if let Some((_, sha)) = line.trim_end_matches('"').rsplit_once('#') {
                    rev = sha.chars().take(7).collect();
                    break;
                }
            }
        }
        out.push(format!("{label} {rev}"));
    }
    out.join(", ")
}
