//! App errors. When launched from Finder there's no terminal to show stderr, so they're also
//! written to `~/Library/Logs/moth/moth.log`.

use std::{fmt::Display, io::Write as _, path::PathBuf};

/// Past this size, the log is moved to `moth.log.old` and a new one is started.
const MAX_LOG_BYTES: u64 = 1024 * 1024;

pub fn log_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Logs/moth/moth.log"))
}

pub fn error(message: impl Display) {
    eprintln!("{message}");
    let Some(path) = log_path() else {
        return;
    };
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX_LOG_BYTES) {
            std::fs::rename(&path, path.with_extension("log.old"))?;
        }
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(file, "[{secs}] {message}")
    };
    write().ok();
}
