//! Repository status of the current folder, read with the system `git`.

use std::{path::Path, process::Command};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GitStatus {
    /// `None` with a detached HEAD.
    pub branch: Option<String>,
    /// Current commit, abbreviated (or empty in a repo with no commits).
    pub head: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
    pub conflicted: u32,
    /// Local branches, most recently committed first.
    pub branches: Vec<String>,
}

impl GitStatus {
    /// Display name: the branch, or the commit if HEAD is detached.
    pub fn display_name(&self) -> String {
        match &self.branch {
            Some(branch) => branch.clone(),
            None if self.head.is_empty() => "HEAD".into(),
            None => self.head.clone(),
        }
    }
}

fn git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    // No optional locks: don't compete with a `git` the user is running.
    command
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C");
    command
}

/// `None` if the folder isn't in a repository or `git` isn't installed.
/// Blocking: call from the background executor.
pub fn read_status(dir: &Path) -> Option<GitStatus> {
    let output = git(dir)
        .args(["status", "--porcelain=v2", "--branch"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut status = parse_porcelain(&String::from_utf8_lossy(&output.stdout));

    if let Ok(output) = git(dir)
        .args([
            "for-each-ref",
            "--sort=-committerdate",
            "--count=60",
            "--format=%(refname:short)",
            "refs/heads/",
        ])
        .output()
    {
        status.branches = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_string)
            .filter(|b| !b.is_empty())
            .collect();
    }
    Some(status)
}

pub fn parse_porcelain(output: &str) -> GitStatus {
    let mut status = GitStatus::default();
    for line in output.lines() {
        if let Some(header) = line.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value != "(initial)" => {
                    status.head = value.chars().take(7).collect();
                }
                "branch.head" if value != "(detached)" => status.branch = Some(value.to_string()),
                "branch.upstream" => status.upstream = Some(value.to_string()),
                "branch.ab" => {
                    for part in value.split_whitespace() {
                        if let Some(n) = part.strip_prefix('+') {
                            status.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            status.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let mut fields = line.splitn(3, ' ');
        match fields.next() {
            Some("1") | Some("2") => {
                let xy = fields.next().unwrap_or("..").as_bytes();
                if xy.first().is_some_and(|&x| x != b'.') {
                    status.staged += 1;
                }
                if xy.get(1).is_some_and(|&y| y != b'.') {
                    status.modified += 1;
                }
            }
            Some("u") => status.conflicted += 1,
            Some("?") => status.untracked += 1,
            _ => {}
        }
    }
    status
}

/// Shell command to switch branches, with the name in single quotes.
pub fn switch_command(branch: &str) -> String {
    format!("git switch '{}'", branch.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_and_changes() {
        let status = parse_porcelain(
            "# branch.oid 3f2a9c1e0d\n\
             # branch.head feat/tabs\n\
             # branch.upstream origin/feat/tabs\n\
             # branch.ab +1 -2\n\
             1 M. N... 100644 100644 100644 abc def src/a.rs\n\
             1 .M N... 100644 100644 100644 abc def src/b.rs\n\
             1 MM N... 100644 100644 100644 abc def src/c.rs\n\
             2 R. N... 100644 100644 100644 abc def R100 new.rs\told.rs\n\
             u UU N... 100644 100644 100644 100644 a b c conflict.rs\n\
             ? notes.md\n",
        );
        assert_eq!(status.branch.as_deref(), Some("feat/tabs"));
        assert_eq!(status.head, "3f2a9c1");
        assert_eq!(status.upstream.as_deref(), Some("origin/feat/tabs"));
        assert_eq!((status.ahead, status.behind), (1, 2));
        assert_eq!((status.staged, status.modified), (3, 2));
        assert_eq!((status.untracked, status.conflicted), (1, 1));
    }

    #[test]
    fn detached_and_initial() {
        let detached = parse_porcelain("# branch.oid 1234567890\n# branch.head (detached)\n");
        assert_eq!(detached.branch, None);
        assert_eq!(detached.display_name(), "1234567");

        let initial = parse_porcelain("# branch.oid (initial)\n# branch.head main\n");
        assert_eq!(initial.display_name(), "main");
        assert_eq!(initial.staged + initial.modified + initial.untracked, 0);
    }

    #[test]
    fn quotes_branch_names() {
        assert_eq!(switch_command("feat/tabs"), "git switch 'feat/tabs'");
        assert_eq!(switch_command("it's"), r"git switch 'it'\''s'");
    }
}
