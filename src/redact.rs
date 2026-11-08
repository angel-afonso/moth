//! Strips secrets from text before it leaves the machine (what goes to the agent).
//!
//! Three layers: token formats that are secret wherever they show up (`ghp_…`, `AKIA…`,
//! private keys, JWTs), values next to a name that says they're secret (`API_TOKEN=…`,
//! `--password x`, `Authorization: Bearer …`, `https://user:pass@host`), and long random
//! strings with no name at all. Each match is replaced with `[REDACTED]`, so the agent still
//! sees the shape of the command. On top of that, commands whose whole output is made of
//! secrets (`env`, `cat .env`, `gh auth token`…) don't send their output at all.

use std::sync::LazyLock;

use regex::{Captures, Regex};

pub const PLACEHOLDER: &str = "[REDACTED]";

/// Text with its secrets replaced, and how many were found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Redacted {
    pub text: String,
    pub count: usize,
}

/// Names that make the value next to them a secret: `DB_PASSWORD=`, `"api_key":`, `--token`.
const SECRET_NAME: &str = r"(?:passw(?:or)?d|passphrase|pwd|secret|token|api[_\-]?key|access[_\-]?key|private[_\-]?key|credentials?|auth[_\-]?(?:key|token)|session[_\-]?(?:id|key|token)|signing[_\-]?key|encryption[_\-]?key|dsn|connection[_\-]?string)";
/// A value: quoted, or up to the next space or separator.
const VALUE: &str = r#"(?:"(?:[^"\\\n]|\\.)*"|'[^'\n]*'|[^\s"',;&|)}\]]+)"#;

static PRIVATE_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY( BLOCK)?-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY( BLOCK)?-----|\z)")
        .unwrap()
});

/// Formats that are secret on their own, wherever they appear.
static TOKENS: LazyLock<Regex> = LazyLock::new(|| {
    let formats = [
        // AWS access key IDs.
        r"\b(?:AKIA|ASIA|AGPA|AIDA|AROA|ANPA|ANVA|AIPA)[0-9A-Z]{16}\b",
        // GitHub, GitLab.
        r"\bgh[pousr]_[A-Za-z0-9]{30,}\b",
        r"\bgithub_pat_[A-Za-z0-9_]{40,}\b",
        r"\bglpat-[A-Za-z0-9_\-]{20,}",
        // OpenAI, Anthropic and other `sk-` keys; Stripe.
        r"\bsk-[A-Za-z0-9_\-]{20,}",
        r"\b(?:sk|rk|pk)_(?:live|test)_[A-Za-z0-9]{16,}\b",
        // Slack tokens and webhooks.
        r"\bxox[abposr]-[A-Za-z0-9\-]{10,}",
        r"https://hooks\.slack\.com/services/[A-Za-z0-9/_\-]+",
        // Google API keys.
        r"\bAIza[0-9A-Za-z_\-]{35}",
        // npm, Hugging Face, PyPI, DigitalOcean, Shopify, SendGrid.
        r"\bnpm_[A-Za-z0-9]{36}\b",
        r"\bhf_[A-Za-z0-9]{30,}\b",
        r"\bpypi-[A-Za-z0-9_\-]{50,}",
        r"\bdop_v1_[a-f0-9]{64}\b",
        r"\bshp(?:at|ss|ca|pa)_[a-fA-F0-9]{32}\b",
        r"\bSG\.[A-Za-z0-9_\-]{16,}\.[A-Za-z0-9_\-]{16,}",
        // JSON Web Tokens.
        r"\beyJ[A-Za-z0-9_\-]{8,}\.eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}",
    ];
    Regex::new(&formats.join("|")).unwrap()
});

/// `https://user:password@host`: the user stays. The user can be empty (`redis://:pass@host`).
static URL_PASSWORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b([a-z][a-z0-9+.\-]*://[^\s:/@]*:)([^\s@/]+)(@)").unwrap());

/// `Authorization: Bearer abc`, `-H 'X-Api-Key: abc'`: the header name and scheme stay.
static AUTH_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b((?:proxy-)?authorization|x-api-key|api-key|x-auth-token|private-token)(\s*[:=]\s*)((?:bearer|basic|token|digest)\s+)?([^\s'"]+)"#,
    )
    .unwrap()
});

static COOKIE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\b((?:set-)?cookie)(\s*:\s*)([^'"\n]+)"#).unwrap());

/// `DB_PASSWORD=x`, `"api_key": "x"`, `token: x`: the name stays.
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?i)(["']?\b[\w.\-]*{SECRET_NAME}[\w.\-]*["']?)(\s*[:=]\s*)({VALUE})"#
    ))
    .unwrap()
});

/// `--password x`, `--token=x`, `-p x` after a name that says so.
static FLAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?i)(\s--?[\w\-]*{SECRET_NAME}[\w\-]*)(=|\s+)({VALUE})"#
    ))
    .unwrap()
});

/// `curl -u user:pass`, `mysql -psecret`, `sshpass -p secret`.
static INLINE_PASSWORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(\s(?:-u|--user)\s+[^\s:'"]+:)([^\s'"]+)|(\b(?:mysql|mysqldump|mariadb)\b[^\n]*?\s-p)([^\s'"]+)|(\bsshpass\s+-p\s*)([^\s'"]+)"#,
    )
    .unwrap()
});

/// Candidates for "a long random string": letters, digits and the base64/url-safe symbols
/// (`/` included: AWS secret keys and plain base64 have it).
static OPAQUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9+/_\-]{32,}={0,2}").unwrap());

pub fn redact(text: &str) -> Redacted {
    let mut count = 0;
    let mut text = PRIVATE_KEY
        .replace_all(text, |_: &Captures| {
            count += 1;
            "[REDACTED PRIVATE KEY]"
        })
        .into_owned();
    text = TOKENS
        .replace_all(&text, |_: &Captures| {
            count += 1;
            PLACEHOLDER
        })
        .into_owned();
    // The patterns below keep a prefix (the name, the user, the flag) and replace the value.
    let keep_prefix = |re: &Regex, text: &str, value: usize, count: &mut usize| -> String {
        re.replace_all(text, |caps: &Captures| {
            let whole = caps.get(0).unwrap();
            let Some(secret) = caps.get(value) else {
                return whole.as_str().to_string();
            };
            if already_redacted(secret.as_str()) {
                return whole.as_str().to_string();
            }
            *count += 1;
            let quote = secret
                .as_str()
                .chars()
                .next()
                .filter(|c| matches!(c, '"' | '\''));
            let start = secret.start() - whole.start();
            let end = secret.end() - whole.start();
            let mut out = whole.as_str()[..start].to_string();
            match quote {
                Some(q) => out.extend([q.to_string(), PLACEHOLDER.into(), q.to_string()]),
                None => out.push_str(PLACEHOLDER),
            }
            out.push_str(&whole.as_str()[end..]);
            out
        })
        .into_owned()
    };
    text = keep_prefix(&URL_PASSWORD, &text, 2, &mut count);
    text = keep_prefix(&AUTH_HEADER, &text, 4, &mut count);
    text = keep_prefix(&COOKIE, &text, 3, &mut count);
    // Each alternative has its own value group: 2, 4 or 6.
    text = INLINE_PASSWORD
        .replace_all(&text, |caps: &Captures| {
            let (prefix, secret) = [(1, 2), (3, 4), (5, 6)]
                .into_iter()
                .find_map(|(p, s)| Some((caps.get(p)?, caps.get(s)?)))
                .unwrap();
            if already_redacted(secret.as_str()) {
                return caps[0].to_string();
            }
            count += 1;
            format!("{}{PLACEHOLDER}", prefix.as_str())
        })
        .into_owned();
    // After `-u user:pass` and `-psecret`, so `-psecret` doesn't read as a `--secret` flag.
    text = keep_prefix(&ASSIGNMENT, &text, 3, &mut count);
    text = keep_prefix(&FLAG, &text, 3, &mut count);
    text = OPAQUE
        .replace_all(&text, |caps: &Captures| {
            let candidate = &caps[0];
            if looks_random(candidate) {
                count += 1;
                PLACEHOLDER.to_string()
            } else {
                candidate.to_string()
            }
        })
        .into_owned();
    Redacted { text, count }
}

/// A value that an earlier pattern already replaced (`VALUE` stops before its `]`).
fn already_redacted(value: &str) -> bool {
    value
        .trim_start_matches(['"', '\''])
        .starts_with("[REDACTED")
}

/// A string with no name to go by: secret if it reads as random. Hex alone (commit hashes,
/// checksums) and identifiers made of words don't count.
fn looks_random(text: &str) -> bool {
    let core = text.trim_end_matches('=');
    // With `/` it may be a path: those have a lowercase word between slashes (`src`, `lib`),
    // which a random string rarely does.
    if core.contains('/')
        && core
            .split('/')
            .any(|piece| piece.len() >= 3 && piece.chars().all(|c| c.is_ascii_lowercase()))
    {
        return false;
    }
    if core.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return false;
    }
    let has = |f: fn(&char) -> bool| core.chars().any(|c| f(&c));
    let classes = [
        has(char::is_ascii_lowercase),
        has(char::is_ascii_uppercase),
        has(char::is_ascii_digit),
    ]
    .into_iter()
    .filter(|&b| b)
    .count();
    classes == 3 && entropy(core) >= 4.0
}

/// Shannon entropy in bits per character.
fn entropy(text: &str) -> f64 {
    let mut counts = std::collections::HashMap::new();
    for c in text.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
    }
    let len = text.chars().count() as f64;
    counts
        .values()
        .map(|&n| {
            let p = n as f64 / len;
            -p * p.log2()
        })
        .sum()
}

/// Commands whose output is mostly secrets (environment dumps, key files, password managers):
/// only the command itself goes to the agent, never what it printed.
pub fn sensitive_command(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    let Some(&program) = words.first() else {
        return false;
    };
    let program = program.rsplit('/').next().unwrap_or(program);
    let args = &words[1..];
    let has = |word: &str| args.contains(&word);
    let secret_file = |arg: &&str| {
        let name = arg.rsplit('/').next().unwrap_or(arg).to_ascii_lowercase();
        name.starts_with(".env")
            || name.ends_with(".env")
            || name.ends_with(".pem")
            || name.ends_with(".key")
            || name.ends_with(".p12")
            || name.ends_with(".pfx")
            || name.starts_with("id_rsa")
            || name.starts_with("id_ed25519")
            || name.starts_with("id_ecdsa")
            || name.contains("secret")
            || name.contains("credential")
            || matches!(
                name.as_str(),
                ".netrc" | ".npmrc" | ".pypirc" | ".pgpass" | "kubeconfig"
            )
            || arg.contains(".aws/")
            || arg.contains(".kube/")
            || arg.contains(".ssh/")
            || arg.contains(".docker/")
            || arg.contains(".gnupg/")
    };
    match program {
        "env" | "printenv" | "set" | "declare" | "typeset" => {
            args.is_empty() || args.iter().all(|a| a.starts_with('-'))
        }
        "export" => args.is_empty() || has("-p"),
        "security" => args.iter().any(|a| a.contains("password")),
        "pass" | "gopass" | "op" | "bw" | "lpass" | "doppler" | "infisical" => true,
        "gpg" => has("-d") || has("--decrypt"),
        "vault" => has("read") || has("kv"),
        "kubectl" | "oc" => args.iter().any(|a| a.starts_with("secret")),
        "aws" => has("configure") || has("secretsmanager") || has("ssm"),
        "gh" | "glab" => has("auth") && has("token"),
        "heroku" | "fly" | "flyctl" => has("config") || has("secrets"),
        "gcloud" => has("print-access-token") || has("print-identity-token"),
        "cat" | "bat" | "less" | "more" | "head" | "tail" | "grep" | "rg" | "base64" | "xxd"
        | "strings" | "jq" | "yq" | "openssl" => args.iter().any(secret_file),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_empty_users_and_base64_with_slashes_but_not_paths() {
        let r = redact("redis://:hunter2secret@cache:6379");
        assert_eq!(r.text, "redis://:[REDACTED]@cache:6379");
        let r = redact("key wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY here");
        assert_eq!(r.text, "key [REDACTED] here");
        let path = "see /Users/Dev2/src/components/Button2/IndexV3Helper9.tsx";
        assert_eq!(redact(path).text, path);
    }

    fn clean(text: &str) -> String {
        redact(text).text
    }

    #[test]
    fn known_token_formats_go_wherever_they_are() {
        // Fakes in each service's format, split so the source never holds a whole token and
        // secret scanners don't take them for real ones.
        let cases = [
            concat!("AKIA", "IOSFODNN7EXAMPLE"),
            concat!("ghp_", "0123456789abcdefghijABCDEFGHIJklmnop"),
            concat!(
                "github_pat_",
                "11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyzABCDEFG"
            ),
            concat!("sk-ant-", "api03-abcdefghijklmnopqrstuvwxyz012345"),
            concat!("sk_live_", "51H8abcdefghijklmnop"),
            concat!("xoxb-", "1234567890-abcdefghij"),
            concat!("AIza", "SyA1234567890abcdefghijklmnopqrstuv"),
            concat!("npm_", "abcdefghijklmnopqrstuvwxyz0123456789"),
            concat!(
                "eyJhbGciOiJIUzI1NiJ9.",
                "eyJzdWIiOiIxMjM0NTY3ODkwIn0.",
                "dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"
            ),
        ];
        for secret in cases {
            let redacted = redact(&format!("error: bad key {secret} (401)"));
            assert_eq!(redacted.text, "error: bad key [REDACTED] (401)", "{secret}");
            assert_eq!(redacted.count, 1, "{secret}");
        }
    }

    #[test]
    fn private_keys_are_dropped_whole() {
        let text = concat!(
            "before\n-----BEGIN OPENSSH ",
            "PRIVATE KEY-----\nb3BlbnNzaC1rZXk\nAAAA\n-----END OPENSSH ",
            "PRIVATE KEY-----\nafter"
        );
        assert_eq!(clean(text), "before\n[REDACTED PRIVATE KEY]\nafter");
        // Cut off by the output limit: everything from the header on.
        assert_eq!(
            clean(concat!("x\n-----BEGIN RSA ", "PRIVATE KEY-----\nMIIE")),
            "x\n[REDACTED PRIVATE KEY]"
        );
    }

    #[test]
    fn named_values_keep_their_name() {
        assert_eq!(
            clean("export OPENAI_API_KEY=abc123 && run"),
            "export OPENAI_API_KEY=[REDACTED] && run"
        );
        assert_eq!(
            clean(r#"{"password": "hunter2", "user": "bob"}"#),
            r#"{"password": "[REDACTED]", "user": "bob"}"#
        );
        assert_eq!(
            clean("db_password: x\nuser: bob\ntoken: abc"),
            "db_password: [REDACTED]\nuser: bob\ntoken: [REDACTED]"
        );
        assert_eq!(
            clean("psql --password=hunter2 -h db"),
            "psql --password=[REDACTED] -h db"
        );
        assert_eq!(
            clean("deploy --api-key abc --env prod"),
            "deploy --api-key [REDACTED] --env prod"
        );
        assert_eq!(
            clean("DATABASE_PASSWORD='a b c'"),
            "DATABASE_PASSWORD='[REDACTED]'"
        );
    }

    #[test]
    fn credentials_in_urls_headers_and_flags() {
        assert_eq!(
            clean("git clone https://bob:s3cret@github.com/x/y.git"),
            "git clone https://bob:[REDACTED]@github.com/x/y.git"
        );
        assert_eq!(
            clean("postgres://app:pw@localhost:5432/db"),
            "postgres://app:[REDACTED]@localhost:5432/db"
        );
        assert_eq!(
            clean("curl -H 'Authorization: Bearer abc.def' https://api"),
            "curl -H 'Authorization: Bearer [REDACTED]' https://api"
        );
        assert_eq!(
            clean("curl -u admin:pa55 https://x"),
            "curl -u admin:[REDACTED] https://x"
        );
        assert_eq!(
            clean("mysql -u root -psecret db"),
            "mysql -u root -p[REDACTED] db"
        );
        assert_eq!(clean("Cookie: a=1; b=2"), "Cookie: [REDACTED]");
    }

    #[test]
    fn random_strings_go_but_hashes_paths_and_words_stay() {
        let redacted = redact(concat!(
            "key Zx8kQ2mP9vL4n",
            "R7tW1yB6cF3hJ5sD0gA was rejected"
        ));
        assert_eq!(redacted.text, "key [REDACTED] was rejected");
        let kept = [
            "commit 9fceb02d0ae598e95dc970b74767f19372d61af8",
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "id 123e4567-e89b-12d3-a456-426614174000",
            "/usr/local/lib/node_modules/typescript/lib/tsc.js",
            "error[E0425]: cannot find value `very_long_identifier_name_here` in this scope",
            "warning: unused_variables_in_function_number_42",
            "Author: Ángel <me@example.com>",
        ];
        for text in kept {
            assert_eq!(
                redact(text),
                Redacted {
                    text: text.into(),
                    count: 0
                },
                "{text}"
            );
        }
    }

    #[test]
    fn redacting_twice_changes_nothing() {
        let once = clean(concat!(
            "TOKEN=abc https://u:p@h ghp_",
            "0123456789abcdefghijABCDEFGHIJklmnop"
        ));
        assert_eq!(redact(&once).count, 0);
    }

    #[test]
    fn spots_commands_that_print_secrets() {
        for command in [
            "env",
            "printenv",
            "cat .env",
            "cat config/.env.local",
            "less ~/.aws/credentials",
            "cat ~/.ssh/id_ed25519",
            "gh auth token",
            "security find-generic-password -w -s x",
            "kubectl get secret db -o yaml",
            "op read op://vault/item",
            "/usr/bin/env",
        ] {
            assert!(sensitive_command(command), "{command}");
        }
        for command in [
            "env FOO=1 make",
            "cat README.md",
            "git push",
            "kubectl get pods",
            "ls .ssh",
        ] {
            assert!(!sensitive_command(command), "{command}");
        }
    }
}
