//! Finding and starting programs the same way on macOS, Linux and Windows.

use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Whether `path` is a program that can be run: an executable file on Unix, a file with one of
/// `PATHEXT`'s extensions (`.exe`, `.cmd`…) on Windows.
pub fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(windows)]
    {
        let extension = path.extension().and_then(|ext| ext.to_str());
        path.is_file()
            && extension.is_some_and(|ext| {
                windows_extensions()
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(ext))
            })
    }
}

/// The extensions Windows runs without being told, from `PATHEXT`.
#[cfg(windows)]
fn windows_extensions() -> Vec<String> {
    std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .filter_map(|ext| ext.strip_prefix('.'))
        .map(str::to_string)
        .collect()
}

/// `program` inside `dir`: the file itself or, on Windows, the file with one of `PATHEXT`'s
/// extensions (`git` → `git.exe`).
pub fn find_in_dir(dir: &Path, program: &str) -> Option<PathBuf> {
    let path = dir.join(program);
    if is_executable(&path) {
        return Some(path);
    }
    #[cfg(windows)]
    if Path::new(program).extension().is_none() {
        return windows_extensions()
            .iter()
            .map(|ext| dir.join(format!("{program}.{ext}")))
            .find(|path| is_executable(path));
    }
    None
}

/// The name a program is typed by: its file name, without the extension on Windows
/// (`git.exe` → `git`).
pub fn command_name(path: &Path) -> Option<String> {
    let name = if cfg!(windows) {
        path.file_stem()
    } else {
        path.file_name()
    };
    Some(name?.to_string_lossy().into_owned())
}

/// `program` in the folders of `search_path` (a `PATH` value), or the program itself when it's
/// given as a path.
pub fn which(program: &str, search_path: &str) -> Option<PathBuf> {
    if Path::new(program).components().count() > 1 {
        let path = PathBuf::from(program);
        return is_executable(&path).then_some(path);
    }
    std::env::split_paths(search_path).find_map(|dir| find_in_dir(&dir, program))
}

/// Starts `command` without standard streams (and without a console window on Windows), and
/// reaps it on another thread so it doesn't linger as a zombie.
pub fn spawn_detached(command: &mut Command) -> io::Result<()> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_programs_in_the_search_path() {
        let dir = std::env::temp_dir().join(format!("moth-process-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A program as each platform recognizes one.
        #[cfg(unix)]
        let program = {
            use std::os::unix::fs::PermissionsExt as _;
            let path = dir.join("tool");
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        #[cfg(windows)]
        let program = {
            let path = dir.join("tool.exe");
            std::fs::write(&path, "").unwrap();
            path
        };
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        let search = std::env::join_paths([dir.clone()]).unwrap();
        let search = search.to_str().unwrap();
        assert_eq!(which("tool", search), Some(program.clone()));
        assert_eq!(which(program.to_str().unwrap(), ""), Some(program));
        assert_eq!(which("notes.txt", search), None);
        assert_eq!(which("missing", search), None);
        std::fs::remove_dir_all(dir).ok();
    }
}
