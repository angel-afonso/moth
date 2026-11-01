//! Links en la salida de la terminal: URLs escritas como texto, qué esquemas se pueden abrir y
//! rutas a archivos con línea y columna (`src/main.rs:12:5`), como las que imprimen los
//! compiladores, grep y los tests.

use std::{
    ops::Range,
    path::{Path, PathBuf},
};

/// Esquemas que se abren con el navegador o la app predeterminada. Cualquier otro (por
/// ejemplo uno que llega por OSC 8 desde un programa) se ignora, para que la salida de un
/// comando no pueda lanzar aplicaciones con URLs arbitrarias.
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

/// URLs dentro de `text`, como rangos en bytes. Reconoce `esquema://…` y `mailto:…`, y quita
/// la puntuación final que suele cerrar una oración (`.`, `,`, `)` sin pareja…).
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
        // El esquema no puede ser la cola de otra palabra (`xhttp://`).
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

/// Largo de `url` sin la puntuación final que no le pertenece.
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

/// Un archivo para abrir en el editor, con la posición si la salida la trae.
#[derive(Clone, Debug, PartialEq)]
pub struct FileLink {
    pub path: PathBuf,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// Una ruta escrita en la salida, todavía sin verificar que exista.
#[derive(Clone, Debug, PartialEq)]
pub struct PathMatch {
    /// Bytes de `text` que se subrayan: la ruta y su posición.
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

/// Dígitos al principio de `text`, como número, y cuántos bytes ocupan.
fn leading_number(text: &str) -> Option<(u32, usize)> {
    let len = text.bytes().take_while(u8::is_ascii_digit).count();
    Some((text[..len].parse().ok()?, len))
}

/// Rutas escritas en `text`, con la línea y columna que las acompañan:
/// `src/main.rs:12:5` (rustc, grep, go, pytest), `src/a.ts(12,5)` (tsc) y
/// `File "app.py", line 7` (Python). Solo candidatos: hay que ver si existen con `resolve_path`.
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
    // La ruta llega hasta el primer `:`; después pueden venir la línea y la columna.
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
        // tsc: `archivo.ts(12,5)`.
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
        // Sin posición, la puntuación final es de la oración (`ver README.md.`).
        let trimmed = path.trim_end_matches(['.', ':']);
        range_end -= path.len() - trimmed.len();
        path = trimmed;
    }
    let looks_like_path = path.contains('/')
        || path.rsplit_once('.').is_some_and(|(stem, ext)| {
            // `main.rs`, `Cargo.toml`; no `1.5s` ni `v0.1.0`.
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

/// El archivo al que apunta `path`, si existe: relativo a `cwd`, con `~` o absoluto. Prueba
/// también sin el `a/` o `b/` que agrega `git diff`.
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
        assert_eq!(paths("ver README.md."), vec![("README.md", None, None)]);
        assert_eq!(
            paths("modified:   src/terminal.rs"),
            vec![("src/terminal.rs", None, None)]
        );
        assert_eq!(
            paths("~/notas/todo.txt"),
            vec![("~/notas/todo.txt", None, None)]
        );
    }

    #[test]
    fn ignores_words_versions_and_urls() {
        assert!(paths("compilando moth v0.1.0 ...").is_empty());
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
        // Las carpetas no se abren en el editor.
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
            urls("ver http://localhost:3000/api?q=1&x=2 y ftp://files.example.com/a.tgz"),
            vec![
                "http://localhost:3000/api?q=1&x=2",
                "ftp://files.example.com/a.tgz"
            ]
        );
        assert_eq!(
            urls("escribí a mailto:dev@example.com."),
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
            urls("(ver https://example.com/docs)."),
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
        assert!(urls("https:// sin host").is_empty());
        assert!(urls("xhttp://nope.com").is_empty());
        assert!(urls("solo texto, nada de links").is_empty());
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
