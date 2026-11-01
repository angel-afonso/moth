//! Opens files from the output in the user's editor, at the given line and column.

use std::path::{Path, PathBuf};

use crate::{links::FileLink, process};

/// How to open the file.
#[derive(Debug, PartialEq)]
pub enum Launch {
    /// A GUI editor (Zed, VS Code…): run it directly.
    App { program: PathBuf, args: Vec<String> },
    /// A terminal editor (vim, helix…): this command runs in a new tab.
    Terminal(String),
    /// No editor configured or detected: the system's default app for the file (no line).
    System,
    /// The configured editor isn't on the PATH.
    Missing(String),
}

/// GUI editors looked up on the PATH, in this order, when none is configured.
const DETECTED: &[&str] = &["zed", "cursor", "code", "windsurf", "subl"];
/// On macOS, the CLI bundled inside the app, for when it isn't installed on the PATH (Zed
/// without `zed`, say).
const BUNDLED_CLIS: &[(&str, &str)] = &[
    ("zed", "Zed.app/Contents/MacOS/cli"),
    (
        "code",
        "Visual Studio Code.app/Contents/Resources/app/bin/code",
    ),
    ("cursor", "Cursor.app/Contents/Resources/app/bin/cursor"),
    (
        "windsurf",
        "Windsurf.app/Contents/Resources/app/bin/windsurf",
    ),
    ("subl", "Sublime Text.app/Contents/SharedSupport/bin/subl"),
];
const TERMINAL_EDITORS: &[&str] = &["vim", "nvim", "vi", "hx", "helix", "nano", "micro", "kak"];

/// The PATH to search when the shell didn't report its own (no integration). An app launched
/// from Finder gets a minimal PATH without Homebrew, so its folders are added on macOS.
pub fn fallback_path() -> String {
    let path = std::env::var("PATH").unwrap_or_default();
    if cfg!(target_os = "macos") {
        format!("{path}:/opt/homebrew/bin:/usr/local/bin")
    } else {
        path
    }
}

/// `editor` is the configured value: a known name (`zed`, `code`, `nvim`…) or a template with
/// `{file}`, `{line}` and `{column}`. When unset, the first one found from `DETECTED`.
pub fn launch(editor: Option<&str>, file: &FileLink, search_path: &str) -> Launch {
    let app_dirs: Vec<PathBuf> = if cfg!(target_os = "macos") {
        std::iter::once(PathBuf::from("/Applications"))
            .chain(crate::state::home_dir().map(|home| home.join("Applications")))
            .collect()
    } else {
        Vec::new()
    };
    launch_in(editor, file, search_path, &app_dirs)
}

fn launch_in(
    editor: Option<&str>,
    file: &FileLink,
    search_path: &str,
    app_dirs: &[PathBuf],
) -> Launch {
    let find = |program: &str| {
        process::which(program, search_path).or_else(|| bundled_cli(program, app_dirs))
    };
    let configured = editor.map(str::trim).filter(|editor| !editor.is_empty());
    let template = match configured {
        Some(editor) => editor.to_string(),
        None => match DETECTED.iter().find(|name| find(name).is_some()) {
            Some(name) => name.to_string(),
            None => return Launch::System,
        },
    };
    let mut parts = template.split_whitespace();
    let Some(program) = parts.next() else {
        return Launch::System;
    };
    let extra: Vec<&str> = parts.collect();
    let name = Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program);
    let args: Vec<String> = if template.contains("{file}") {
        extra.iter().map(|arg| fill(arg, file)).collect()
    } else {
        extra
            .iter()
            .map(|arg| arg.to_string())
            .chain(position_args(name, file))
            .collect()
    };
    if TERMINAL_EDITORS.contains(&name) {
        let command = std::iter::once(program.to_string())
            .chain(args)
            .map(|arg| shell_quote(&arg))
            .collect::<Vec<_>>()
            .join(" ");
        return Launch::Terminal(command);
    }
    match find(program) {
        Some(program) => Launch::App { program, args },
        None => Launch::Missing(program.to_string()),
    }
}

/// Runs the editor without waiting for it.
pub fn spawn(program: &Path, args: &[String], search_path: &str) -> std::io::Result<()> {
    process::spawn_detached(
        std::process::Command::new(program)
            .args(args)
            .env("PATH", search_path),
    )
}

/// How each known editor expects to be told the line and column.
fn position_args(name: &str, file: &FileLink) -> Vec<String> {
    let path = file.path.display().to_string();
    let (Some(line), column) = (file.line, file.column) else {
        return vec![path];
    };
    let column = column.unwrap_or(1);
    let joined = format!("{path}:{line}:{column}");
    match name {
        "code" | "code-insiders" | "codium" | "cursor" | "windsurf" => {
            vec!["--goto".into(), joined]
        }
        "vim" | "nvim" | "vi" => vec![format!("+call cursor({line},{column})"), path],
        "nano" => vec![format!("+{line},{column}"), path],
        "kak" => vec![format!("+{line}:{column}"), path],
        "idea" | "webstorm" | "pycharm" | "goland" | "rustrover" | "clion" | "phpstorm" => vec![
            "--line".into(),
            line.to_string(),
            "--column".into(),
            column.to_string(),
            path,
        ],
        // zed, subl, hx, micro and anything else: `file:line:column`.
        _ => vec![joined],
    }
}

fn fill(arg: &str, file: &FileLink) -> String {
    arg.replace("{file}", &file.path.display().to_string())
        .replace("{line}", &file.line.unwrap_or(1).to_string())
        .replace("{column}", &file.column.unwrap_or(1).to_string())
}

fn bundled_cli(program: &str, app_dirs: &[PathBuf]) -> Option<PathBuf> {
    let (_, relative) = BUNDLED_CLIS.iter().find(|(name, _)| *name == program)?;
    app_dirs
        .iter()
        .map(|dir| dir.join(relative))
        .find(|path| process::is_executable(path))
}

fn shell_quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+:,=@%".contains(c))
    {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

// The fake editors below are made executable with Unix permission bits.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn file(path: &str, line: Option<u32>, column: Option<u32>) -> FileLink {
        FileLink {
            path: path.into(),
            line,
            column,
        }
    }

    /// A folder of fake executables to simulate the PATH.
    fn fake_path(programs: &[&str]) -> (PathBuf, String) {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!(
            "moth-editor-{}-{}",
            std::process::id(),
            programs.join("-")
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for program in programs {
            let path = dir.join(program);
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let search = dir.display().to_string();
        (dir, search)
    }

    #[test]
    fn known_editors_get_the_position() {
        let (dir, search) = fake_path(&["zed", "code"]);
        let f = file("/p/src/main.rs", Some(12), Some(5));
        assert_eq!(
            launch_in(Some("code"), &f, &search, &[]),
            Launch::App {
                program: dir.join("code"),
                args: vec!["--goto".into(), "/p/src/main.rs:12:5".into()]
            }
        );
        assert_eq!(
            launch_in(Some("zed"), &file("/p/a.rs", None, None), &search, &[]),
            Launch::App {
                program: dir.join("zed"),
                args: vec!["/p/a.rs".into()]
            }
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn detects_an_installed_editor_or_falls_back_to_macos() {
        let (dir, search) = fake_path(&["subl", "cursor"]);
        // `cursor` comes before `subl` in the detection order.
        assert!(
            matches!(launch_in(None, &file("/a.rs", None, None), &search, &[]), Launch::App { program, .. } if program == dir.join("cursor"))
        );
        assert_eq!(
            launch_in(None, &file("/a.rs", None, None), "/nowhere", &[]),
            Launch::System
        );
        assert_eq!(
            launch_in(Some("code"), &file("/a.rs", None, None), "/nowhere", &[]),
            Launch::Missing("code".into())
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn finds_the_cli_inside_the_app() {
        use std::os::unix::fs::PermissionsExt as _;
        let apps = std::env::temp_dir().join(format!("moth-apps-{}", std::process::id()));
        let cli = apps.join("Zed.app/Contents/MacOS/cli");
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::write(&cli, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        let f = file("/p/a.rs", Some(2), Some(4));
        assert_eq!(
            launch_in(None, &f, "/nowhere", std::slice::from_ref(&apps)),
            Launch::App {
                program: cli.clone(),
                args: vec!["/p/a.rs:2:4".into()]
            }
        );
        std::fs::remove_dir_all(apps).ok();
    }

    #[test]
    fn terminal_editors_and_templates() {
        assert_eq!(
            launch_in(
                Some("nvim"),
                &file("/p/my file.rs", Some(3), None),
                "/nowhere",
                &[]
            ),
            Launch::Terminal("nvim '+call cursor(3,1)' '/p/my file.rs'".into())
        );
        assert_eq!(
            launch_in(
                Some("hx"),
                &file("/p/a.rs", Some(3), Some(2)),
                "/nowhere",
                &[]
            ),
            Launch::Terminal("hx /p/a.rs:3:2".into())
        );
        let (dir, search) = fake_path(&["myedit"]);
        assert_eq!(
            launch_in(
                Some("myedit --open {file} -l {line}"),
                &file("/p/a.rs", Some(9), None),
                &search,
                &[]
            ),
            Launch::App {
                program: dir.join("myedit"),
                args: vec!["--open".into(), "/p/a.rs".into(), "-l".into(), "9".into()]
            }
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
