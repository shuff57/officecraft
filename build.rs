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
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
}
