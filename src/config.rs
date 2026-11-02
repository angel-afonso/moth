//! User settings in `config.json` inside `state::state_dir` (`~/Library/Application
//! Support/moth` on macOS, `%APPDATA%\moth` on Windows, `~/.config/moth` on Linux).
//! Everything is optional; the file is reloaded when the window regains focus if it changed.

use std::{collections::BTreeMap, path::PathBuf, time::SystemTime};

use gpui::{App, Global};
use serde::Deserialize;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Terminal font; for example a Nerd Font, for starship or p10k icons.
    pub font_family: Option<String>,
    /// Editor that opens paths from the output: `zed`, `code`, `cursor`, `nvim`… or a template
    /// with `{file}`, `{line}` and `{column}`.
    pub editor: Option<String>,
    /// The shell each new tab runs, as a command line (`zsh`, `nu`, `cmd.exe`, `wsl.exe`…);
    /// unset, `$SHELL` on macOS and Linux and PowerShell on Windows.
    pub shell: Option<String>,
    /// Scrollback lines for each new tab.
    pub scrollback: Option<usize>,
    /// Reopen the previous session's windows, tabs and panes (yes by default).
    pub restore_session: Option<bool>,
    /// A desktop notification when a long command finishes while the app is in the background
    /// (yes by default).
    pub notifications: Option<bool>,
    /// What Tab does at the prompt: the shell's completion (default) or open the app's
    /// suggestions, which are otherwise always on ⌃Space.
    pub tab_completion: Option<TabCompletion>,
    /// Look for a new release at startup and every few hours, download it in the background
    /// and offer the restart (yes by default; release builds only).
    pub check_updates: Option<bool>,
    /// The ACP agent that suggests commands: `"claude"`, `"codex"` or `"gemini"`, an object with
    /// the command to run, or `false` to turn it off; unset, the first one found on the shell's
    /// PATH (see `agent::resolve`).
    pub agent: Option<AgentSetting>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum AgentSetting {
    Enabled(bool),
    Kind(AgentKind),
    Command(AgentCommandConfig),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
    Gemini,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCommandConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabCompletion {
    Shell,
    Popup,
}

/// What gets written the first time the settings are opened.
const TEMPLATE: &str = r#"{
  "font_family": null,
  "editor": null,
  "shell": null,
  "scrollback": 10000,
  "restore_session": true,
  "notifications": true,
  "tab_completion": "shell",
  "check_updates": true,
  "agent": null
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

/// The error from the last attempt to read the file, if it was invalid.
pub fn error(cx: &App) -> Option<&str> {
    cx.global::<Loaded>().error.as_deref()
}

/// Reloads the file if it changed since last time; returns whether it changed.
pub fn reload_if_changed(cx: &mut App) -> bool {
    let modified = path().and_then(|path| std::fs::metadata(path).ok()?.modified().ok());
    if cx.global::<Loaded>().modified == modified {
        return false;
    }
    cx.set_global(read());
    true
}

/// The settings file, created with every option if it doesn't exist.
pub fn ensure_file() -> std::io::Result<PathBuf> {
    let path = path().ok_or_else(|| std::io::Error::other("HOME not set"))?;
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
            let error = format!("invalid config.json: {err}");
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
        assert_eq!(
            parse(r#"{"agent": false}"#).unwrap().agent,
            Some(AgentSetting::Enabled(false))
        );
        assert_eq!(
            parse(r#"{"agent": "codex"}"#).unwrap().agent,
            Some(AgentSetting::Kind(AgentKind::Codex))
        );
        assert!(parse(r#"{"agent": "copilot"}"#).is_err());
        let Some(AgentSetting::Command(agent)) =
            parse(r#"{"agent": {"command": "gemini", "args": ["--acp"]}}"#)
                .unwrap()
                .agent
        else {
            panic!("expected a command");
        };
        assert_eq!(agent.args, ["--acp"]);
        assert!(parse(r#"{"agent": {"comand": "gemini"}}"#).is_err());
    }
}
