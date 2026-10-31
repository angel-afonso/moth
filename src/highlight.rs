//! Resaltado de la línea en edición: un lexer pequeño de sintaxis de shell.

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
    /// Rango en bytes dentro de la línea.
    pub range: Range<usize>,
    pub kind: TokenKind,
}

/// Herramientas cuyo primer argumento es un subcomando (`git push`, `cargo build`).
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

/// Prefijos tras los que la siguiente palabra vuelve a ser un comando.
const PRECOMMANDS: &[&str] = &[
    "sudo", "time", "env", "exec", "nohup", "command", "builtin", "noglob", "nice", "xargs",
    "watch",
];

const KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "in", "do", "done", "while", "until", "case",
    "esac", "function", "select", "repeat", "{", "}", "[[", "]]", "!",
];

/// Builtins de zsh habituales; la shell además envía su lista real al iniciar.
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

/// Resuelve si una palabra es un comando ejecutable. Guarda en caché las búsquedas en PATH.
#[derive(Default)]
pub struct CommandResolver {
    path: Vec<PathBuf>,
    names: HashSet<String>,
    cache: HashMap<String, bool>,
    /// Ejecutables del PATH, ordenados; se arma la primera vez que se completa un comando.
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

    /// Comandos que empiezan con `prefix`: builtins, alias y funciones de la shell y
    /// ejecutables del PATH.
    pub fn names_with_prefix(&mut self, prefix: &str) -> Vec<String> {
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
                .filter(|entry| is_executable(&entry.path()))
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
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
            .filter(|name| name.starts_with(prefix) && !name.starts_with('_'))
            .collect();
        found.sort();
        found.dedup();
        found.truncate(200);
        found
    }

    /// Alias, funciones y builtins que reporta la shell.
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
            is_executable(&expand_path(word, cwd))
        } else {
            self.path.iter().any(|dir| is_executable(&dir.join(word)))
        };
        if !word.contains('/') {
            self.cache.insert(word.to_string(), found);
        }
        found
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

pub fn expand_path(word: &str, cwd: Option<&Path>) -> PathBuf {
    if let Some(rest) = word.strip_prefix('~')
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest.trim_start_matches('/'));
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
    /// Primer argumento de una herramienta con subcomandos.
    Subcommand,
    Argument,
    /// Destino de una redirección.
    Target,
}

/// Divide la línea en tokens coloreables. `cursor` es un índice en bytes: la palabra que
/// termina en el cursor no se marca como comando desconocido mientras se escribe.
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

        // Palabra: hasta un espacio, operador o comilla, respetando `\`.
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
            kinds("FOO=1 echo $HOME ${PATH} # nota"),
            vec![
                ("FOO=1", Variable),
                ("echo", Command),
                ("$HOME", Variable),
                ("${PATH}", Variable),
                ("# nota", Comment),
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

    #[test]
    fn sudo_keeps_command_position() {
        use TokenKind::*;
        assert_eq!(kinds("sudo ls"), vec![("sudo", Command), ("ls", Command)]);
    }

    #[test]
    fn handles_non_ascii() {
        use TokenKind::*;
        assert_eq!(
            kinds("echo \"canción\" ñandú"),
            vec![("echo", Command), ("\"canción\"", String), ("ñandú", Plain)]
        );
    }
}
