//! Fixes for the command that just failed, worked out on the machine (nothing is sent anywhere):
//! the command the tool itself printed (`git push --set-upstream origin main`), the word it
//! says was meant (`git stauts` → `status`, `cargo biuld` → `build`), a command name that's a
//! typo of a known one (`gti` → `git`), a folder or branch that's a typo of one that exists,
//! and a script in the current folder run without `./`.

use std::{path::Path, sync::LazyLock};

use regex::Regex;

/// A command that failed, with the tail of its output.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub command: String,
    pub exit_code: Option<i32>,
    pub output: String,
}

impl Failure {
    /// Whether it's worth suggesting anything: not for ⌃C and other signals, nor for commands
    /// whose exit code 1 just means "no" (`grep` with no matches, `diff` with differences).
    pub fn worth_fixing(&self) -> bool {
        let Some(code) = self.exit_code else {
            return false;
        };
        if code == 0 || code > 128 {
            return false;
        }
        let program = self.command.split_whitespace().next().unwrap_or_default();
        !(code == 1
            && matches!(
                program,
                "grep" | "egrep" | "fgrep" | "rg" | "ag" | "diff" | "cmp" | "test" | "[" | "false"
            ))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fix {
    pub command: String,
    /// Where it comes from, for the hint next to it.
    pub reason: String,
}

/// What the fixes need from the terminal, so they can be tested without one.
pub struct Context<'a> {
    pub cwd: Option<&'a Path>,
    /// The closest command name (PATH, aliases, functions) to a word that wasn't found.
    pub closest_command: &'a dyn Fn(&str) -> Option<String>,
    pub branches: &'a [String],
    /// Subfolders of a folder.
    pub list_dirs: &'a dyn Fn(&Path) -> Vec<String>,
}

pub fn suggest(failure: &Failure, cx: &Context) -> Option<Fix> {
    if !failure.worth_fixing() {
        return None;
    }
    let fix = printed_command(failure)
        .or_else(|| meant_word(failure))
        .or_else(|| unknown_command(failure, cx))
        .or_else(|| missing_folder(failure, cx))
        .or_else(|| missing_branch(failure, cx))?;
    (fix.command.trim() != failure.command.trim()).then_some(fix)
}

/// Output lines without git's `hint:` prefix.
fn lines(output: &str) -> Vec<&str> {
    output
        .lines()
        .map(|line| line.strip_prefix("hint:").unwrap_or(line))
        .collect()
}

fn program(command: &str) -> Option<&str> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let skip = words
        .iter()
        .take_while(|w| matches!(**w, "sudo" | "command" | "noglob" | "time" | "nice"))
        .count();
    words.get(skip).copied()
}

/// A command the tool printed on an indented line after "…, use" or "…:", such as git's
/// `git push --set-upstream origin main`. Placeholders (`<branch>`) rule it out.
fn printed_command(failure: &Failure) -> Option<Fix> {
    let program = program(&failure.command)?;
    let lines = lines(&failure.output);
    for (ix, line) in lines.iter().enumerate() {
        let indented = line.starts_with("  ") || line.starts_with('\t');
        let candidate = line.trim();
        if !indented
            || !candidate.starts_with(&format!("{program} "))
            || candidate.contains('<')
            || candidate.contains('>')
        {
            continue;
        }
        let intro = lines[..ix]
            .iter()
            .rev()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .unwrap_or_default();
        let lower = intro.to_ascii_lowercase();
        if intro.ends_with(':') || lower.ends_with(" use") || lower.ends_with(" run") {
            return Some(Fix {
                command: candidate.to_string(),
                reason: format!("suggested by {program}"),
            });
        }
    }
    None
}

static UNKNOWN_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)'([^']+)' is not a [\w\-]+ command|no such (?:sub)?command:? [`'"]?([^`'"\s]+)|unknown (?:sub)?command:? [`'"]?([^`'"\s]+)|unrecognized subcommand [`'"]?([^`'"\s]+)|invalid choice: [`'"]?([^`'"\s]+)"#,
    )
    .unwrap()
});

static MEANT_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)did you mean[: ]+[`'"]?([\w\-.:]+)[`'"]?\s*\?|maybe you meant[: ]+[`'"]?([\w\-.:]+)|similar name exists:? [`'"]?([\w\-.:]+)|tip: (?:a similar subcommand exists|some similar subcommands exist): [`'"]?([\w\-.:]+)"#,
    )
    .unwrap()
});

/// The tool says which word was meant: `git stauts` ("The most similar command is status"),
/// `cargo biuld` ("Did you mean `build`?"), `npm isntall` ("Did you mean this? npm install").
fn meant_word(failure: &Failure) -> Option<Fix> {
    let program = program(&failure.command)?;
    let output = &failure.output;
    let unknown = UNKNOWN_WORD
        .captures(output)
        .and_then(|caps| caps.iter().skip(1).flatten().next())
        .map(|m| m.as_str().to_string());

    let mut meant = MEANT_WORD
        .captures(output)
        .and_then(|caps| caps.iter().skip(1).flatten().next())
        .map(|m| m.as_str().to_string())
        // npm's "Did you mean this?" puts the answer on the next line.
        .filter(|word| !word.eq_ignore_ascii_case("this"));
    // A list on the following indented lines: git's "The most similar command(s)", npm's
    // "Did you mean this?".
    if meant.is_none() {
        let lines = lines(output);
        let intro = lines.iter().position(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("most similar command")
                || lower.trim_end().ends_with("did you mean this?")
        });
        meant = intro.and_then(|ix| {
            let line = lines.get(ix + 1)?.trim();
            let words: Vec<&str> = line.split('#').next()?.split_whitespace().collect();
            match words.as_slice() {
                [word] => Some(word.to_string()),
                [first, second, ..] if *first == program => Some(second.to_string()),
                _ => None,
            }
        });
    }
    let meant = meant?;

    let words: Vec<&str> = failure.command.split_whitespace().collect();
    let target = match &unknown {
        Some(unknown) => words.iter().position(|w| w == unknown)?,
        // Without the tool naming the wrong word, the closest one to what it meant.
        None => {
            words
                .iter()
                .enumerate()
                .skip(1)
                .filter(|(_, w)| w.len() >= 3 && !w.starts_with('-'))
                .map(|(ix, w)| (distance(w, &meant), ix))
                .filter(|&(d, _)| d > 0 && d <= 2)
                .min()?
                .1
        }
    };
    if target == 0 || words[target] == meant {
        return None;
    }
    Some(Fix {
        command: replace_word(&failure.command, target, &meant),
        reason: format!("suggested by {program}"),
    })
}

/// `gti status` → `git status`; `deploy.sh` → `./deploy.sh` when it's in the current folder.
fn unknown_command(failure: &Failure, cx: &Context) -> Option<Fix> {
    let not_found = failure.exit_code == Some(127)
        || failure.output.contains("command not found")
        || failure.output.contains("not recognized as");
    if !not_found {
        return None;
    }
    let words: Vec<&str> = failure.command.split_whitespace().collect();
    let target = words
        .iter()
        .position(|w| !matches!(*w, "sudo" | "command" | "noglob" | "time" | "nice"))?;
    let word = words[target];
    if word.contains('/') || word.contains('=') {
        return None;
    }
    if let Some(cwd) = cx.cwd
        && crate::process::is_executable(&cwd.join(word))
    {
        return Some(Fix {
            command: replace_word(&failure.command, target, &format!("./{word}")),
            reason: format!("{word} is in this folder"),
        });
    }
    let closest = (cx.closest_command)(word)?;
    Some(Fix {
        command: replace_word(&failure.command, target, &closest),
        reason: format!("{word} isn't a command"),
    })
}

/// `cd Documnets` → `cd Documents`.
fn missing_folder(failure: &Failure, cx: &Context) -> Option<Fix> {
    let words: Vec<&str> = failure.command.split_whitespace().collect();
    let [cmd, dir] = words.as_slice() else {
        return None;
    };
    if !matches!(*cmd, "cd" | "pushd")
        || !failure
            .output
            .to_ascii_lowercase()
            .contains("no such file or directory")
    {
        return None;
    }
    let dir = dir.trim_end_matches('/');
    let (parent, name) = match dir.rsplit_once('/') {
        Some((parent, name)) => (Some(parent), name),
        None => (None, dir),
    };
    let base = match parent {
        Some("") => Path::new("/").to_path_buf(),
        Some(parent) => crate::highlight::expand_path(parent, cx.cwd),
        None => cx.cwd?.to_path_buf(),
    };
    let closest = closest(name, (cx.list_dirs)(&base).iter().map(String::as_str))?;
    let fixed = match parent {
        Some(parent) => format!("{parent}/{closest}"),
        None => closest,
    };
    Some(Fix {
        command: replace_word(&failure.command, 1, &crate::completions::escape(&fixed)),
        reason: format!("{name} doesn't exist"),
    })
}

/// `git checkout mian` → `git checkout main`.
fn missing_branch(failure: &Failure, cx: &Context) -> Option<Fix> {
    let words: Vec<&str> = failure.command.split_whitespace().collect();
    if words.first() != Some(&"git") || !matches!(words.get(1), Some(&("checkout" | "switch"))) {
        return None;
    }
    let output = failure.output.to_ascii_lowercase();
    if !output.contains("did not match any") && !output.contains("invalid reference") {
        return None;
    }
    let (target, branch) = words
        .iter()
        .enumerate()
        .skip(2)
        .rfind(|(_, w)| !w.starts_with('-'))?;
    let closest = closest(branch, cx.branches.iter().map(String::as_str))?;
    Some(Fix {
        command: replace_word(&failure.command, target, &closest),
        reason: format!("no branch {branch}"),
    })
}

/// The candidate closest to `word`, within two edits (case aside), if there's a clear one.
pub fn closest<'a>(word: &str, candidates: impl Iterator<Item = &'a str>) -> Option<String> {
    let lower = word.to_lowercase();
    let max = if word.chars().count() <= 3 { 1 } else { 2 };
    let mut best: Option<(usize, &str)> = None;
    for candidate in candidates {
        if candidate == word {
            continue;
        }
        let d = if candidate.to_lowercase() == lower {
            0
        } else {
            distance(&lower, &candidate.to_lowercase())
        };
        if d <= max && best.is_none_or(|(best_d, _)| d < best_d) {
            best = Some((d, candidate));
        }
    }
    best.map(|(_, candidate)| candidate.to_string())
}

/// Edit distance counting a swap of two neighbors as one edit (optimal string alignment).
pub fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// The command with its `index`th word (split on whitespace) replaced, keeping the spacing.
fn replace_word(command: &str, index: usize, with: &str) -> String {
    let mut out = String::new();
    let mut word = 0;
    let mut rest = command;
    loop {
        let start = rest.len() - rest.trim_start().len();
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        if rest.is_empty() {
            break;
        }
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        out.push_str(if word == index { with } else { &rest[..end] });
        rest = &rest[end..];
        word += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn failure(command: &str, exit_code: i32, output: &str) -> Failure {
        Failure {
            command: command.into(),
            exit_code: Some(exit_code),
            output: output.into(),
        }
    }

    fn fix(failure: &Failure) -> Option<String> {
        let closest_command = |word: &str| {
            closest(
                word,
                ["git", "ls", "cargo", "npm", "python3", "make"].into_iter(),
            )
        };
        let list_dirs = |dir: &Path| {
            if dir == Path::new("/home/me") {
                vec!["Documents".into(), "Downloads".into(), "projects".into()]
            } else {
                vec![]
            }
        };
        let branches = vec!["main".to_string(), "feature/login".to_string()];
        let cwd = PathBuf::from("/home/me");
        suggest(
            failure,
            &Context {
                cwd: Some(&cwd),
                closest_command: &closest_command,
                branches: &branches,
                list_dirs: &list_dirs,
            },
        )
        .map(|fix| fix.command)
    }

    #[test]
    fn takes_the_command_git_prints() {
        let output = "fatal: The current branch feat has no upstream branch.\nTo push the current branch and set the remote as upstream, use\n\n    git push --set-upstream origin feat\n\nTo have this happen automatically for branches without a tracking\nupstream, see 'push.autoSetupRemote' in 'git help config'.";
        assert_eq!(
            fix(&failure("git push", 128, output)).as_deref(),
            Some("git push --set-upstream origin feat")
        );
        // Placeholders aren't a command to run.
        let output = "If you wish to set tracking information for this branch you can do so with:\n\n    git branch --set-upstream-to=origin/<branch> feat\n";
        assert_eq!(fix(&failure("git pull", 1, output)), None);
    }

    #[test]
    fn replaces_the_word_the_tool_meant() {
        let git = "git: 'stauts' is not a git command. See 'git --help'.\n\nThe most similar command is\n\tstatus";
        assert_eq!(
            fix(&failure("git stauts -s", 1, git)).as_deref(),
            Some("git status -s")
        );
        let cargo = "error: no such command: `biuld`\n\n\tDid you mean `build`?\n";
        assert_eq!(
            fix(&failure("cargo biuld --release", 101, cargo)).as_deref(),
            Some("cargo build --release")
        );
        let cargo_new =
            "error: no such command: `tset`\n\nhelp: a command with a similar name exists: `test`";
        assert_eq!(
            fix(&failure("cargo tset", 101, cargo_new)).as_deref(),
            Some("cargo test")
        );
        let npm = "Unknown command: \"isntall\"\n\nDid you mean this?\n    npm install # Install a package\n";
        assert_eq!(
            fix(&failure("npm isntall left-pad", 1, npm)).as_deref(),
            Some("npm install left-pad")
        );
        let pip = "ERROR: unknown command \"isntall\" - maybe you meant \"install\"";
        assert_eq!(
            fix(&failure("pip isntall requests", 1, pip)).as_deref(),
            Some("pip install requests")
        );
    }

    #[test]
    fn fixes_command_names() {
        let zsh = "zsh: command not found: gti";
        assert_eq!(
            fix(&failure("gti status", 127, zsh)).as_deref(),
            Some("git status")
        );
        assert_eq!(
            fix(&failure(
                "sudo mkae install",
                127,
                "sudo: mkae: command not found"
            ))
            .as_deref(),
            Some("sudo make install")
        );
        // Nothing close enough.
        assert_eq!(fix(&failure("kubectl get pods", 127, zsh)), None);
    }

    #[test]
    fn fixes_folders_and_branches() {
        let cd = "cd: no such file or directory: Documnets";
        assert_eq!(
            fix(&failure("cd Documnets", 1, cd)).as_deref(),
            Some("cd Documents")
        );
        assert_eq!(
            fix(&failure("cd documents", 1, cd)).as_deref(),
            Some("cd Documents")
        );
        let git = "error: pathspec 'mian' did not match any file(s) known to git";
        assert_eq!(
            fix(&failure("git checkout mian", 1, git)).as_deref(),
            Some("git checkout main")
        );
        let switch = "fatal: invalid reference: featur/login";
        assert_eq!(
            fix(&failure("git switch featur/login", 128, switch)).as_deref(),
            Some("git switch feature/login")
        );
    }

    #[test]
    fn leaves_alone_signals_and_plain_no() {
        assert_eq!(
            fix(&failure("gti", 130, "zsh: command not found: gti")),
            None
        );
        assert!(!failure("grep foo x", 1, "").worth_fixing());
        assert!(failure("grep foo x", 2, "").worth_fixing());
        assert!(failure("make", 2, "").worth_fixing());
    }

    #[test]
    fn distances_and_word_replacement() {
        assert_eq!(distance("gti", "git"), 1);
        assert_eq!(distance("stauts", "status"), 1);
        assert_eq!(distance("biuld", "build"), 1);
        assert_eq!(distance("kitten", "sitting"), 3);
        assert_eq!(replace_word("  a  b c", 1, "X"), "  a  X c");
        assert_eq!(closest("ls", ["ls", "lsof"].into_iter()), None);
    }
}
