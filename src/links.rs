//! Links en la salida de la terminal: URLs escritas como texto y qué esquemas se pueden abrir.

use std::ops::Range;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(text: &str) -> Vec<&str> {
        find_urls(text).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn finds_urls_in_output() {
        assert_eq!(
            urls("remote: Create a pull request: https://github.com/you/terminalapp/pull/new/feat"),
            vec!["https://github.com/you/terminalapp/pull/new/feat"]
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
