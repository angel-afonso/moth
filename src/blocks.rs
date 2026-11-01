//! Command blocks: prompt, typed input and output.
//!
//! Each prompt leaves an invisible mark in the grid: zero-width characters that the PTY scanner
//! inserts right after `OSC 133;B` and that alacritty stores attached to the prompt's last cell.
//! That way the mark travels with the text when scrolling, when the window is resized and when
//! the scrollback drops old lines; a row number stored separately would drift.

use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Line, Point},
    term::{Term, cell::Flags},
};

/// Mark for a prompt whose previous command succeeded / failed.
const MARK_OK: char = '\u{2061}';
const MARK_FAILED: char = '\u{2062}';
/// One per prompt row above the row that holds the mark.
const MARK_ROW: char = '\u{2063}';
const MAX_PROMPT_ROWS: u8 = 8;

/// What the shell integration reports about the prompt in `OSC 133;B;rows=N;prev_err=0|1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PromptMark {
    /// Rows the prompt takes up, including the blank line separating it from the previous block.
    pub rows: u8,
    /// The previous command failed: the previous block gets flagged when this one appears.
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

    /// If zsh redraws the prompt, the mark may be repeated in the cell: the first one wins.
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

/// Strips the marks from text taken out of the grid (when copying, for example).
pub fn strip_marks(text: &str) -> String {
    text.chars().filter(|&c| !is_mark_char(c)).collect()
}

/// A command with its prompt and output, as grid lines.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    /// First row of the prompt.
    pub start: Line,
    /// Row where the typed input starts (the one with the mark).
    pub input: Line,
    /// First row of the output; if it's past `end`, the command printed nothing.
    pub output: Line,
    /// Last row of the block.
    pub end: Line,
    /// The command failed. Only known once the next prompt appears.
    pub failed: bool,
    /// The tail of a block whose prompt is above where the search started.
    pub partial: bool,
}

/// Blocks with at least one row between `top` and the bottom of the screen. Rows above the
/// first mark are the tail of a block whose prompt is further up: they come back as a `partial`
/// block, with its result, without walking the scrollback to find its start.
pub fn blocks_from<T>(term: &Term<T>, top: Line) -> Vec<Block> {
    blocks_in(term, top, None)
}

/// Like `blocks_from`, but stops at the first mark after `bottom`: enough to know whether the
/// last visible block failed, without scanning everything below it.
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
        // The input continues on the rows the terminal width wrapped it onto.
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

/// Complete blocks that already ran or are running (excluding the prompt under the cursor).
pub fn commands<T>(term: &Term<T>) -> Vec<Block> {
    let cursor = term.grid().cursor.point.line;
    blocks_from(term, term.topmost_line())
        .into_iter()
        .filter(|block| !block.partial && block.input < cursor)
        .collect()
}

/// The block's rows without the blank lines at its edges (the one separating prompts).
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

/// The command's output as text, without marks or trailing whitespace.
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

/// A row that follows its text as lines move into the scrollback. Once the scrollback is full
/// there's no way to tell how far it shifted, so it may drift.
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

    /// Mimics the scanner: the mark goes right after the prompt.
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
            "{}ls\r\none\r\ntwo\r\n\r\n{}false\r\n\r\n{}",
            prompt(1, false),
            prompt(2, false),
            prompt(2, true),
        );
        feed(&mut term, &text);
        let blocks = blocks_from(&term, term.topmost_line());
        assert_eq!(blocks.len(), 3);
        // `ls`: input on row 0, output on 1–2; row 3 is the blank line of the next prompt,
        // which spans two rows.
        assert_eq!(
            (blocks[0].input, blocks[0].output, blocks[0].end),
            (Line(0), Line(1), Line(2))
        );
        assert!(!blocks[0].failed);
        assert_eq!(output_text(&term, &blocks[0]), "one\ntwo");
        // `false` failed: the next prompt's mark says so.
        assert_eq!((blocks[1].start, blocks[1].input), (Line(3), Line(4)));
        assert!(blocks[1].failed);
        assert_eq!(output_text(&term, &blocks[1]), "");
        assert_eq!(trimmed_rows(&term, &blocks[1]), Some((Line(4), Line(4))));
        // The current prompt isn't a command.
        assert_eq!(commands(&term).len(), 2);
    }

    #[test]
    fn long_input_wraps_before_the_output() {
        let mut term = term(10, 8);
        feed(
            &mut term,
            &format!(
                "{}echo 123456789\r\nend\r\n{}",
                prompt(1, false),
                prompt(1, false)
            ),
        );
        let blocks = commands(&term);
        assert_eq!(blocks[0].output, Line(2));
        assert_eq!(output_text(&term, &blocks[0]), "end");
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
