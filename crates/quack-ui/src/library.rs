//! User presets on disk: one `.qkpreset` text file per preset (format in
//! `quack_dsp::presets`), in the DuckDSP folder of the OS:
//!
//! | OS | Folder |
//! |---|---|
//! | macOS | `~/Library/Audio/Presets/DuckDSP/Quackapella/` |
//! | Windows | `%APPDATA%\DuckDSP\Quackapella\Presets\` |
//! | Linux | `$XDG_DATA_HOME/DuckDSP/Quackapella/presets/` (or `~/.local/share/…`) |
//!
//! The favorites (factory or user presets) sit beside them in
//! `favorites.txt`, one `factory:Name` or `user:Name` per line.
//!
//! Editor thread only: read when the list opens, written on save.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use quack_dsp::presets::{EXTENSION, Preset};

/// The user preset folder, `None` when the home folder is unknown.
#[must_use]
pub fn user_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join("Library/Audio/Presets/DuckDSP/Quackapella"))
    }
    #[cfg(target_os = "windows")]
    {
        let app = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(app).join("DuckDSP\\Quackapella\\Presets"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
        Some(base.join("DuckDSP/Quackapella/presets"))
    }
}

/// One user preset found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
}

/// The presets in `dir`, by name (unreadable files skipped).
#[must_use]
pub fn list(dir: &Path) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Entry> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == EXTENSION))
        .filter_map(|path| {
            let p = load(&path)?;
            Some(Entry { name: p.name, path })
        })
        .collect();
    out.sort_by_key(|e| e.name.to_lowercase());
    out
}

#[must_use]
pub fn load(path: &Path) -> Option<Preset> {
    Preset::parse(&std::fs::read_to_string(path).ok()?)
}

/// File name for a preset name: letters, digits, spaces, `-` and `_`.
#[must_use]
pub fn file_name(name: &str) -> String {
    let clean: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean = clean.trim();
    format!(
        "{}.{EXTENSION}",
        if clean.is_empty() { "Preset" } else { clean }
    )
}

/// Write `preset` into `dir` (created if needed), replacing a preset of
/// the same name.
///
/// # Errors
/// Any I/O error.
pub fn save(dir: &Path, preset: &Preset) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(file_name(&preset.name));
    std::fs::write(&path, preset.to_text())?;
    Ok(path)
}

/// Find a user preset by name.
#[must_use]
pub fn find(dir: &Path, name: &str) -> Option<Preset> {
    let direct = dir.join(file_name(name));
    if let Some(p) = load(&direct).filter(|p| p.name == name) {
        return Some(p);
    }
    list(dir)
        .into_iter()
        .find(|e| e.name == name)
        .and_then(|e| load(&e.path))
}

/// Favorites file in the user folder: one key per line.
const FAVORITES: &str = "favorites.txt";

/// Favorite key of a factory preset.
#[must_use]
pub fn factory_key(name: &str) -> String {
    format!("factory:{name}")
}

/// Favorite key of a user preset.
#[must_use]
pub fn user_key(name: &str) -> String {
    format!("user:{name}")
}

/// The favorites saved in `dir` (none when the file is missing).
#[must_use]
pub fn favorites(dir: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(dir.join(FAVORITES))
        .map(|t| {
            t.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Write the favorites into `dir` (created if needed).
///
/// # Errors
/// Any I/O error.
pub fn save_favorites(dir: &Path, favs: &BTreeSet<String>) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut t = String::from("# Quackapella favorite presets\n");
    for f in favs {
        t.push_str(f);
        t.push('\n');
    }
    std::fs::write(dir.join(FAVORITES), t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_list_and_find() {
        let dir = std::env::temp_dir().join(format!("quack-presets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut p = quack_dsp::presets::FACTORY[3].to_preset();
        p.name = "Mon/preset: 1".to_owned();
        p.category = "User".to_owned();
        let path = save(&dir, &p).expect("save");
        assert_eq!(path.file_name().unwrap(), "Mon_preset_ 1.qkpreset");
        let l = list(&dir);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].name, "Mon/preset: 1");
        assert_eq!(find(&dir, "Mon/preset: 1"), Some(p));
        // Favorites live beside the presets.
        assert!(favorites(&dir).is_empty());
        let favs: BTreeSet<String> = [factory_key("Radio Pop"), user_key("Mon/preset: 1")].into();
        save_favorites(&dir, &favs).expect("favorites");
        assert_eq!(favorites(&dir), favs);
        assert_eq!(list(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
