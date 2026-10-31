//! Bloques de comandos: prompt, línea escrita y salida.
//!
//! Cada prompt deja en la grilla una marca invisible: caracteres de ancho cero que el escáner
//! del PTY inserta justo después de `OSC 133;B` y que alacritty guarda pegados a la última celda
//! del prompt. Así la marca viaja con el texto al hacer scroll, al redimensionar la ventana y
//! cuando el scrollback descarta líneas viejas; una fila guardada aparte se desfasaría.

use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Line, Point},
    term::{Term, cell::Flags},
};

/// Marca de un prompt cuyo comando anterior terminó bien / con error.
const MARK_OK: char = '\u{2061}';
const MARK_FAILED: char = '\u{2062}';
/// Una por cada fila del prompt por encima de la que tiene la marca.
const MARK_ROW: char = '\u{2063}';
const MAX_PROMPT_ROWS: u8 = 8;

/// Lo que la integración cuenta del prompt en `OSC 133;B;rows=N;prev_err=0|1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PromptMark {
    /// Filas que ocupa el prompt, incluida la línea en blanco que lo separa del bloque anterior.
    pub rows: u8,
    /// El comando anterior terminó con error: el bloque anterior queda marcado al aparecer este.
    pub previous_failed: bool,
}

impl PromptMark {
    pub fn encode(self) -> String {
        let mut text = String::from(if self.previous_failed {
            MARK_FAILED
        } else {
            MARK_OK
        });
        let rows = self.rows.clamp(1, MAX_PROMPT_ROWS);
        text.extend(std::iter::repeat_n(MARK_ROW, usize::from(rows - 1)));
        text
    }

    /// Si zsh redibuja el prompt, la marca puede quedar repetida en la celda: vale la primera.
    fn decode(chars: &[char]) -> Option<Self> {
        let start = chars
            .iter()
            .position(|&c| c == MARK_OK || c == MARK_FAILED)?;
        let rows = 1 + chars[start + 1..]
            .iter()
            .take_while(|&&c| c == MARK_ROW)
            .count();
        Some(Self {
            rows: rows.min(usize::from(MAX_PROMPT_ROWS)) as u8,
            previous_failed: chars[start] == MARK_FAILED,
        })
    }
}

pub fn is_mark_char(c: char) -> bool {
    matches!(c, MARK_OK | MARK_FAILED | MARK_ROW)
}

/// Quita las marcas de un texto sacado de la grilla (copiar, por ejemplo).
pub fn strip_marks(text: &str) -> String {
    text.chars().filter(|&c| !is_mark_char(c)).collect()
}

/// Un comando con su prompt y su salida, en líneas de la grilla.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    /// Primera fila del prompt.
    pub start: Line,
    /// Fila donde empieza lo que se escribió (la de la marca).
    pub input: Line,
    /// Primera fila de la salida; si pasa de `end`, el comando no imprimió nada.
    pub output: Line,
    /// Última fila del bloque.
    pub end: Line,
    /// El comando terminó con error. Se sabe recién cuando aparece el prompt siguiente.
    pub failed: bool,
    /// La cola de un bloque cuyo prompt está más arriba de donde se empezó a buscar.
    pub partial: bool,
}

/// Bloques con alguna fila entre `top` y el final de la pantalla. Las filas de arriba de la
/// primera marca son la cola de un bloque cuyo prompt queda más arriba: van como bloque
/// `partial`, con su resultado, sin recorrer el scrollback para encontrarle el inicio.
pub fn blocks_from<T>(term: &Term<T>, top: Line) -> Vec<Block> {
    blocks_in(term, top, None)
}

/// Como `blocks_from`, pero deja de buscar en la primera marca después de `bottom`: alcanza
/// para saber si falló el último bloque visible, sin recorrer todo lo que queda abajo.
pub fn visible_blocks<T>(term: &Term<T>, top: Line, bottom: Line) -> Vec<Block> {
    blocks_in(term, top, Some(bottom))
}

fn blocks_in<T>(term: &Term<T>, top: Line, stop_after: Option<Line>) -> Vec<Block> {
    let grid = term.grid();
    let columns = term.columns();
    let mut marks: Vec<(Line, PromptMark)> = Vec::new();
    for line in (top.0..=term.bottommost_line().0).map(Line) {
        let row = &grid[line];
        let mark =
            (0..columns).find_map(|column| PromptMark::decode(row[Column(column)].zerowidth()?));
        if let Some(mark) = mark {
            marks.push((line, mark));
            if stop_after.is_some_and(|stop| line > stop) {
                break;
            }
        }
    }

    let start_of =
        |(line, mark): (Line, PromptMark)| Line((line.0 - i32::from(mark.rows) + 1).max(top.0));
    let cursor = grid.cursor.point.line;
    let mut blocks = Vec::new();
    if let Some(&first) = marks.first()
        && start_of(first) > top
    {
        blocks.push(Block {
            start: top,
            input: top,
            output: top,
            end: start_of(first) - 1,
            failed: first.1.previous_failed,
            partial: true,
        });
    }
    for (ix, &(line, mark)) in marks.iter().enumerate() {
        let next = marks.get(ix + 1).copied();
        let end = next
            .map_or(cursor.max(line), |next| start_of(next) - 1)
            .max(line);
        // Lo escrito sigue en las filas que partió el ancho de la terminal.
        let mut input_end = line;
        while input_end < end
            && grid[input_end][term.last_column()]
                .flags
                .contains(Flags::WRAPLINE)
        {
            input_end += 1;
        }
        blocks.push(Block {
            start: start_of((line, mark)),
            input: line,
            output: input_end + 1,
            end,
            failed: next.is_some_and(|(_, next)| next.previous_failed),
            partial: false,
        });
    }
    blocks
}

/// Los bloques completos ya ejecutados o en ejecución (sin el prompt donde está el cursor).
pub fn commands<T>(term: &Term<T>) -> Vec<Block> {
    let cursor = term.grid().cursor.point.line;
    blocks_from(term, term.topmost_line())
        .into_iter()
        .filter(|block| !block.partial && block.input < cursor)
        .collect()
}

/// Filas del bloque sin las líneas en blanco de los bordes (la que separa los prompts).
pub fn trimmed_rows<T>(term: &Term<T>, block: &Block) -> Option<(Line, Line)> {
    let blank = |line: Line| {
        let row = &term.grid()[line];
        (0..term.columns()).all(|column| matches!(row[Column(column)].c, ' ' | '\t'))
    };
    let mut start = block.start;
    let mut end = block.end;
    while start <= end && blank(start) {
        start += 1;
    }
    while end > start && blank(end) {
        end -= 1;
    }
    (start <= end).then_some((start, end))
}

/// La salida del comando como texto, sin las marcas ni el espacio final.
pub fn output_text<T>(term: &Term<T>, block: &Block) -> String {
    if block.output > block.end {
        return String::new();
    }
    let text = term.bounds_to_string(
        Point::new(block.output, Column(0)),
        Point::new(block.end, term.last_column()),
    );
    strip_marks(&text).trim_end().to_string()
}

/// Una fila que sigue a su texto aunque entren líneas al scrollback. Con el scrollback lleno
/// no hay cómo saber cuánto se corrió y puede quedar desfasada.
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    line: Line,
    history_size: usize,
}

impl Anchor {
    pub fn new<T>(line: Line, term: &Term<T>) -> Self {
        Self {
            line,
            history_size: term.grid().history_size(),
        }
    }

    pub fn resolve<T>(&self, term: &Term<T>) -> Option<Line> {
        let shift = term.grid().history_size().checked_sub(self.history_size)? as i32;
        let line = self.line - shift;
        (line >= term.topmost_line()).then_some(line)
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::{
        event::VoidListener,
        index::{Column, Line},
        term::Term,
        vte::ansi::Processor,
    };

    use super::*;

    fn term(columns: usize, lines: usize) -> Term<VoidListener> {
        let size = crate::terminal::TermSize {
            columns,
            lines,
            cell_width: gpui::px(8.),
            line_height: gpui::px(22.),
        };
        Term::new(Default::default(), &size, VoidListener)
    }

    /// Simula lo que hace el escáner: la marca va justo después del prompt.
    fn prompt(rows: u8, previous_failed: bool) -> String {
        format!(
            "❯ {}",
            PromptMark {
                rows,
                previous_failed
            }
            .encode()
        )
    }

    fn feed(term: &mut Term<VoidListener>, text: &str) {
        let mut parser: Processor = Processor::new();
        parser.advance(term, text.as_bytes());
    }

    #[test]
    fn marks_round_trip_and_tolerate_repeats() {
        let mark = PromptMark {
            rows: 3,
            previous_failed: true,
        };
        let chars: Vec<char> = mark.encode().chars().collect();
        assert_eq!(PromptMark::decode(&chars), Some(mark));
        let repeated: Vec<char> = mark.encode().repeat(2).chars().collect();
        assert_eq!(PromptMark::decode(&repeated), Some(mark));
        assert_eq!(strip_marks(&format!("a{}b", mark.encode())), "ab");
    }

    #[test]
    fn finds_blocks_their_output_and_failures() {
        let mut term = term(20, 12);
        let text = format!(
            "{}ls\r\nuno\r\ndos\r\n\r\n{}false\r\n\r\n{}",
            prompt(1, false),
            prompt(2, false),
            prompt(2, true),
        );
        feed(&mut term, &text);
        let blocks = blocks_from(&term, term.topmost_line());
        assert_eq!(blocks.len(), 3);
        // `ls`: entrada en la fila 0, salida en 1–2; la fila 3 es la línea en blanco del
        // prompt siguiente, que ocupa dos filas.
        assert_eq!(
            (blocks[0].input, blocks[0].output, blocks[0].end),
            (Line(0), Line(1), Line(2))
        );
        assert!(!blocks[0].failed);
        assert_eq!(output_text(&term, &blocks[0]), "uno\ndos");
        // `false` falló: lo dice la marca del prompt siguiente.
        assert_eq!((blocks[1].start, blocks[1].input), (Line(3), Line(4)));
        assert!(blocks[1].failed);
        assert_eq!(output_text(&term, &blocks[1]), "");
        assert_eq!(trimmed_rows(&term, &blocks[1]), Some((Line(4), Line(4))));
        // El prompt actual no es un comando.
        assert_eq!(commands(&term).len(), 2);
    }

    #[test]
    fn long_input_wraps_before_the_output() {
        let mut term = term(10, 8);
        feed(
            &mut term,
            &format!(
                "{}echo 123456789\r\nfin\r\n{}",
                prompt(1, false),
                prompt(1, false)
            ),
        );
        let blocks = commands(&term);
        assert_eq!(blocks[0].output, Line(2));
        assert_eq!(output_text(&term, &blocks[0]), "fin");
    }

    #[test]
    fn rows_above_the_first_mark_are_a_partial_block() {
        let mut term = term(20, 6);
        feed(&mut term, &format!("motd\r\n{}", prompt(1, false)));
        let blocks = blocks_from(&term, term.topmost_line());
        assert!(blocks[0].partial);
        assert_eq!(blocks[0].end, Line(0));
        assert!(commands(&term).is_empty());
    }

    #[test]
    fn anchors_follow_lines_into_the_scrollback() {
        let mut term = term(20, 3);
        feed(&mut term, "a\r\nb\r\n");
        let anchor = Anchor::new(Line(1), &term);
        feed(&mut term, "c\r\nd\r\n");
        let line = anchor.resolve(&term).unwrap();
        assert_eq!(term.grid()[line][Column(0)].c, 'b');
    }
}
