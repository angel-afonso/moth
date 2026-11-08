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
    /// The agent's model and its settings (see `AgentOptions`); unset, the quickest ones.
    pub agent_options: AgentOptions,
}

/// How the agent answers. By default its lightest model, without reasoning where the agent
/// allows it and at its lowest effort otherwise, so the answer comes in a second or two.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AgentOptions {
    /// A model the agent offers, by its id or its name (`"sonnet"`, `"6 Luna"`), or `"auto"`
    /// (default) for the lightest.
    pub model: Option<String>,
    /// Let the model reason before answering (no by default). Off, Claude runs without
    /// thinking; agents that can't turn it off use their lowest effort.
    pub reasoning: Option<bool>,
    /// A reasoning effort the model offers (`"low"`, `"medium"`…), or `"auto"` (default) for
    /// the lowest.
    pub effort: Option<String>,
    /// Any other option the agent announces, by its id: `"fast": "on"`.
    #[serde(flatten)]
    pub other: BTreeMap<String, AgentOptionValue>,
}

impl AgentOptions {
    pub fn reasoning(&self) -> bool {
        self.reasoning.unwrap_or(false)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum AgentOptionValue {
    Bool(bool),
    Text(String),
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
  "agent": null,
  "agent_options": { "model": "auto", "reasoning": false, "effort": "auto" }
}
"#;

pub const MAX_SCROLLBACK: usize = 100_000;

#[derive(Default)]
struct Loaded {
    config: Config,
    modified: Option<SystemTime>,
    error: Option<String>,
    /// The file isn't a JSON object, so nothing in it could be used.
    unreadable: bool,
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

/// Reloads the file if it changed since last time; returns whether it changed. A file that
/// isn't JSON at all (most likely half-edited) keeps the settings in use.
pub fn reload_if_changed(cx: &mut App) -> bool {
    let modified = path().and_then(|path| std::fs::metadata(path).ok()?.modified().ok());
    if cx.global::<Loaded>().modified == modified {
        return false;
    }
    let mut loaded = read();
    if loaded.unreadable {
        loaded.config = cx.global::<Loaded>().config.clone();
    }
    cx.set_global(loaded);
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
    let (config, problems, unreadable) = match parse_lenient(&text) {
        Ok((config, problems)) => (config, problems, false),
        Err(err) => (Config::default(), vec![err.to_string()], true),
    };
    let error = (!problems.is_empty()).then(|| {
        let error = format!("invalid config.json: {}", problems.join("; "));
        crate::log::error(&error);
        error
    });
    Loaded {
        config,
        modified,
        error,
        unreadable,
    }
}

#[cfg(test)]
fn parse(text: &str) -> serde_json::Result<Config> {
    serde_json::from_str(text)
}

/// The settings in `text`, leaving out (and describing) each one that's unknown or invalid,
/// so a typo or a key from a newer version doesn't reset the rest (`"agent": false` included).
/// An error only if `text` isn't a JSON object.
fn parse_lenient(text: &str) -> serde_json::Result<(Config, Vec<String>)> {
    let object: serde_json::Map<String, serde_json::Value> = serde_json::from_str(text)?;
    let mut valid = serde_json::Map::new();
    let mut problems = Vec::new();
    for (key, value) in object {
        let single =
            serde_json::Value::Object(serde_json::Map::from_iter([(key.clone(), value.clone())]));
        match serde_json::from_value::<Config>(single) {
            Ok(_) => {
                valid.insert(key, value);
            }
            Err(err) if err.to_string().starts_with("unknown field") => {
                problems.push(format!("unknown setting \"{key}\""));
            }
            Err(err) => problems.push(format!("\"{key}\": {err}")),
        }
    }
    let config = serde_json::from_value(serde_json::Value::Object(valid))?;
    Ok((config, problems))
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

        assert!(!config.agent_options.reasoning());
        let options = parse(
            r#"{"agent_options": {"model": "sonnet", "reasoning": true, "effort": "medium", "fast": "on", "verbose": false}}"#,
        )
        .unwrap()
        .agent_options;
        assert_eq!(options.model.as_deref(), Some("sonnet"));
        assert!(options.reasoning());
        assert_eq!(options.effort.as_deref(), Some("medium"));
        assert_eq!(
            options.other.get("fast"),
            Some(&AgentOptionValue::Text("on".into()))
        );
        assert_eq!(
            options.other.get("verbose"),
            Some(&AgentOptionValue::Bool(false))
        );
    }

    #[test]
    fn one_bad_setting_keeps_the_others() {
        let (config, problems) = parse_lenient(
            r#"{"agent": false, "check_updates": false, "editr": "zed", "scrollback": "lots"}"#,
        )
        .unwrap();
        assert_eq!(config.agent, Some(AgentSetting::Enabled(false)));
        assert_eq!(config.check_updates, Some(false));
        assert_eq!(config.scrollback, None);
        assert_eq!(problems.len(), 2);
        assert!(problems.iter().any(|p| p == "unknown setting \"editr\""));
        assert!(problems.iter().any(|p| p.starts_with("\"scrollback\":")));
        assert!(parse_lenient("{ \"agent\": fal").is_err());
        assert!(parse_lenient("[]").is_err());
    }
}
