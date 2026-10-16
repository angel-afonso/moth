//! Estado que sobrevive entre ejecuciones: por ahora, la última carpeta usada.

use std::path::{Path, PathBuf};

fn state_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Application Support/terminalapp"))
}

/// La última carpeta guardada, si todavía existe.
pub fn last_cwd() -> Option<PathBuf> {
    read_last_cwd(&state_dir()?)
}

pub fn save_last_cwd(cwd: &Path) {
    if let Some(dir) = state_dir()
        && let Err(err) = write_last_cwd(&dir, cwd)
    {
        eprintln!("no se pudo guardar la última carpeta: {err}");
    }
}

fn read_last_cwd(dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(dir.join("last_cwd")).ok()?;
    let path = PathBuf::from(text.trim_end_matches('\n'));
    path.is_dir().then_some(path)
}

fn write_last_cwd(dir: &Path, cwd: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    // Escribe a un temporal y renombra, para no dejar el archivo a medias.
    let tmp = dir.join("last_cwd.tmp");
    std::fs::write(&tmp, cwd.as_os_str().as_encoded_bytes())?;
    std::fs::rename(tmp, dir.join("last_cwd"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_ignores_missing_dirs() {
        let dir = std::env::temp_dir().join(format!("tapp-state-{}", std::process::id()));
        let existing = dir.join("proyecto con espacios");
        std::fs::create_dir_all(&existing).unwrap();

        assert_eq!(read_last_cwd(&dir), None);
        write_last_cwd(&dir, &existing).unwrap();
        assert_eq!(read_last_cwd(&dir), Some(existing.clone()));

        write_last_cwd(&dir, &dir.join("ya-no-existe")).unwrap();
        assert_eq!(read_last_cwd(&dir), None);

        std::fs::remove_dir_all(dir).ok();
    }
}
