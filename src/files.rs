//! File operations for the Home page (desktop build): list one folder, rename, make a folder,
//! move into another folder, and Move to Trash.
//!
//! Trash goes to the system trash (freedesktop / Windows Recycle Bin / macOS Trash). Where there
//! is none, typically a NAS or network mount, the item goes to OfficeCraft's own trash under
//! `<data dir>/Trash` instead, with a note of where it came from so it can be restored. Nothing
//! here deletes a file outright except "Delete forever" on an item already in OfficeCraft's trash.

use std::path::{Path, PathBuf};

/// The subfolders (first, by name) and supported files (newest first) directly inside `dir`.
/// Hidden entries are skipped. `supported` says which files to keep.
pub fn list_dir(dir: &Path, supported: impl Fn(&str) -> bool) -> Result<Vec<PathBuf>, String> {
    let read = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for e in read.flatten() {
        let path = e.path();
        if path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
            continue;
        }
        // Follow symlinks: a link to a folder browses like the folder.
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if meta.is_dir() {
            dirs.push(path);
        } else if meta.is_file() && path.to_str().is_some_and(&supported) {
            files.push((meta.modified().ok(), path));
        }
    }
    dirs.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    files.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(dirs.into_iter().chain(files.into_iter().map(|(_, path)| path)).collect())
}

/// A file or folder name the user typed: not empty, no path separators, not `.` or `..`.
pub fn check_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("The name is empty.".into());
    }
    if name == "." || name == ".." || name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err("A name can't contain / or \\, or be . or ..".into());
    }
    Ok(name)
}

/// Renames `path` to `new_name` in the same folder. Refuses to replace an existing item.
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    let name = check_name(new_name)?;
    let dest = path.with_file_name(name);
    if dest == path {
        return Ok(dest);
    }
    if dest.exists() {
        return Err(format!("{name} already exists here."));
    }
    std::fs::rename(path, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// Makes folder `name` inside `dir`.
pub fn new_folder(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = check_name(name)?;
    let dest = dir.join(name);
    if dest.exists() {
        return Err(format!("{name} already exists here."));
    }
    std::fs::create_dir(&dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// Moves `src` into folder `dest_dir`, keeping its name. Refuses to replace an existing item or
/// to move a folder into itself.
pub fn move_into(src: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    let name = src.file_name().ok_or("nothing to move")?;
    let dest = dest_dir.join(name);
    if dest == src || src.parent() == Some(dest_dir) {
        return Ok(src.to_path_buf());
    }
    if let (Ok(s), Ok(d)) = (src.canonicalize(), dest_dir.canonicalize())
        && d.starts_with(&s)
    {
        return Err("A folder can't be moved into itself.".into());
    }
    if dest.exists() {
        return Err(format!("{} already exists in {}.", name.to_string_lossy(), dest_dir.display()));
    }
    relocate(src, &dest)?;
    Ok(dest)
}

/// `fs::rename`, or copy and delete when `src` and `dest` are on different file systems.
fn relocate(src: &Path, dest: &Path) -> Result<(), String> {
    match std::fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(_) => {
            copy_all(src, dest).map_err(|e| format!("copy to {}: {e}", dest.display()))?;
            if src.is_dir() { std::fs::remove_dir_all(src) } else { std::fs::remove_file(src) }.map_err(|e| format!("{} was copied but not removed: {e}", src.display()))
        }
    }
}

fn copy_all(src: &Path, dest: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir(dest)?;
        for e in std::fs::read_dir(src)?.flatten() {
            copy_all(&e.path(), &dest.join(e.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dest).map(|_| ())
    }
}

/// Where a trashed item went.
#[derive(Debug, PartialEq, Eq)]
pub enum Trashed {
    /// The system trash: restore it from the file manager.
    System,
    /// OfficeCraft's own trash (no system trash for that location): restore it from Home.
    Own,
}

/// Moves `path` (a file or a whole folder) to the trash.
pub fn trash(path: &Path) -> Result<Trashed, String> {
    match ::trash::delete(path) {
        Ok(()) => Ok(Trashed::System),
        Err(sys) => {
            let root = own_trash_root().ok_or_else(|| format!("no trash for this location ({sys})"))?;
            to_own_trash(&root, path).map(|_| Trashed::Own).map_err(|e| format!("system trash: {sys}; OfficeCraft trash: {e}"))
        }
    }
}

/// `<data dir>/Trash`, with `files/` (the items) and `info/` (one `<item>.json` each).
pub fn own_trash_root() -> Option<PathBuf> {
    crate::platform::data_dir().map(|d| d.join("Trash"))
}

/// One item in OfficeCraft's own trash.
#[derive(Clone)]
pub struct OwnItem {
    /// The item's name in `files/` (unique).
    pub key: String,
    pub original: PathBuf,
    pub deleted: Option<std::time::SystemTime>,
    pub is_dir: bool,
}

fn to_own_trash(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let files = root.join("files");
    let info = root.join("info");
    std::fs::create_dir_all(&files).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&info).map_err(|e| e.to_string())?;
    let base = path.file_name().ok_or("nothing to trash")?.to_string_lossy().to_string();
    let mut key = base.clone();
    let mut n = 1;
    while files.join(&key).exists() || info.join(format!("{key}.json")).exists() {
        n += 1;
        key = format!("{base}.{n}");
    }
    let original = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let deleted = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let note = serde_json::json!({ "original": original.to_string_lossy(), "deleted": deleted });
    // The note first: an item without one could not be put back.
    std::fs::write(info.join(format!("{key}.json")), note.to_string()).map_err(|e| e.to_string())?;
    let dest = files.join(&key);
    if let Err(e) = relocate(path, &dest) {
        let _ = std::fs::remove_file(info.join(format!("{key}.json")));
        return Err(e);
    }
    Ok(dest)
}

/// What is in OfficeCraft's own trash, most recently deleted first.
pub fn own_trash_list() -> Vec<OwnItem> {
    let Some(root) = own_trash_root() else { return Vec::new() };
    let Ok(read) = std::fs::read_dir(root.join("files")) else { return Vec::new() };
    let mut items: Vec<OwnItem> = read
        .flatten()
        .filter_map(|e| {
            let key = e.file_name().to_string_lossy().to_string();
            let note: serde_json::Value = std::fs::read_to_string(root.join("info").join(format!("{key}.json"))).ok().and_then(|t| serde_json::from_str(&t).ok())?;
            let original = PathBuf::from(note.get("original")?.as_str()?);
            let deleted = note.get("deleted").and_then(serde_json::Value::as_u64).map(|s| std::time::UNIX_EPOCH + std::time::Duration::from_secs(s));
            Some(OwnItem { key, original, deleted, is_dir: e.path().is_dir() })
        })
        .collect();
    items.sort_by(|a, b| b.deleted.cmp(&a.deleted));
    items
}

/// Puts an item from OfficeCraft's trash back where it was. Refuses if something new is there.
pub fn restore(item: &OwnItem) -> Result<PathBuf, String> {
    let root = own_trash_root().ok_or("no data directory")?;
    if item.original.exists() {
        return Err(format!("{} already exists; move it away first.", item.original.display()));
    }
    if let Some(parent) = item.original.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    relocate(&root.join("files").join(&item.key), &item.original)?;
    let _ = std::fs::remove_file(root.join("info").join(format!("{}.json", item.key)));
    Ok(item.original.clone())
}

/// Deletes an item from OfficeCraft's trash for good.
pub fn purge(item: &OwnItem) -> Result<(), String> {
    let root = own_trash_root().ok_or("no data directory")?;
    let p = root.join("files").join(&item.key);
    if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) }.map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(root.join("info").join(format!("{}.json", item.key)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("oc-files-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn lists_folders_first_then_supported_files() {
        let d = tmp();
        std::fs::create_dir(d.join("b-sub")).unwrap();
        std::fs::create_dir(d.join("A-sub")).unwrap();
        std::fs::create_dir(d.join(".hidden")).unwrap();
        std::fs::write(d.join("notes.docx"), b"x").unwrap();
        std::fs::write(d.join("skip.bin"), b"x").unwrap();
        let rows = list_dir(&d, |p| p.ends_with(".docx")).unwrap();
        let names: Vec<_> = rows.iter().map(|r| (r.file_name().unwrap().to_string_lossy().to_string(), r.is_dir())).collect();
        assert_eq!(names, [("A-sub".into(), true), ("b-sub".into(), true), ("notes.docx".into(), false)]);
    }

    #[test]
    fn rename_and_new_folder_refuse_bad_names_and_clashes() {
        let d = tmp();
        std::fs::write(d.join("a.docx"), b"x").unwrap();
        std::fs::write(d.join("b.docx"), b"x").unwrap();
        assert!(rename(&d.join("a.docx"), "b.docx").is_err());
        assert!(rename(&d.join("a.docx"), "../a.docx").is_err());
        assert_eq!(rename(&d.join("a.docx"), "c.docx").unwrap(), d.join("c.docx"));
        assert!(new_folder(&d, "").is_err());
        assert!(new_folder(&d, "Sub").unwrap().is_dir());
        assert!(new_folder(&d, "Sub").is_err());
    }

    #[test]
    fn move_into_keeps_the_name_and_refuses_itself() {
        let d = tmp();
        let sub = new_folder(&d, "Sub").unwrap();
        std::fs::write(d.join("a.docx"), b"x").unwrap();
        assert_eq!(move_into(&d.join("a.docx"), &sub).unwrap(), sub.join("a.docx"));
        assert!(sub.join("a.docx").exists() && !d.join("a.docx").exists());
        assert!(move_into(&sub, &sub).is_ok_and(|p| p == sub) || move_into(&sub, &sub).is_err());
        let inner = new_folder(&sub, "Inner").unwrap();
        assert!(move_into(&sub, &inner).is_err());
    }

    #[test]
    fn own_trash_round_trip() {
        let d = tmp();
        let root = d.join("Trash");
        std::fs::write(d.join("gone.docx"), b"x").unwrap();
        let kept = to_own_trash(&root, &d.join("gone.docx")).unwrap();
        assert!(kept.exists() && !d.join("gone.docx").exists());
        assert!(root.join("info/gone.docx.json").exists());
        // A second item of the same name gets its own slot.
        std::fs::write(d.join("gone.docx"), b"y").unwrap();
        let second = to_own_trash(&root, &d.join("gone.docx")).unwrap();
        assert_ne!(kept, second);
    }
}
