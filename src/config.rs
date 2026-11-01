//! Configuración del usuario en `~/Library/Application Support/terminalapp/config.json`.
//! Todo es opcional; se relee al volver a la ventana si el archivo cambió.

use std::{path::PathBuf, time::SystemTime};

use gpui::{App, Global};
use serde::Deserialize;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Fuente de la terminal; por ejemplo una Nerd Font, para los íconos de starship o p10k.
    pub font_family: Option<String>,
    /// Editor donde se abren las rutas de la salida: `zed`, `code`, `cursor`, `nvim`… o una
    /// plantilla con `{file}`, `{line}` y `{column}`.
    pub editor: Option<String>,
    /// Líneas de scrollback de cada pestaña nueva.
    pub scrollback: Option<usize>,
    /// Abrir las ventanas, pestañas y paneles de la vez anterior (sí, si no se indica).
    pub restore_session: Option<bool>,
    /// Notificación de macOS cuando un comando largo termina con la app en segundo plano (sí,
    /// si no se indica).
    pub notifications: Option<bool>,
    /// Qué hace Tab en el prompt: la completion de la shell (por defecto) o abrir las
    /// sugerencias de la app, que si no están siempre en ⌃Space.
    pub tab_completion: Option<TabCompletion>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabCompletion {
    Shell,
    Popup,
}

/// Lo que se escribe al abrir la configuración por primera vez.
const TEMPLATE: &str = r#"{
  "font_family": null,
  "editor": null,
  "scrollback": 10000,
  "restore_session": true,
  "notifications": true,
  "tab_completion": "shell"
}
"#;

pub const MAX_SCROLLBACK: usize = 100_000;

#[derive(Default)]
struct Loaded {
    config: Config,
    modified: Option<SystemTime>,
    error: Option<String>,
}

impl Global for Loaded {}

pub fn path() -> Option<PathBuf> {
    Some(crate::state::state_dir()?.join("config.json"))
}

pub fn init(cx: &mut App) {
    cx.set_global(read());
}

pub fn get(cx: &App) -> &Config {
    &cx.global::<Loaded>().config
}

/// El error del último intento de leer el archivo, si era inválido.
pub fn error(cx: &App) -> Option<&str> {
    cx.global::<Loaded>().error.as_deref()
}

/// Relee el archivo si cambió desde la última vez; devuelve si cambió.
pub fn reload_if_changed(cx: &mut App) -> bool {
    let modified = path().and_then(|path| std::fs::metadata(path).ok()?.modified().ok());
    if cx.global::<Loaded>().modified == modified {
        return false;
    }
    cx.set_global(read());
    true
}

/// El archivo de configuración, creándolo con todas las opciones si no existe.
pub fn ensure_file() -> std::io::Result<PathBuf> {
    let path = path().ok_or_else(|| std::io::Error::other("sin HOME"))?;
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, TEMPLATE)?;
    }
    Ok(path)
}

fn read() -> Loaded {
    let Some(path) = path() else {
        return Loaded::default();
    };
    let modified = std::fs::metadata(&path)
        .ok()
        .and_then(|meta| meta.modified().ok());
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Loaded {
            modified,
            ..Loaded::default()
        };
    };
    match parse(&text) {
        Ok(config) => Loaded {
            config,
            modified,
            error: None,
        },
        Err(err) => {
            let error = format!("config.json inválido: {err}");
            crate::log::error(&error);
            Loaded {
                config: Config::default(),
                modified,
                error: Some(error),
            }
        }
    }
}

fn parse(text: &str) -> serde_json::Result<Config> {
    serde_json::from_str(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_and_unknown_keys_are_errors() {
        let config = parse(TEMPLATE).unwrap();
        assert_eq!(config.scrollback, Some(10000));
        assert_eq!(config.editor, None);
        assert_eq!(parse("{}").unwrap(), Config::default());
        assert!(parse(r#"{"editr": "zed"}"#).is_err());
        assert_eq!(
            parse(r#"{"tab_completion": "popup"}"#)
                .unwrap()
                .tab_completion,
            Some(TabCompletion::Popup)
        );
        assert!(parse(r#"{"tab_completion": "menu"}"#).is_err());
    }
}
