//! Where the shell keeps its small settings files. Natively that is `~/.config/officecraft`
//! (or `$XDG_CONFIG_HOME/officecraft`); in a browser page it is `localStorage`, one key per file.

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::path::PathBuf;

    pub fn config_dir() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|c| c.join("officecraft"))
    }

    pub fn read(name: &str) -> Option<String> {
        std::fs::read_to_string(config_dir()?.join(name)).ok()
    }

    pub fn write(name: &str, text: &str) -> Result<(), String> {
        let dir = config_dir().ok_or("no config directory")?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(name), text).map_err(|e| e.to_string())
    }

    /// Writes a file only the owner can read (keys). The mode is set before the secret goes in.
    pub fn write_private(name: &str, text: &str) -> Result<(), String> {
        use std::io::Write;
        let dir = config_dir().ok_or("no config directory")?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(name);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path).map_err(|e| e.to_string())?;
            // an older file keeps its old mode through `mode()`: tighten it too
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
            f.write_all(text.as_bytes()).map_err(|e| e.to_string())
        }
        #[cfg(not(unix))]
        {
            let mut f = std::fs::File::create(&path).map_err(|e| e.to_string())?;
            f.write_all(text.as_bytes()).map_err(|e| e.to_string())
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }

    fn key(name: &str) -> String {
        format!("officecraft/{name}")
    }

    pub fn read(name: &str) -> Option<String> {
        storage()?.get_item(&key(name)).ok().flatten()
    }

    pub fn write(name: &str, text: &str) -> Result<(), String> {
        storage().ok_or("browser storage is unavailable")?.set_item(&key(name), text).map_err(|e| format!("{e:?}"))
    }

    /// A page has no owner-only file; keys sit in this origin's `localStorage`.
    pub fn write_private(name: &str, text: &str) -> Result<(), String> {
        write(name, text)
    }
}

pub use imp::*;

/// Lines of a stored list, without blanks.
pub fn read_lines(name: &str) -> Vec<String> {
    read(name).map(|s| s.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()).unwrap_or_default()
}

pub fn write_lines(name: &str, lines: &[String]) {
    let _ = write(name, &lines.join("\n"));
}

/// "desktop 92d0cc9" / "web 92d0cc9": which commit this build came from and which target it is, so
/// the two can be compared. A trailing `+` means the tree had uncommitted changes.
pub fn build_label() -> String {
    let target = if cfg!(target_arch = "wasm32") { "web" } else { "desktop" };
    format!("{target} {}", env!("OFFICECRAFT_BUILD"))
}
