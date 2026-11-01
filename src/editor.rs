//! Abrir archivos de la salida en el editor del usuario, en la línea y columna indicadas.

use std::path::{Path, PathBuf};

use crate::links::FileLink;

/// Cómo abrir el archivo.
#[derive(Debug, PartialEq)]
pub enum Launch {
    /// Un editor con ventana propia (Zed, VS Code…): se ejecuta directo.
    App { program: PathBuf, args: Vec<String> },
    /// Un editor de terminal (vim, helix…): este comando va en una pestaña nueva.
    Terminal(String),
    /// Sin editor configurado ni detectado: la app predeterminada de macOS (sin línea).
    System,
    /// El editor configurado no está en el PATH.
    Missing(String),
}

/// Editores con ventana que se buscan en el PATH si no hay uno configurado, en este orden.
const DETECTED: &[&str] = &["zed", "cursor", "code", "windsurf", "subl"];
/// La CLI dentro de la app, para cuando no se instaló en el PATH (Zed sin `zed`, por ejemplo).
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

/// El PATH de una shell sin integración o abierta desde el Finder no trae Homebrew.
pub fn fallback_path() -> String {
    let path = std::env::var("PATH").unwrap_or_default();
    format!("{path}:/opt/homebrew/bin:/usr/local/bin")
}

/// `editor` es lo que dice la configuración: un nombre conocido (`zed`, `code`, `nvim`…) o una
/// plantilla con `{file}`, `{line}` y `{column}`. Sin configurar, el primero de `DETECTED`.
pub fn launch(editor: Option<&str>, file: &FileLink, search_path: &str) -> Launch {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let app_dirs: Vec<PathBuf> = std::iter::once(PathBuf::from("/Applications"))
        .chain(home.map(|home| home.join("Applications")))
        .collect();
    launch_in(editor, file, search_path, &app_dirs)
}

fn launch_in(
    editor: Option<&str>,
    file: &FileLink,
    search_path: &str,
    app_dirs: &[PathBuf],
) -> Launch {
    let find =
        |program: &str| which(program, search_path).or_else(|| bundled_cli(program, app_dirs));
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

/// Ejecuta el editor sin esperar a que termine (se espera en otro hilo para no dejar zombis).
pub fn spawn(program: &Path, args: &[String], search_path: &str) -> std::io::Result<()> {
    let mut child = std::process::Command::new(program)
        .args(args)
        .env("PATH", search_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Cómo pide cada editor conocido abrir en una línea y columna.
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
        // zed, subl, hx, micro y cualquier otro: `archivo:línea:columna`.
        _ => vec![joined],
    }
}

fn fill(arg: &str, file: &FileLink) -> String {
    arg.replace("{file}", &file.path.display().to_string())
        .replace("{line}", &file.line.unwrap_or(1).to_string())
        .replace("{column}", &file.column.unwrap_or(1).to_string())
}

fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

fn bundled_cli(program: &str, app_dirs: &[PathBuf]) -> Option<PathBuf> {
    let (_, relative) = BUNDLED_CLIS.iter().find(|(name, _)| *name == program)?;
    app_dirs
        .iter()
        .map(|dir| dir.join(relative))
        .find(|path| executable(path))
}

fn which(program: &str, search_path: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let path = PathBuf::from(program);
        return executable(&path).then_some(path);
    }
    std::env::split_paths(search_path)
        .map(|dir| dir.join(program))
        .find(|path| executable(path))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, line: Option<u32>, column: Option<u32>) -> FileLink {
        FileLink {
            path: path.into(),
            line,
            column,
        }
    }

    /// Una carpeta con ejecutables falsos para simular el PATH.
    fn fake_path(programs: &[&str]) -> (PathBuf, String) {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!(
            "tapp-editor-{}-{}",
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
        // `cursor` va antes que `subl` en el orden de detección.
        assert!(
            matches!(launch_in(None, &file("/a.rs", None, None), &search, &[]), Launch::App { program, .. } if program == dir.join("cursor"))
        );
        assert_eq!(
            launch_in(None, &file("/a.rs", None, None), "/nada", &[]),
            Launch::System
        );
        assert_eq!(
            launch_in(Some("code"), &file("/a.rs", None, None), "/nada", &[]),
            Launch::Missing("code".into())
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn finds_the_cli_inside_the_app() {
        use std::os::unix::fs::PermissionsExt as _;
        let apps = std::env::temp_dir().join(format!("tapp-apps-{}", std::process::id()));
        let cli = apps.join("Zed.app/Contents/MacOS/cli");
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::write(&cli, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        let f = file("/p/a.rs", Some(2), Some(4));
        assert_eq!(
            launch_in(None, &f, "/nada", std::slice::from_ref(&apps)),
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
                &file("/p/mi archivo.rs", Some(3), None),
                "/nada",
                &[]
            ),
            Launch::Terminal("nvim '+call cursor(3,1)' '/p/mi archivo.rs'".into())
        );
        assert_eq!(
            launch_in(Some("hx"), &file("/p/a.rs", Some(3), Some(2)), "/nada", &[]),
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
