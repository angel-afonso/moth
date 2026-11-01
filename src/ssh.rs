//! SSH sessions: recognize the `ssh` command, resolve the real destination with `ssh -G` and
//! list the hosts in `~/.ssh/config` for "Connect to…".

use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// Destination of an `ssh` as you typed it.
#[derive(Clone, Debug, PartialEq)]
pub struct SshTarget {
    /// What was typed as the destination, without the user (`prod-db`, `10.0.4.12`).
    pub alias: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub jump: Option<String>,
    /// `ssh` arguments (without the remote command), for `ssh -G`.
    pub args: Vec<String>,
    /// `ssh host command`: not an interactive session.
    pub remote_command: bool,
}

/// What `ssh -G` says will actually be used.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedHost {
    pub hostname: String,
    pub user: String,
    pub port: u16,
}

/// `ssh` options that consume the next argument.
const OPTS_WITH_ARG: &str = "BbcDEeFIiJLlmOoPpQRSWw";

/// Splits like the shell would, honoring single and double quotes.
fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut in_word = false;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    in_word = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            (None, c) => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(current);
    }
    words
}

/// Recognizes `ssh [options] destination [command]` (also with a path, `command ssh` or `env … ssh`).
pub fn parse_ssh_command(command: &str) -> Option<SshTarget> {
    let words = shell_words(command);
    let mut iter = words.iter().peekable();
    // Skip common prefixes and variable assignments.
    while let Some(word) = iter.peek() {
        if matches!(
            word.as_str(),
            "command" | "exec" | "env" | "time" | "noglob"
        ) || (word.contains('=') && !word.starts_with('-'))
        {
            iter.next();
        } else {
            break;
        }
    }
    let program = iter.next()?;
    if program.rsplit('/').next() != Some("ssh") {
        return None;
    }

    let mut args = Vec::new();
    let mut user = None;
    let mut port = None;
    let mut jump = None;
    let mut destination = None;
    while let Some(word) = iter.next() {
        if destination.is_none() && word.starts_with('-') && word.len() > 1 {
            args.push(word.clone());
            // `-p2222` or `-p 2222`; options without an argument can be combined (`-tt`, `-At`).
            let flags: Vec<char> = word[1..].chars().collect();
            for (i, flag) in flags.iter().enumerate() {
                if OPTS_WITH_ARG.contains(*flag) {
                    let inline: String = flags[i + 1..].iter().collect();
                    let value = if inline.is_empty() {
                        let value = iter.next().cloned();
                        if let Some(value) = &value {
                            args.push(value.clone());
                        }
                        value
                    } else {
                        Some(inline)
                    };
                    match flag {
                        'p' => port = value.and_then(|v| v.parse().ok()),
                        'l' => user = value,
                        'J' => jump = value,
                        _ => {}
                    }
                    break;
                }
            }
            continue;
        }
        if destination.is_none() {
            args.push(word.clone());
            destination = Some(word.clone());
            continue;
        }
        // After the destination: remote command.
        return Some(build_target(destination?, user, port, jump, args, true));
    }
    Some(build_target(destination?, user, port, jump, args, false))
}

fn build_target(
    destination: String,
    mut user: Option<String>,
    mut port: Option<u16>,
    jump: Option<String>,
    args: Vec<String>,
    remote_command: bool,
) -> SshTarget {
    let mut host = destination
        .strip_prefix("ssh://")
        .unwrap_or(&destination)
        .to_string();
    if let Some((u, h)) = host.split_once('@') {
        user = Some(u.to_string());
        host = h.to_string();
    }
    // `ssh://host:2222`
    if destination.starts_with("ssh://")
        && let Some((h, p)) = host.rsplit_once(':')
        && let Ok(p) = p.parse()
    {
        port = Some(p);
        host = h.to_string();
    }
    SshTarget {
        alias: host,
        user,
        port,
        jump,
        args,
        remote_command,
    }
}

pub fn parse_ssh_g(output: &str) -> Option<ResolvedHost> {
    let mut hostname = None;
    let mut user = None;
    let mut port = None;
    for line in output.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        match key {
            "hostname" => hostname = Some(value.to_string()),
            "user" => user = Some(value.to_string()),
            "port" => port = value.parse().ok(),
            _ => {}
        }
    }
    Some(ResolvedHost {
        hostname: hostname?,
        user: user?,
        port: port.unwrap_or(22),
    })
}

/// `ssh -G` with the same arguments: applies `~/.ssh/config` without connecting.
/// Blocking: call from the background executor.
pub fn resolve(target: &SshTarget) -> Option<ResolvedHost> {
    let output = Command::new("ssh")
        .arg("-G")
        .args(&target.args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_ssh_g(&String::from_utf8_lossy(&output.stdout))
}

/// A `Host` from `~/.ssh/config` without wildcards.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
}

impl ConfigHost {
    /// `user@host:port` with whatever is known.
    pub fn describe(&self) -> String {
        let mut text = String::new();
        if let Some(user) = &self.user {
            text.push_str(user);
            text.push('@');
        }
        text.push_str(self.hostname.as_deref().unwrap_or(&self.alias));
        if let Some(port) = self.port.filter(|p| *p != 22) {
            text.push_str(&format!(":{port}"));
        }
        text
    }
}

pub fn parse_ssh_config(text: &str, includes: &mut Vec<String>) -> Vec<ConfigHost> {
    let mut hosts: Vec<ConfigHost> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (k.to_lowercase(), v.trim().trim_start_matches('=').trim()),
            None => continue,
        };
        match key.as_str() {
            "host" => {
                current.clear();
                for alias in value.split_whitespace() {
                    if alias.contains(['*', '?', '!']) {
                        continue;
                    }
                    current.push(hosts.len());
                    hosts.push(ConfigHost {
                        alias: alias.to_string(),
                        hostname: None,
                        user: None,
                        port: None,
                    });
                }
            }
            "match" => current.clear(),
            "include" => includes.extend(value.split_whitespace().map(str::to_string)),
            "hostname" => current
                .iter()
                .for_each(|&i| hosts[i].hostname = Some(value.into())),
            "user" => current
                .iter()
                .for_each(|&i| hosts[i].user = Some(value.into())),
            "port" => current
                .iter()
                .for_each(|&i| hosts[i].port = value.parse().ok()),
            _ => {}
        }
    }
    hosts
}

/// Expands an `Include` (relative to `~/.ssh`, with `*` in the file name).
fn expand_include(pattern: &str, ssh_dir: &Path) -> Vec<PathBuf> {
    let path = if let Some(rest) = pattern.strip_prefix("~/") {
        ssh_dir
            .parent()
            .map(|home| home.join(rest))
            .unwrap_or_default()
    } else if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        ssh_dir.join(pattern)
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !name.contains('*') {
        return vec![path];
    }
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| crate::hosts::glob_match(&name, &n.to_string_lossy()))
        })
        .collect();
    files.sort();
    files
}

/// Hosts from `~/.ssh/config` and its `Include`s, in order, without duplicate aliases.
pub fn config_hosts() -> Vec<ConfigHost> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let ssh_dir = Path::new(&home).join(".ssh");
    let mut pending = vec![ssh_dir.join("config")];
    let mut seen_files = Vec::new();
    let mut hosts: Vec<ConfigHost> = Vec::new();
    while let Some(file) = pending.pop() {
        if seen_files.contains(&file) || seen_files.len() > 32 {
            continue;
        }
        seen_files.push(file.clone());
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let mut includes = Vec::new();
        for host in parse_ssh_config(&text, &mut includes) {
            if !hosts.iter().any(|h| h.alias == host.alias) {
                hosts.push(host);
            }
        }
        for include in includes.iter().rev() {
            pending.extend(expand_include(include, &ssh_dir).into_iter().rev());
        }
    }
    hosts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        let t = parse_ssh_command("ssh prod-db").unwrap();
        assert_eq!(t.alias, "prod-db");
        assert!(!t.remote_command);

        let t = parse_ssh_command("ssh -p 2222 -i ~/.ssh/key git@gitea.local").unwrap();
        assert_eq!(
            (t.alias.as_str(), t.user.as_deref(), t.port),
            ("gitea.local", Some("git"), Some(2222))
        );
        assert_eq!(
            t.args,
            vec!["-p", "2222", "-i", "~/.ssh/key", "git@gitea.local"]
        );

        let t = parse_ssh_command("ssh -tt -J bastion -lroot app-02").unwrap();
        assert_eq!(
            (t.alias.as_str(), t.user.as_deref()),
            ("app-02", Some("root"))
        );
        assert_eq!(t.jump.as_deref(), Some("bastion"));

        let t = parse_ssh_command("ssh ssh://deploy@example.com:2200").unwrap();
        assert_eq!(
            (t.alias.as_str(), t.user.as_deref(), t.port),
            ("example.com", Some("deploy"), Some(2200))
        );
    }

    #[test]
    fn detects_remote_commands_and_ignores_other_programs() {
        let t = parse_ssh_command("ssh prod-db 'df -h'").unwrap();
        assert!(t.remote_command);
        assert!(parse_ssh_command("sshfs host:/ /mnt").is_none());
        assert!(parse_ssh_command("git push").is_none());
        assert!(parse_ssh_command("ssh -V").is_none());
        assert_eq!(
            parse_ssh_command("TERM=xterm /usr/bin/ssh box")
                .unwrap()
                .alias,
            "box"
        );
    }

    #[test]
    fn parses_ssh_g_output() {
        let r = parse_ssh_g(
            "user angel\nhostname 10.0.4.12\nport 22\nidentityfile ~/.ssh/id_ed25519\n",
        )
        .unwrap();
        assert_eq!(
            r,
            ResolvedHost {
                hostname: "10.0.4.12".into(),
                user: "angel".into(),
                port: 22
            }
        );
    }

    #[test]
    fn parses_config_hosts() {
        let mut includes = Vec::new();
        let hosts = parse_ssh_config(
            "Include config.d/*\n\nHost prod-db-01 prod\n  HostName 10.0.4.12\n  User angel\n\nHost *.internal\n  User deploy\n\nHost gitea\n  HostName gitea.local\n  Port 2222\n",
            &mut includes,
        );
        assert_eq!(includes, vec!["config.d/*"]);
        let aliases: Vec<_> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, vec!["prod-db-01", "prod", "gitea"]);
        assert_eq!(hosts[0].describe(), "angel@10.0.4.12");
        assert_eq!(hosts[2].describe(), "gitea.local:2222");
    }
}
