//! App errors. When launched from Finder, the Start menu or a desktop launcher there's no
//! terminal to show stderr, so they're also written to `moth.log` in `state::log_dir`
//! (`~/Library/Logs/moth` on macOS).

use std::{fmt::Display, io::Write as _, path::PathBuf};

/// Forwards what gpui and other libraries log (renderer and platform failures, mostly) to
/// stderr, and errors also to the log file. `MOTH_LOG` (`error`, `warn`, `info`, `debug`,
/// `trace`) sets how much; warnings and errors by default.
pub fn init() {
    let level = std::env::var("MOTH_LOG")
        .ok()
        .and_then(|level| level.parse().ok())
        .unwrap_or(::log::LevelFilter::Warn);
    if ::log::set_logger(&LOGGER).is_ok() {
        ::log::set_max_level(level);
    }
}

struct Logger;

static LOGGER: Logger = Logger;

impl ::log::Log for Logger {
    fn enabled(&self, metadata: &::log::Metadata) -> bool {
        metadata.level() <= ::log::max_level()
    }

    fn log(&self, record: &::log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let message = format!("{} {}: {}", record.level(), record.target(), record.args());
        if record.level() == ::log::Level::Error {
            error(message);
        } else {
            eprintln!("{message}");
        }
    }

    fn flush(&self) {}
}

/// Past this size, the log is moved to `moth.log.old` and a new one is started.
const MAX_LOG_BYTES: u64 = 1024 * 1024;

pub fn log_path() -> Option<PathBuf> {
    Some(crate::state::log_dir()?.join("moth.log"))
}

pub fn error(message: impl Display) {
    eprintln!("{message}");
    // Tests that check error paths (a broken theme, say) would otherwise fill the user's log.
    if cfg!(test) {
        return;
    }
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
