//! Coincidencia difusa por subsecuencia, con puntaje y posiciones para resaltar.

/// Resultado de comparar una consulta con un texto: mayor puntaje = mejor coincidencia.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub score: i32,
    /// Índices en bytes de los caracteres que coinciden (para `HighlightedLabel`).
    pub positions: Vec<usize>,
}

/// `None` si los caracteres de `query` no aparecen en orden dentro de `text`.
/// Ignora mayúsculas y espacios de la consulta.
pub fn fuzzy_match(query: &str, text: &str) -> Option<Match> {
    let query: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if query.is_empty() {
        return Some(Match {
            score: 0,
            positions: Vec::new(),
        });
    }

    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let lower: Vec<char> = chars
        .iter()
        .map(|&(_, c)| c.to_lowercase().next().unwrap_or(c))
        .collect();

    // Si la consulta aparece literal, se prefiere esa ubicación (la primera).
    let haystack: String = lower.iter().collect();
    let needle: String = query.iter().collect();
    if let Some(byte_start) = haystack.find(&needle) {
        let start = haystack[..byte_start].chars().count();
        let positions = (start..start + query.len()).map(|i| chars[i].0).collect();
        let boundary = start == 0 || is_boundary(chars[start - 1].1);
        let score = 1000 + query.len() as i32 * 10 + if boundary { 200 } else { 0 }
            - start as i32
            - (chars.len() as i32 / 8);
        return Some(Match { score, positions });
    }

    // Si no, subsecuencia codiciosa con bonos por inicio de palabra y por caracteres seguidos.
    let mut positions = Vec::with_capacity(query.len());
    let mut score = 0;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    for (i, &c) in lower.iter().enumerate() {
        if qi == query.len() {
            break;
        }
        if c != query[qi] {
            continue;
        }
        score += 10;
        if i == 0 || is_boundary(chars[i - 1].1) {
            score += 30;
        }
        match last {
            Some(prev) if prev + 1 == i => score += 20,
            Some(prev) => score -= ((i - prev - 1) as i32).min(10),
            None => score -= (i as i32).min(15),
        }
        positions.push(chars[i].0);
        last = Some(i);
        qi += 1;
    }
    (qi == query.len()).then(|| Match {
        score: score - (chars.len() as i32 / 8),
        positions,
    })
}

fn is_boundary(c: char) -> bool {
    c.is_whitespace() || matches!(c, '-' | '_' | '/' | '.' | ':' | '=' | '\'' | '"')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_subsequences_case_insensitively() {
        let m = fuzzy_match("gco", "git checkout").unwrap();
        assert_eq!(m.positions, vec![0, 4, 9]);
        assert!(fuzzy_match("GIT", "git status").is_some());
        assert!(fuzzy_match("xyz", "git status").is_none());
    }

    #[test]
    fn prefers_literal_and_word_starts() {
        let literal = fuzzy_match("push", "git push origin").unwrap();
        let scattered = fuzzy_match("push", "pnpm run build:ush").unwrap();
        assert!(literal.score > scattered.score);

        let start = fuzzy_match("nueva", "Nueva pestaña").unwrap();
        let middle = fuzzy_match("nueva", "Abrir nueva ventana").unwrap();
        assert!(start.score > middle.score);
    }

    #[test]
    fn positions_are_byte_offsets() {
        let m = fuzzy_match("pest", "Cerrar pestaña").unwrap();
        assert_eq!(m.positions, vec![7, 8, 9, 10]);
        let m = fuzzy_match("ña", "pestaña").unwrap();
        assert_eq!(&"pestaña"[m.positions[0]..m.positions[0] + 2], "ñ");
    }

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(fuzzy_match("  ", "lo que sea").unwrap().score, 0);
    }
}
