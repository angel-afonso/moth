//! State that persists across runs: the last folder, recent folders and the theme.

use std::path::{Path, PathBuf};

pub fn state_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Application Support/moth"))
}

/// The app used to be called terminalapp: if its folder is still around and there's no Moth
/// folder yet, move the whole thing over (settings, themes, hosts, session, recent folders) so
/// nothing is lost.
pub fn migrate_legacy_dir() {
    let (Some(home), Some(dir)) = (std::env::var_os("HOME"), state_dir()) else {
        return;
    };
    let legacy = Path::new(&home).join("Library/Application Support/terminalapp");
    if let Err(err) = migrate(&legacy, &dir) {
        crate::log::error(format!(
            "couldn't move {} to {}: {err}",
            legacy.display(),
            dir.display()
        ));
    }
}

fn migrate(legacy: &Path, dir: &Path) -> std::io::Result<()> {
    if !legacy.is_dir() || dir.exists() {
        return Ok(());
    }
    std::fs::rename(legacy, dir)?;
    // The zsh integration gets reinstalled under the new name.
    std::fs::remove_file(dir.join("shell/zsh/terminalapp.zsh")).ok();
    Ok(())
}

/// The last saved folder, if it still exists.
pub fn last_cwd() -> Option<PathBuf> {
    read_last_cwd(&state_dir()?)
}

const MAX_RECENT_DIRS: usize = 15;

/// Folders used, most recent first, only those that still exist.
pub fn recent_dirs() -> Vec<PathBuf> {
    state_dir().map(|dir| read_recent(&dir)).unwrap_or_default()
}

/// Saves `cwd` as the last folder and moves it to the front of the recent ones.
pub fn record_dir(cwd: &Path) {
    if let Some(dir) = state_dir() {
        let result = write_last_cwd(&dir, cwd).and_then(|()| write_recent(&dir, cwd));
        if let Err(err) = result {
            crate::log::error(format!("couldn't save the current folder: {err}"));
        }
    }
}

/// Name of the theme picked last time.
pub fn saved_theme() -> Option<String> {
    let text = std::fs::read_to_string(state_dir()?.join("theme")).ok()?;
    Some(text.trim().to_string()).filter(|name| !name.is_empty())
}

pub fn save_theme(name: &str) {
    let Some(dir) = state_dir() else {
        return;
    };
    let result =
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join("theme"), name));
    if let Err(err) = result {
        crate::log::error(format!("couldn't save the theme: {err}"));
    }
}

fn read_recent(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(dir.join("recent_dirs"))
        .unwrap_or_default()
        .lines()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .collect()
}

fn write_recent(dir: &Path, cwd: &Path) -> std::io::Result<()> {
    let mut dirs = read_recent(dir);
    dirs.retain(|d| d != cwd);
    dirs.insert(0, cwd.to_path_buf());
    dirs.truncate(MAX_RECENT_DIRS);
    let text: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
    let tmp = dir.join("recent_dirs.tmp");
    std::fs::write(&tmp, text.join("\n"))?;
    std::fs::rename(tmp, dir.join("recent_dirs"))
}

fn read_last_cwd(dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(dir.join("last_cwd")).ok()?;
    let path = PathBuf::from(text.trim_end_matches('\n'));
    path.is_dir().then_some(path)
}

fn write_last_cwd(dir: &Path, cwd: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    // Write to a temp file and rename it, so the file is never left half-written.
    let tmp = dir.join("last_cwd.tmp");
    std::fs::write(&tmp, cwd.as_os_str().as_encoded_bytes())?;
    std::fs::rename(tmp, dir.join("last_cwd"))
}

/// Terminal text size, validated before use.
pub fn saved_font_size() -> f32 {
    state_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join("font_size")).ok())
        .and_then(|text| text.trim().parse::<f32>().ok())
        .filter(|size| size.is_finite() && (8.0..=32.0).contains(size))
        .unwrap_or(13.5)
}

pub fn save_font_size(size: f32) {
    if let Some(dir) = state_dir()
        && let Err(err) = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(dir.join("font_size"), size.to_string()))
    {
        crate::log::error(format!("couldn't save the text size: {err}"));
    }
}

/// Whether ⌥ acts as Meta in the terminal; off unless it was ever turned on.
pub fn option_as_meta() -> bool {
    state_dir().is_some_and(|dir| dir.join("option_as_meta").exists())
}

pub fn save_option_as_meta(enabled: bool) {
    let Some(dir) = state_dir() else {
        return;
    };
    let path = dir.join("option_as_meta");
    let result = if enabled {
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, ""))
    } else {
        std::fs::remove_file(&path).or_else(|err| match err.kind() {
            std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(err),
        })
    };
    if let Err(err) = result {
        crate::log::error(format!("couldn't save Option as Meta: {err}"));
    }
}

/// Folders of closed tabs; commands are neither saved nor re-run.
pub fn closed_dirs() -> Vec<PathBuf> {
    state_dir()
        .and_then(|dir| std::fs::read(dir.join("closed_tabs.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<Vec<PathBuf>>(&bytes).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|dir| dir.is_dir())
        .rev()
        .take(20)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

pub fn save_closed_dirs(dirs: &[PathBuf]) {
    let Some(dir) = state_dir() else {
        return;
    };
    let result = (|| -> anyhow::Result<()> {
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join("closed_tabs.tmp");
        std::fs::write(&tmp, serde_json::to_vec(dirs)?)?;
        std::fs::rename(tmp, dir.join("closed_tabs.json"))?;
        Ok(())
    })();
    if let Err(err) = result {
        crate::log::error(format!("couldn't save the closed tabs: {err}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_the_old_folder_only_once() {
        let root = std::env::temp_dir().join(format!("moth-migrate-{}", std::process::id()));
        let legacy = root.join("terminalapp");
        let dir = root.join("moth");
        std::fs::create_dir_all(legacy.join("shell/zsh")).unwrap();
        std::fs::write(legacy.join("config.json"), "{}").unwrap();
        std::fs::write(legacy.join("shell/zsh/terminalapp.zsh"), "").unwrap();
        migrate(&legacy, &dir).unwrap();
        assert!(!legacy.exists());
        assert!(dir.join("config.json").exists());
        assert!(!dir.join("shell/zsh/terminalapp.zsh").exists());
        // If a Moth folder already exists, the old one doesn't overwrite it.
        std::fs::create_dir_all(&legacy).unwrap();
        migrate(&legacy, &dir).unwrap();
        assert!(legacy.exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn round_trips_and_ignores_missing_dirs() {
        let dir = std::env::temp_dir().join(format!("moth-state-{}", std::process::id()));
        let existing = dir.join("project with spaces");
        std::fs::create_dir_all(&existing).unwrap();

        assert_eq!(read_last_cwd(&dir), None);
        write_last_cwd(&dir, &existing).unwrap();
        assert_eq!(read_last_cwd(&dir), Some(existing.clone()));

        write_last_cwd(&dir, &dir.join("no-longer-exists")).unwrap();
        assert_eq!(read_last_cwd(&dir), None);

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn keeps_recent_dirs_unique_and_most_recent_first() {
        let dir = std::env::temp_dir().join(format!("moth-recent-{}", std::process::id()));
        let a = dir.join("a");
        let b = dir.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();

        write_recent(&dir, &a).unwrap();
        write_recent(&dir, &b).unwrap();
        write_recent(&dir, &a).unwrap();
        assert_eq!(read_recent(&dir), vec![a, b]);

        std::fs::remove_dir_all(dir).ok();
    }
}
