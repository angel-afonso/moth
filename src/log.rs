//! Errores de la app. Abierta desde el Finder no hay terminal donde ver stderr, así que además
//! se guardan en `~/Library/Logs/moth/moth.log`.

use std::{fmt::Display, io::Write as _, path::PathBuf};

/// Al pasar este tamaño, el log pasa a `moth.log.old` y se empieza uno nuevo.
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
