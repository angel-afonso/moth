//! Completions for the line being edited: what can be typed at the cursor position.
//!
//! The sources are the specs in `assets/completions/specs.json` (each tool's subcommands,
//! flags and argument type), commands on the PATH, git branches, `package.json` scripts,
//! SSH hosts, paths and history. Everything external comes in through `Context`, so the
//! engine can be tested without touching the disk.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use gpui::{App, Global};
use serde::Deserialize;

use crate::{assets::Assets, components::SuggestionKind, fuzzy::fuzzy_match};

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Spec {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub usage: Option<String>,
    /// What the arguments expect: `path`, `dir`, `branch`, `script`, `host`, `command`.
    #[serde(default)]
    pub args: Option<String>,
    #[serde(default)]
    pub flags: BTreeMap<String, String>,
    #[serde(default)]
    pub subcommands: BTreeMap<String, Spec>,
}

#[derive(Default)]
pub struct Specs(pub HashMap<String, Spec>);

impl Global for Specs {}

pub fn load_specs() -> Specs {
    Assets::get("completions/specs.json")
        .and_then(|file| serde_json::from_slice(&file.data).ok())
        .map(Specs)
        .unwrap_or_default()
}

pub fn init(cx: &mut App) {
    cx.set_global(load_specs());
}

/// Documentation for the side pane.
#[derive(Clone, Debug, PartialEq)]
pub struct Doc {
    pub title: String,
    pub description: Option<String>,
    pub usage: Option<String>,
    pub flags: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// What gets written to the line.
    pub text: String,
    /// What's displayed (may differ from `text`, e.g. for paths with spaces).
    pub label: String,
    pub kind: SuggestionKind,
    pub description: Option<String>,
    pub doc: Option<Doc>,
    /// Characters of `label` that match the typed text (byte offsets), for highlighting.
    pub positions: Vec<usize>,
    pub score: i32,
    /// Byte offset in the line where replacement starts (the word start, or 0 for history).
    pub replace_from: usize,
    /// Append a space afterwards (not for folders, which continue with `/`).
    pub append_space: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    /// The word being completed.
    pub token: String,
    pub items: Vec<Candidate>,
}

pub struct Context<'a> {
    pub buffer: &'a str,
    /// Cursor position in bytes.
    pub cursor: usize,
    pub cwd: Option<&'a Path>,
    pub specs: &'a Specs,
    pub branches: &'a [String],
    pub current_branch: Option<&'a str>,
    /// History, most recent last.
    pub history: &'a [String],
    /// SSH hosts: (alias, description). Only asked for when a host is what can be typed.
    pub hosts: &'a dyn Fn() -> Vec<(String, String)>,
    /// PATH commands, aliases and functions that start with the prefix.
    pub commands: &'a dyn Fn(&str) -> Vec<String>,
    /// Entries of a folder: (name, is folder).
    pub list_dir: &'a dyn Fn(&Path) -> Vec<(String, bool)>,
    /// `package.json` scripts in the current folder: (name, command).
    pub scripts: &'a dyn Fn() -> Vec<(String, String)>,
}

const PRECOMMANDS: &[&str] = &[
    "sudo", "env", "time", "nohup", "command", "exec", "noglob", "nice",
];
const MAX_ITEMS: usize = 60;

/// Words of a segment with their start offsets, respecting quotes.
fn words(text: &str, offset: usize) -> Vec<(usize, String)> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut start = None;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if escaped {
            current.push(c);
            escaped = false;
            continue;
        }
        match (quote, c) {
            (_, '\\') if quote != Some('\'') => {
                escaped = true;
                start.get_or_insert(offset + i);
            }
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                start.get_or_insert(offset + i);
            }
            (None, c) if c.is_whitespace() => {
                if let Some(s) = start.take() {
                    words.push((s, std::mem::take(&mut current)));
                }
            }
            (None, c) => {
                current.push(c);
                start.get_or_insert(offset + i);
            }
        }
    }
    if let Some(s) = start {
        words.push((s, current));
    }
    words
}

/// Start of the last command on the line (after `|`, `;`, `&&`, `||`, `(`).
fn segment_start(text: &str) -> usize {
    let mut start = 0;
    let mut quote: Option<char> = None;
    for (i, c) in text.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '|' | ';' | '&' | '(') => start = i + 1,
            _ => {}
        }
    }
    start
}

pub fn escape(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_whitespace() || "'\"\\$`!&;|()<>*?[]#~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn expand(dir: &str, cwd: Option<&Path>) -> PathBuf {
    if let Some(rest) = dir.strip_prefix('~')
        && let Some(home) = crate::state::home_dir()
    {
        return home.join(rest.trim_start_matches('/'));
    }
    let path = Path::new(dir);
    match cwd {
        Some(cwd) if path.is_relative() => cwd.join(path),
        _ => path.to_path_buf(),
    }
}

fn spec_doc(title: String, spec: &Spec) -> Doc {
    Doc {
        title,
        description: spec.description.clone(),
        usage: spec.usage.clone(),
        flags: spec
            .flags
            .iter()
            .take(6)
            .map(|(f, d)| (f.clone(), d.clone()))
            .collect(),
    }
}

struct Builder<'a> {
    token: &'a str,
    replace_from: usize,
    items: Vec<Candidate>,
}

impl Builder<'_> {
    /// Adds a candidate if it matches the typed text. With an empty word, everything matches.
    fn push(
        &mut self,
        text: String,
        label: String,
        kind: SuggestionKind,
        description: Option<String>,
        doc: Option<Doc>,
        append_space: bool,
    ) {
        let query = self.token.trim_start_matches(['-']);
        let target = label.trim_start_matches(['-']);
        let Some(m) = fuzzy_match(query, target) else {
            return;
        };
        // The matcher works without the leading dashes, so positions are shifted back.
        let shift = label.len() - target.len();
        let prefix_bonus = if label.starts_with(self.token) {
            500
        } else {
            0
        };
        self.items.push(Candidate {
            text,
            label,
            kind,
            description,
            doc,
            positions: m.positions.into_iter().map(|p| p + shift).collect(),
            score: m.score + prefix_bonus,
            replace_from: self.replace_from,
            append_space,
        });
    }
}

pub fn complete(ctx: &Context) -> Option<Completion> {
    let before = ctx.buffer.get(..ctx.cursor)?;
    let segment = segment_start(before);
    let mut words = words(&before[segment..], segment);
    let at_new_word = before.ends_with(char::is_whitespace) || before.len() == segment;
    let (token_start, token) = if at_new_word {
        (ctx.cursor, String::new())
    } else {
        words.pop()?
    };
    // Prefixes like `sudo` or `FOO=1` don't count: what follows is a command again.
    while let Some((_, first)) = words.first() {
        if PRECOMMANDS.contains(&first.as_str()) || (first.contains('=') && !first.starts_with('-'))
        {
            words.remove(0);
        } else {
            break;
        }
    }

    let mut b = Builder {
        token: &token,
        replace_from: token_start,
        items: Vec::new(),
    };

    if words.is_empty() {
        if token.is_empty() {
            return None;
        }
        for (name, spec) in &ctx.specs.0 {
            b.push(
                name.clone(),
                name.clone(),
                SuggestionKind::Command,
                spec.description.clone(),
                Some(spec_doc(name.clone(), spec)),
                true,
            );
        }
        for name in (ctx.commands)(&token) {
            if !ctx.specs.0.contains_key(&name) {
                b.push(
                    name.clone(),
                    name,
                    SuggestionKind::Command,
                    None,
                    None,
                    true,
                );
            }
        }
    } else {
        let command = words[0].1.as_str();
        let spec = ctx.specs.0.get(command);
        let sub = spec.and_then(|s| words.get(1).and_then(|(_, w)| s.subcommands.get(w)));
        let active = sub.or(spec);

        if token.starts_with('-') {
            if let Some(active) = active {
                for (flag, description) in &active.flags {
                    b.push(
                        flag.clone(),
                        flag.clone(),
                        SuggestionKind::Flag,
                        Some(description.clone()),
                        None,
                        true,
                    );
                }
            }
        } else if let (Some(spec), 1) = (spec, words.len())
            && !spec.subcommands.is_empty()
        {
            for (name, sub) in &spec.subcommands {
                b.push(
                    name.clone(),
                    name.clone(),
                    SuggestionKind::Subcommand,
                    sub.description.clone(),
                    Some(spec_doc(format!("{command} {name}"), sub)),
                    true,
                );
            }
        } else {
            match active.and_then(|s| s.args.as_deref()) {
                Some("branch") => {
                    for branch in ctx.branches {
                        let current = ctx.current_branch == Some(branch.as_str());
                        let description = if current {
                            "current branch"
                        } else {
                            "local branch"
                        };
                        b.push(
                            escape(branch),
                            branch.clone(),
                            SuggestionKind::Branch,
                            Some(description.into()),
                            None,
                            true,
                        );
                    }
                }
                Some("script") => {
                    for (name, command) in (ctx.scripts)() {
                        b.push(
                            escape(&name),
                            name,
                            SuggestionKind::Subcommand,
                            Some(command),
                            None,
                            true,
                        );
                    }
                }
                Some("host") => {
                    for (alias, describe) in (ctx.hosts)() {
                        b.push(
                            escape(&alias),
                            alias,
                            SuggestionKind::Path,
                            Some(describe),
                            None,
                            true,
                        );
                    }
                }
                Some("command") => {
                    if !token.is_empty() {
                        for name in (ctx.commands)(&token) {
                            b.push(
                                name.clone(),
                                name,
                                SuggestionKind::Command,
                                None,
                                None,
                                true,
                            );
                        }
                    }
                }
                args => complete_paths(&mut b, &token, ctx.cwd, ctx.list_dir, args == Some("dir")),
            }
        }
    }

    // History lines that start with the typed text, at the end of the list.
    let typed = before.trim_start();
    if typed.len() >= 2 {
        let mut seen = Vec::new();
        for entry in ctx.history.iter().rev() {
            if entry.len() > typed.len() && entry.starts_with(typed) && !seen.contains(entry) {
                seen.push(entry.clone());
                b.items.push(Candidate {
                    text: entry.clone(),
                    label: entry.clone(),
                    kind: SuggestionKind::History,
                    description: Some("from history".into()),
                    doc: None,
                    // Byte offsets of each typed character, for the highlight: `ú` takes two.
                    positions: typed.char_indices().map(|(i, _)| i).collect(),
                    score: i32::MIN / 2,
                    replace_from: before.len() - typed.len(),
                    append_space: false,
                });
                if seen.len() == 3 {
                    break;
                }
            }
        }
    }

    let mut items = b.items;
    items.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.label.cmp(&b.label)));
    items.truncate(MAX_ITEMS);
    (!items.is_empty()).then_some(Completion { token, items })
}

fn complete_paths(
    b: &mut Builder,
    token: &str,
    cwd: Option<&Path>,
    list_dir: &dyn Fn(&Path) -> Vec<(String, bool)>,
    dirs_only: bool,
) {
    let (dir_part, prefix) = match token.rfind('/') {
        Some(i) => (&token[..=i], &token[i + 1..]),
        None => ("", token),
    };
    let base = if dir_part.is_empty() {
        cwd.map(Path::to_path_buf).unwrap_or_default()
    } else {
        expand(dir_part, cwd)
    };
    let mut entries = list_dir(&base);
    entries.sort();
    // The fuzzy filter works on the name; the full word is built afterwards.
    let mut names = Builder {
        token: prefix,
        replace_from: b.replace_from,
        items: Vec::new(),
    };
    for (name, is_dir) in entries {
        if (name.starts_with('.') && !prefix.starts_with('.')) || (dirs_only && !is_dir) {
            continue;
        }
        let text = format!(
            "{dir_part}{}{}",
            escape(&name),
            if is_dir { "/" } else { "" }
        );
        let label = format!("{name}{}", if is_dir { "/" } else { "" });
        let description = Some(if is_dir { "folder" } else { "file" }.to_string());
        names.push(
            text,
            label,
            SuggestionKind::Path,
            description,
            None,
            !is_dir,
        );
    }
    b.items.extend(names.items);
}

/// The line after accepting `candidate`, and where the cursor ends up (in characters).
pub fn apply(buffer: &str, cursor: usize, candidate: &Candidate) -> (String, usize) {
    let rest = &buffer[cursor.min(buffer.len())..];
    let mut inserted = candidate.text.clone();
    if candidate.append_space && !rest.starts_with(' ') {
        inserted.push(' ');
    }
    let head = &buffer[..candidate.replace_from.min(buffer.len())];
    let new_cursor = head.chars().count() + inserted.chars().count();
    (format!("{head}{inserted}{rest}"), new_cursor)
}

/// `package.json` scripts in `dir`.
pub fn package_scripts(dir: &Path) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(dir.join("package.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    value
        .get("scripts")
        .and_then(|s| s.as_object())
        .map(|scripts| {
            scripts
                .iter()
                .map(|(name, command)| {
                    (
                        name.clone(),
                        command.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Entries of a folder (at most 500), for path completion.
pub fn list_dir(dir: &Path) -> Vec<(String, bool)> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .take(500)
        .map(|entry| {
            let is_dir = entry.path().is_dir();
            (entry.file_name().to_string_lossy().into_owned(), is_dir)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(buffer: &str) -> Vec<(String, SuggestionKind)> {
        run_at(buffer, buffer.len())
    }

    fn run_at(buffer: &str, cursor: usize) -> Vec<(String, SuggestionKind)> {
        let specs = load_specs();
        let branches = vec![
            "main".to_string(),
            "feat/tabs".to_string(),
            "fix/scroll".to_string(),
        ];
        let history = vec![
            "git checkout -b feat/completions".to_string(),
            "ls -la".to_string(),
        ];
        let hosts = vec![("prod-db".to_string(), "angel@10.0.4.12".to_string())];
        let commands = |prefix: &str| {
            ["gcc", "git", "gitk", "grep", "go"]
                .iter()
                .filter(|c| c.starts_with(prefix))
                .map(|c| c.to_string())
                .collect()
        };
        let list_dir = |dir: &Path| {
            if dir.ends_with("src") {
                vec![
                    ("main.rs".to_string(), false),
                    ("migration notes.rs".to_string(), false),
                ]
            } else {
                vec![
                    ("src".to_string(), true),
                    ("Cargo.toml".to_string(), false),
                    (".git".to_string(), true),
                    ("README.md".to_string(), false),
                ]
            }
        };
        let scripts = || {
            vec![
                ("dev".to_string(), "vite".to_string()),
                ("build".to_string(), "vite build".to_string()),
            ]
        };
        let ctx = Context {
            buffer,
            cursor,
            cwd: Some(Path::new("/project")),
            specs: &specs,
            branches: &branches,
            current_branch: Some("main"),
            history: &history,
            hosts: &|| hosts.clone(),
            commands: &commands,
            list_dir: &list_dir,
            scripts: &scripts,
        };
        complete(&ctx)
            .map(|c| c.items.into_iter().map(|i| (i.text, i.kind)).collect())
            .unwrap_or_default()
    }

    fn texts(items: &[(String, SuggestionKind)]) -> Vec<&str> {
        items.iter().map(|(t, _)| t.as_str()).collect()
    }

    #[test]
    fn specs_parse() {
        let specs = load_specs();
        assert!(specs.0["git"].subcommands.contains_key("checkout"));
        assert_eq!(specs.0["cd"].args.as_deref(), Some("dir"));
    }

    #[test]
    fn subcommands_ranked_by_prefix_then_history() {
        let items = run("git ch");
        assert_eq!(
            items[0],
            ("checkout".to_string(), SuggestionKind::Subcommand)
        );
        assert!(texts(&items).contains(&"cherry-pick"));
        assert_eq!(
            items.last().unwrap(),
            &(
                "git checkout -b feat/completions".to_string(),
                SuggestionKind::History
            )
        );
    }

    #[test]
    fn commands_from_specs_and_path() {
        let items = run("gi");
        assert_eq!(texts(&items)[..2], ["git", "gitk"]);
        assert!(run("").is_empty());
    }

    #[test]
    fn flags_of_the_subcommand() {
        let items = run("git commit --am");
        assert_eq!(items[0], ("--amend".to_string(), SuggestionKind::Flag));
        let items = run("ls -");
        assert!(texts(&items).contains(&"-la") || texts(&items).contains(&"-l"));
    }

    #[test]
    fn branches_scripts_hosts_and_dirs() {
        assert_eq!(texts(&run("git switch fe")), vec!["feat/tabs"]);
        assert_eq!(texts(&run("npm run b")), vec!["build"]);
        assert_eq!(texts(&run("ssh pro")), vec!["prod-db"]);
        // `cd` only offers folders, and no hidden ones.
        assert_eq!(texts(&run("cd ")), vec!["src/"]);
        assert_eq!(texts(&run("cd .g")), vec![".git/"]);
    }

    #[test]
    fn paths_inside_folders_escape_spaces() {
        // Prefix matches first; `main.rs` also matches (fuzzy), but ranks lower.
        assert_eq!(
            texts(&run("cat src/mi"))[..2],
            ["src/migration\\ notes.rs", "src/main.rs"]
        );
        let items = run("cat ");
        assert!(texts(&items).contains(&"src/") && texts(&items).contains(&"Cargo.toml"));
    }

    #[test]
    fn pipes_and_sudo_reset_command_position() {
        assert_eq!(texts(&run("ls | gre")), vec!["grep"]);
        assert_eq!(run("sudo gi")[0].0, "git");
        assert_eq!(run("FOO=1 git sw")[0].0, "switch");
    }

    #[test]
    fn apply_replaces_the_word() {
        let candidate = Candidate {
            text: "checkout".into(),
            label: "checkout".into(),
            kind: SuggestionKind::Subcommand,
            description: None,
            doc: None,
            positions: vec![],
            score: 0,
            replace_from: 4,
            append_space: true,
        };
        assert_eq!(apply("git ch", 6, &candidate), ("git checkout ".into(), 13));
        // Text after the cursor is kept, and the cursor lands right after the insertion.
        assert_eq!(
            apply("git ch main", 6, &candidate),
            ("git checkout main".into(), 12)
        );
        let dir = Candidate {
            text: "src/".into(),
            append_space: false,
            replace_from: 3,
            ..candidate
        };
        assert_eq!(apply("cd s", 4, &dir), ("cd src/".into(), 7));
    }

    #[test]
    fn completes_in_the_middle_of_the_line() {
        // Cursor after `git ch`, with more text to the right.
        let items = run_at("git ch main", 6);
        assert_eq!(items[0].0, "checkout");
    }
}
