//! Highlighting for the line being edited: a small shell syntax lexer.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Command,
    UnknownCommand,
    Subcommand,
    Keyword,
    Flag,
    String,
    Path,
    ExistingPath,
    Operator,
    Variable,
    Number,
    Comment,
    Plain,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// Byte range within the line.
    pub range: Range<usize>,
    pub kind: TokenKind,
}

/// Tools whose first argument is a subcommand (`git push`, `cargo build`).
const SUBCOMMAND_TOOLS: &[&str] = &[
    "git",
    "cargo",
    "npm",
    "pnpm",
    "yarn",
    "bun",
    "docker",
    "kubectl",
    "brew",
    "go",
    "rustup",
    "gh",
    "pip",
    "pip3",
    "uv",
    "poetry",
    "systemctl",
    "launchctl",
    "terraform",
    "helm",
    "deno",
    "swift",
    "xcrun",
    "npx",
    "make",
    "just",
];

/// Prefixes after which the next word is a command again.
const PRECOMMANDS: &[&str] = &[
    "sudo", "time", "env", "exec", "nohup", "command", "builtin", "noglob", "nice", "xargs",
    "watch",
];

const KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "in", "do", "done", "while", "until", "case",
    "esac", "function", "select", "repeat", "{", "}", "[[", "]]", "!",
];

/// Common zsh builtins; the shell also sends its actual list on startup.
const BUILTINS: &[&str] = &[
    "cd",
    "echo",
    "export",
    "source",
    ".",
    "alias",
    "unalias",
    "unset",
    "exit",
    "history",
    "pwd",
    "pushd",
    "popd",
    "type",
    "which",
    "eval",
    "exec",
    "set",
    "local",
    "read",
    "printf",
    "test",
    "[",
    "true",
    "false",
    "kill",
    "jobs",
    "fg",
    "bg",
    "wait",
    "ulimit",
    "umask",
    "setopt",
    "unsetopt",
    "autoload",
    "bindkey",
    "zle",
    "print",
    "whence",
    "rehash",
    "hash",
    "trap",
    "return",
    "shift",
    "getopts",
    "let",
    "typeset",
    "declare",
    "functions",
    "emulate",
    "disown",
    "builtin",
    "command",
    "time",
    "sudo",
    "dirs",
    "logout",
    "zmodload",
    "compdef",
];

/// Resolves whether a word is an executable command. Caches PATH lookups.
#[derive(Default)]
pub struct CommandResolver {
    path: Vec<PathBuf>,
    names: HashSet<String>,
    cache: HashMap<String, bool>,
    /// PATH executables, sorted; built the first time a command is completed.
    executables: Option<Vec<String>>,
}

impl CommandResolver {
    pub fn from_env() -> Self {
        let mut resolver = Self::default();
        resolver.set_path(&std::env::var("PATH").unwrap_or_default());
        resolver
    }

    pub fn set_path(&mut self, path: &str) {
        self.path = std::env::split_paths(path).collect();
        self.cache.clear();
        self.executables = None;
    }

    /// Commands that start with `prefix`: shell builtins, aliases and functions, and
    /// PATH executables.
    pub fn names_with_prefix(&mut self, prefix: &str) -> Vec<String> {
        let mut found: Vec<String> = self
            .all_names()
            .into_iter()
            .filter(|name| name.starts_with(prefix))
            .collect();
        found.truncate(200);
        found
    }

    /// The command closest to `word`, a name the shell didn't find (`gti` → `git`).
    pub fn closest(&mut self, word: &str) -> Option<String> {
        let names = self.all_names();
        crate::fixes::closest(word, names.iter().map(String::as_str))
    }

    /// Builtins, aliases, functions and PATH executables, sorted, without `_` helpers.
    fn all_names(&mut self) -> Vec<String> {
        let path = &self.path;
        let executables = self.executables.get_or_insert_with(|| {
            let mut names: Vec<String> = path
                .iter()
                .flat_map(|dir| {
                    std::fs::read_dir(dir)
                        .into_iter()
                        .flatten()
                        .filter_map(Result::ok)
                })
                .map(|entry| entry.path())
                .filter(|path| crate::process::is_executable(path))
                .filter_map(|path| crate::process::command_name(&path))
                .collect();
            names.sort();
            names.dedup();
            names
        });
        let mut found: Vec<String> = BUILTINS
            .iter()
            .map(|b| b.to_string())
            .chain(self.names.iter().cloned())
            .chain(executables.iter().cloned())
            .filter(|name| !name.starts_with('_'))
            .collect();
        found.sort();
        found.dedup();
        found
    }

    /// Aliases, functions and builtins reported by the shell.
    pub fn set_names(&mut self, names: &str) {
        self.names = names.split_whitespace().map(str::to_string).collect();
    }

    pub fn is_command(&mut self, word: &str, cwd: Option<&Path>) -> bool {
        if BUILTINS.contains(&word) || self.names.contains(word) {
            return true;
        }
        if let Some(&known) = self.cache.get(word) {
            return known;
        }
        let found = if word.contains('/') {
            crate::process::is_executable(&expand_path(word, cwd))
        } else {
            self.path
                .iter()
                .any(|dir| crate::process::find_in_dir(dir, word).is_some())
        };
        if !word.contains('/') {
            self.cache.insert(word.to_string(), found);
        }
        found
    }
}

pub fn expand_path(word: &str, cwd: Option<&Path>) -> PathBuf {
    if let Some(rest) = word.strip_prefix('~')
        && let Some(home) = crate::state::home_dir()
    {
        return home.join(rest.trim_start_matches('/'));
    }
    let path = Path::new(word);
    match cwd {
        Some(cwd) if path.is_relative() => cwd.join(path),
        _ => path.to_path_buf(),
    }
}

fn is_operator_char(c: char) -> bool {
    matches!(c, '|' | '&' | ';' | '<' | '>' | '(' | ')')
}

#[derive(PartialEq)]
enum Expect {
    Command,
    /// First argument of a tool with subcommands.
    Subcommand,
    Argument,
    /// Target of a redirection.
    Target,
}

/// Splits the line into highlightable tokens. `cursor` is a byte index: the word ending at
/// the cursor isn't flagged as an unknown command while it's being typed.
pub fn highlight(
    line: &str,
    cursor: usize,
    cwd: Option<&Path>,
    resolver: &mut CommandResolver,
) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut expect = Expect::Command;
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let end_of = |i: usize| chars.get(i).map_or(line.len(), |&(b, _)| b);
    let mut i = 0;

    while i < chars.len() {
        let (start, c) = chars[i];

        if c.is_whitespace() {
            i += 1;
            continue;
        }

        if c == '#' && (i == 0 || chars[i - 1].1.is_whitespace()) {
            tokens.push(Token {
                range: start..line.len(),
                kind: TokenKind::Comment,
            });
            break;
        }

        if is_operator_char(c) {
            let mut j = i;
            while j < chars.len() && is_operator_char(chars[j].1) {
                j += 1;
            }
            let op = &line[start..end_of(j)];
            expect = if op.ends_with('>') || op.ends_with('<') {
                Expect::Target
            } else {
                Expect::Command
            };
            tokens.push(Token {
                range: start..end_of(j),
                kind: TokenKind::Operator,
            });
            i = j;
            continue;
        }

        if c == '\'' || c == '"' {
            let mut j = i + 1;
            while j < chars.len() {
                let ch = chars[j].1;
                if ch == '\\' && c == '"' {
                    j += 2;
                    continue;
                }
                if ch == c {
                    j += 1;
                    break;
                }
                j += 1;
            }
            let j = j.min(chars.len());
            tokens.push(Token {
                range: start..end_of(j),
                kind: TokenKind::String,
            });
            if expect != Expect::Command {
                expect = Expect::Argument;
            }
            i = j;
            continue;
        }

        if c == '$' {
            if chars.get(i + 1).is_some_and(|&(_, n)| n == '(') {
                tokens.push(Token {
                    range: start..end_of(i + 2),
                    kind: TokenKind::Operator,
                });
                expect = Expect::Command;
                i += 2;
                continue;
            }
            let mut j = i + 1;
            if chars.get(j).is_some_and(|&(_, n)| n == '{') {
                while j < chars.len() && chars[j].1 != '}' {
                    j += 1;
                }
                j = (j + 1).min(chars.len());
            } else {
                while j < chars.len()
                    && (chars[j].1.is_alphanumeric() || matches!(chars[j].1, '_' | '?' | '@' | '#'))
                {
                    j += 1;
                }
            }
            tokens.push(Token {
                range: start..end_of(j),
                kind: TokenKind::Variable,
            });
            if expect != Expect::Command {
                expect = Expect::Argument;
            }
            i = j;
            continue;
        }

        // Word: up to a space, operator or quote, honoring `\`.
        let mut j = i;
        while j < chars.len() {
            let ch = chars[j].1;
            if ch == '\\' {
                j += 2;
                continue;
            }
            if ch.is_whitespace() || is_operator_char(ch) || ch == '\'' || ch == '"' {
                break;
            }
            j += 1;
        }
        let j = j.min(chars.len());
        let range = start..end_of(j);
        let word = &line[range.clone()];
        let typing = range.end == cursor;

        let kind = match expect {
            Expect::Command if is_assignment(word) => TokenKind::Variable,
            Expect::Command if KEYWORDS.contains(&word) => TokenKind::Keyword,
            Expect::Command => {
                if resolver.is_command(word, cwd) {
                    expect = if SUBCOMMAND_TOOLS.contains(&word) {
                        Expect::Subcommand
                    } else if PRECOMMANDS.contains(&word) {
                        Expect::Command
                    } else {
                        Expect::Argument
                    };
                    TokenKind::Command
                } else {
                    expect = Expect::Argument;
                    if typing {
                        TokenKind::Plain
                    } else {
                        TokenKind::UnknownCommand
                    }
                }
            }
            _ if word.starts_with('-') => TokenKind::Flag,
            Expect::Subcommand
                if word
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_') =>
            {
                expect = Expect::Argument;
                TokenKind::Subcommand
            }
            _ => {
                expect = Expect::Argument;
                classify_argument(word, cwd)
            }
        };
        tokens.push(Token { range, kind });
        i = j;
    }

    tokens
}

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn classify_argument(word: &str, cwd: Option<&Path>) -> TokenKind {
    if word.chars().all(|c| c.is_ascii_digit()) {
        return TokenKind::Number;
    }
    let looks_like_path = word.contains('/') || word.starts_with('~') || word.starts_with('.');
    if looks_like_path || cwd.is_some_and(|cwd| word.len() > 1 && cwd.join(word).exists()) {
        if expand_path(word, cwd).exists() {
            return TokenKind::ExistingPath;
        }
        if looks_like_path {
            return TokenKind::Path;
        }
    }
    TokenKind::Plain
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(line: &str) -> Vec<(&str, TokenKind)> {
        let mut resolver = CommandResolver::default();
        resolver.set_path("/usr/bin:/bin");
        highlight(line, usize::MAX, None, &mut resolver)
            .into_iter()
            .map(|t| (&line[t.range], t.kind))
            .collect()
    }

    // Looks commands up in the real /usr/bin and /bin.
    #[cfg(unix)]
    #[test]
    fn colors_command_subcommand_flags_and_strings() {
        use TokenKind::*;
        assert_eq!(
            kinds("git commit -m \"feat: tabs\" --amend"),
            vec![
                ("git", Command),
                ("commit", Subcommand),
                ("-m", Flag),
                ("\"feat: tabs\"", String),
                ("--amend", Flag),
            ]
        );
    }

    // Looks commands up in the real /usr/bin and /bin.
    #[cfg(unix)]
    #[test]
    fn pipes_reset_command_position() {
        use TokenKind::*;
        assert_eq!(
            kinds("ls -la /tmp | grep '.rs' > out.txt"),
            vec![
                ("ls", Command),
                ("-la", Flag),
                ("/tmp", ExistingPath),
                ("|", Operator),
                ("grep", Command),
                ("'.rs'", String),
                (">", Operator),
                ("out.txt", Plain),
            ]
        );
    }

    #[test]
    fn variables_assignments_and_comments() {
        use TokenKind::*;
        assert_eq!(
            kinds("FOO=1 echo $HOME ${PATH} # note"),
            vec![
                ("FOO=1", Variable),
                ("echo", Command),
                ("$HOME", Variable),
                ("${PATH}", Variable),
                ("# note", Comment),
            ]
        );
    }

    #[test]
    fn unknown_command_only_after_typing() {
        let mut resolver = CommandResolver::default();
        resolver.set_path("/usr/bin:/bin");
        let typing = highlight("gti", 3, None, &mut resolver);
        assert_eq!(typing[0].kind, TokenKind::Plain);
        let done = highlight("gti status", 10, None, &mut resolver);
        assert_eq!(done[0].kind, TokenKind::UnknownCommand);
    }

    // Looks commands up in the real /usr/bin and /bin.
    #[cfg(unix)]
    #[test]
    fn sudo_keeps_command_position() {
        use TokenKind::*;
        assert_eq!(kinds("sudo ls"), vec![("sudo", Command), ("ls", Command)]);
    }

    #[test]
    fn handles_non_ascii() {
        use TokenKind::*;
        assert_eq!(
            kinds("echo \"naïve\" 日本語"),
            vec![("echo", Command), ("\"naïve\"", String), ("日本語", Plain)]
        );
    }
}
