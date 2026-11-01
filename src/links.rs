//! Links in terminal output: URLs written as plain text, which schemes may be opened, and file
//! paths with line and column (`src/main.rs:12:5`) like the ones compilers, grep and test
//! runners print.

use std::{
    ops::Range,
    path::{Path, PathBuf},
};

/// Schemes opened with the browser or the default app. Anything else (for example one sent by
/// a program via OSC 8) is ignored, so a command's output can't launch apps with arbitrary
/// URLs.
const ALLOWED_SCHEMES: &[&str] = &["http", "https", "ftp", "file", "mailto"];

pub fn is_openable(url: &str) -> bool {
    url.split_once(':').is_some_and(|(scheme, rest)| {
        ALLOWED_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()) && !rest.is_empty()
    })
}

fn is_url_char(c: char) -> bool {
    !c.is_whitespace()
        && !c.is_control()
        && !matches!(c, '<' | '>' | '"' | '`' | '{' | '}' | '|' | '\\' | '^')
}

/// URLs in `text`, as byte ranges. Recognizes `scheme://…` and `mailto:…`, and trims trailing
/// punctuation that usually ends a sentence (`.`, `,`, an unmatched `)`…).
pub fn find_urls(text: &str) -> Vec<Range<usize>> {
    let mut urls = Vec::new();
    let mut search_from = 0;
    while search_from < text.len() {
        let rest = &text[search_from..];
        let Some(found) = ALLOWED_SCHEMES
            .iter()
            .filter_map(|scheme| {
                let marker = if *scheme == "mailto" {
                    "mailto:".to_string()
                } else {
                    format!("{scheme}://")
                };
                rest.to_ascii_lowercase()
                    .find(&marker)
                    .map(|i| (i, marker.len()))
            })
            .min_by_key(|(i, _)| *i)
        else {
            break;
        };
        let (offset, marker_len) = found;
        let start = search_from + offset;
        // The scheme can't be the tail of another word (`xhttp://`).
        let preceded_by_word = text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric());

        let body_start = start + marker_len;
        let body_len = text[body_start..]
            .char_indices()
            .find(|(_, c)| !is_url_char(*c))
            .map_or(text.len() - body_start, |(i, _)| i);
        let end = start + trim_trailing(&text[start..body_start + body_len]);

        if !preceded_by_word && end > start + marker_len {
            urls.push(start..end);
        }
        search_from = end.max(start + marker_len);
    }
    urls
}

/// Length of `url` without trailing punctuation that isn't part of it.
fn trim_trailing(url: &str) -> usize {
    let mut end = url.len();
    loop {
        let Some(last) = url[..end].chars().next_back() else {
            return end;
        };
        let unbalanced = |open: char, close: char| {
            last == close && url[..end].matches(open).count() < url[..end].matches(close).count()
        };
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*')
            || unbalanced('(', ')')
            || unbalanced('[', ']')
        {
            end -= last.len_utf8();
        } else {
            return end;
        }
    }
}

/// A file to open in the editor, with its position if the output includes one.
#[derive(Clone, Debug, PartialEq)]
pub struct FileLink {
    pub path: PathBuf,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// A path found in the output, not yet checked for existence.
#[derive(Clone, Debug, PartialEq)]
pub struct PathMatch {
    /// Bytes of `text` to underline: the path and its position.
    pub range: Range<usize>,
    pub path: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

fn is_path_char(c: char) -> bool {
    !c.is_whitespace()
        && !c.is_control()
        && !matches!(
            c,
            '"' | '\''
                | '`'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '<'
                | '>'
                | ','
                | ';'
                | '|'
                | '*'
                | '?'
                | '!'
                | '&'
                | '$'
                | '='
        )
}

/// Leading digits of `text` as a number, and how many bytes they take.
fn leading_number(text: &str) -> Option<(u32, usize)> {
    let len = text.bytes().take_while(u8::is_ascii_digit).count();
    Some((text[..len].parse().ok()?, len))
}

/// Paths in `text`, with the line and column that come with them:
/// `src/main.rs:12:5` (rustc, grep, go, pytest), `src/a.ts(12,5)` (tsc) and
/// `File "app.py", line 7` (Python). Candidates only: check they exist with `resolve_path`.
pub fn find_paths(text: &str) -> Vec<PathMatch> {
    let mut found = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if !is_path_char(c) {
            continue;
        }
        let mut end = start + c.len_utf8();
        while let Some(&(ix, c)) = chars.peek() {
            if !is_path_char(c) {
                break;
            }
            end = ix + c.len_utf8();
            chars.next();
        }
        if let Some(found_path) = parse_path(text, start, end) {
            found.push(found_path);
        }
    }
    found
}

fn parse_path(text: &str, start: usize, end: usize) -> Option<PathMatch> {
    let token = &text[start..end];
    // The path runs up to the first `:`; line and column may follow.
    let (mut path, suffix) = match token.find(':') {
        Some(colon) => (&token[..colon], Some(&token[colon + 1..])),
        None => (token, None),
    };
    let mut line = None;
    let mut column = None;
    let mut range_end = start + path.len();
    if let Some((number, len)) = suffix.and_then(leading_number) {
        line = Some(number);
        range_end += 1 + len;
        let rest = &suffix.unwrap_or_default()[len..];
        if let Some((number, len)) = rest.strip_prefix(':').and_then(leading_number) {
            column = Some(number);
            range_end += 1 + len;
        }
    }
    let after = &text[end..];
    if line.is_none() {
        // tsc: `file.ts(12,5)`.
        if let Some(inside) = after.strip_prefix('(')
            && let Some((number, len)) = leading_number(inside)
        {
            let rest = &inside[len..];
            let column_part = rest.strip_prefix(',').and_then(leading_number);
            let close = 1 + len + column_part.map_or(0, |(_, len)| 1 + len);
            if inside[close - 1..].starts_with(')') {
                line = Some(number);
                column = column_part.map(|(number, _)| number);
                range_end = end + close + 1;
            }
        }
        // Python: `File "app.py", line 7`.
        if text[..start].ends_with('"')
            && let Some(rest) = after.strip_prefix("\", line ")
            && let Some((number, _)) = leading_number(rest)
        {
            line = Some(number);
        }
        // Without a position, trailing punctuation belongs to the sentence (`see README.md.`).
        let trimmed = path.trim_end_matches(['.', ':']);
        range_end -= path.len() - trimmed.len();
        path = trimmed;
    }
    let looks_like_path = path.contains('/')
        || path.rsplit_once('.').is_some_and(|(stem, ext)| {
            // `main.rs`, `Cargo.toml`; not `1.5s` or `v0.1.0`.
            stem.chars().any(char::is_alphabetic)
                && ext.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        });
    if !looks_like_path || path.contains("://") || path.chars().all(|c| c == '.' || c == '/') {
        return None;
    }
    Some(PathMatch {
        range: start..range_end,
        path: path.to_string(),
        line,
        column,
    })
}

/// The file `path` points to, if it exists: relative to `cwd`, `~`-based or absolute. Also
/// tries without the `a/` or `b/` prefix that `git diff` adds.
pub fn resolve_path(path: &str, cwd: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let full = |path: &str| -> Option<PathBuf> {
        if let Some(rest) = path.strip_prefix("~/") {
            Some(home?.join(rest))
        } else {
            Some(cwd.join(path))
        }
    };
    let candidates = [Some(path), path.strip_prefix("a/"), path.strip_prefix("b/")];
    candidates
        .into_iter()
        .flatten()
        .filter_map(full)
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(text: &str) -> Vec<(&str, Option<u32>, Option<u32>)> {
        find_paths(text)
            .into_iter()
            .map(|m| (&text[m.range], m.line, m.column))
            .collect()
    }

    #[test]
    fn finds_paths_from_compilers_grep_and_tests() {
        assert_eq!(
            paths("  --> src/main.rs:12:5"),
            vec![("src/main.rs:12:5", Some(12), Some(5))]
        );
        assert_eq!(
            paths("src/lib.rs:3:fn main() {"),
            vec![("src/lib.rs:3", Some(3), None)]
        );
        assert_eq!(
            paths("./main.go:10:2: undefined: x"),
            vec![("./main.go:10:2", Some(10), Some(2))]
        );
        assert_eq!(
            paths("tests/test_a.py:42: AssertionError"),
            vec![("tests/test_a.py:42", Some(42), None)]
        );
        assert_eq!(
            paths("src/app.ts(12,5): error TS2322"),
            vec![("src/app.ts(12,5)", Some(12), Some(5))]
        );
        assert_eq!(
            paths("  File \"/srv/app/views.py\", line 7, in index"),
            vec![("/srv/app/views.py", Some(7), None)]
        );
    }

    #[test]
    fn finds_plain_paths_without_sentence_punctuation() {
        assert_eq!(paths("see README.md."), vec![("README.md", None, None)]);
        assert_eq!(
            paths("modified:   src/terminal.rs"),
            vec![("src/terminal.rs", None, None)]
        );
        assert_eq!(
            paths("~/notes/todo.txt"),
            vec![("~/notes/todo.txt", None, None)]
        );
    }

    #[test]
    fn ignores_words_versions_and_urls() {
        assert!(paths("compiling moth v0.1.0 ...").is_empty());
        assert!(paths("1.5s  ok").is_empty());
        assert!(paths("https://example.com/a.html").is_empty());
        assert!(paths(".. / ...").is_empty());
    }

    #[test]
    fn resolves_relative_home_and_git_diff_paths() {
        let dir = std::env::temp_dir().join(format!("moth-links-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "").unwrap();
        let file = dir.join("src/main.rs");
        assert_eq!(resolve_path("src/main.rs", &dir, None), Some(file.clone()));
        assert_eq!(
            resolve_path("b/src/main.rs", &dir, None),
            Some(file.clone())
        );
        assert_eq!(
            resolve_path("~/src/main.rs", Path::new("/"), Some(&dir)),
            Some(file.clone())
        );
        assert_eq!(
            resolve_path(file.to_str().unwrap(), Path::new("/"), None),
            Some(file)
        );
        // Folders don't open in the editor.
        assert_eq!(resolve_path("src", &dir, None), None);
        std::fs::remove_dir_all(dir).ok();
    }

    fn urls(text: &str) -> Vec<&str> {
        find_urls(text).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn finds_urls_in_output() {
        assert_eq!(
            urls("remote: Create a pull request: https://github.com/you/moth/pull/new/feat"),
            vec!["https://github.com/you/moth/pull/new/feat"]
        );
        assert_eq!(
            urls("see http://localhost:3000/api?q=1&x=2 and ftp://files.example.com/a.tgz"),
            vec![
                "http://localhost:3000/api?q=1&x=2",
                "ftp://files.example.com/a.tgz"
            ]
        );
        assert_eq!(
            urls("write to mailto:dev@example.com."),
            vec!["mailto:dev@example.com"]
        );
        assert_eq!(
            urls("file:///Users/dev/report.html"),
            vec!["file:///Users/dev/report.html"]
        );
    }

    #[test]
    fn trims_sentence_punctuation_and_unbalanced_parens() {
        assert_eq!(
            urls("(see https://example.com/docs)."),
            vec!["https://example.com/docs"]
        );
        assert_eq!(
            urls("https://en.wikipedia.org/wiki/Rust_(programming_language)"),
            vec!["https://en.wikipedia.org/wiki/Rust_(programming_language)"]
        );
        assert_eq!(
            urls("<https://example.com/a>"),
            vec!["https://example.com/a"]
        );
        assert_eq!(
            urls("\"https://example.com/b\","),
            vec!["https://example.com/b"]
        );
    }

    #[test]
    fn ignores_non_urls() {
        assert!(urls("https:// no host").is_empty());
        assert!(urls("xhttp://nope.com").is_empty());
        assert!(urls("just text, no links here").is_empty());
    }

    #[test]
    fn only_safe_schemes_open() {
        assert!(is_openable("https://example.com"));
        assert!(is_openable("mailto:a@b.c"));
        assert!(is_openable("file:///tmp/x"));
        assert!(!is_openable("javascript:alert(1)"));
        assert!(!is_openable("vscode://open?x"));
        assert!(!is_openable("https:"));
    }
}
