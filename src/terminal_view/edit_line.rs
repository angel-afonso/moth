//! Where the line being edited sits in the grid, for clicks and highlighting.

use super::*;

/// Cell of a character on the line being edited: grid line, starting column, and width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CharCell {
    pub(super) line: i32,
    pub(super) column: usize,
    pub(super) width: usize,
}

/// Where the cursor should land (as a character index) when clicking at `line`, `column`
/// (fractional column). Like in an editor: the right half of a character puts the cursor
/// after it, and a click past the end of a row goes to the end of that row.
/// `None` if the click doesn't land on the line being edited (e.g. on the prompt).
pub(super) fn click_target(cells: &[CharCell], line: i32, column: f32) -> Option<usize> {
    let row: Vec<(usize, &CharCell)> = cells
        .iter()
        .enumerate()
        .filter(|(_, c)| c.line == line)
        .collect();
    let (first_ix, first) = *row.first()?;
    if column < first.column as f32 {
        // Left of the text: on a continuation row, the start of that row; on the
        // first row it's the prompt, which is left alone.
        return (first_ix > 0).then_some(first_ix);
    }
    for (ix, cell) in &row {
        let start = cell.column as f32;
        let end = start + cell.width as f32;
        if column >= start && column < end {
            let right_half = column - start >= cell.width as f32 / 2.;
            return Some(if right_half { ix + 1 } else { *ix });
        }
    }
    let (last_ix, _) = *row.last()?;
    Some(last_ix + 1)
}

/// Grid position of each character on the line being edited, walking back from the
/// cursor. `None` if the cells don't match the text the shell reported.
pub(super) fn locate_input(term: &Term<Listener>, line: &EditLine) -> Option<Vec<AlacPoint>> {
    let grid = term.grid();
    let chars: Vec<char> = line.buffer.chars().collect();
    let cursor = line.cursor.min(chars.len());
    let mut points = vec![AlacPoint::default(); chars.len()];

    let step_back = |point: AlacPoint| -> Option<AlacPoint> {
        let mut prev = point.sub(grid, Boundary::Grid, 1);
        if prev == point {
            return None;
        }
        if grid[prev]
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            let before = prev.sub(grid, Boundary::Grid, 1);
            if before == prev {
                return None;
            }
            prev = before;
        }
        Some(prev)
    };

    let mut point = grid.cursor.point;
    for i in (0..cursor).rev() {
        point = step_back(point)?;
        if grid[point].c != chars[i] {
            return None;
        }
        points[i] = point;
    }

    let mut point = grid.cursor.point;
    for i in cursor..chars.len() {
        if i > cursor {
            let width = if grid[point].flags.contains(Flags::WIDE_CHAR) {
                2
            } else {
                1
            };
            let next = point.add(grid, Boundary::Grid, width);
            if next == point {
                return None;
            }
            point = next;
            if grid[point].flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
                point = point.add(grid, Boundary::Grid, 1);
            }
        }
        if point.column.0 >= grid.columns() || grid[point].c != chars[i] {
            return None;
        }
        points[i] = point;
    }
    Some(points)
}

pub(super) fn token_cells(
    buffer: &str,
    tokens: &[Token],
    cells: &[AlacPoint],
) -> HashMap<(i32, usize), TokenKind> {
    let mut map = HashMap::new();
    for (char_ix, (byte_ix, _)) in buffer.char_indices().enumerate() {
        if let Some(token) = tokens.iter().find(|t| t.range.contains(&byte_ix))
            && let Some(&point) = cells.get(char_ix)
        {
            map.insert((point.line.0, point.column.0), token.kind);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(line: i32, start: usize, widths: &[usize]) -> Vec<CharCell> {
        let mut column = start;
        widths
            .iter()
            .map(|&width| {
                let cell = CharCell {
                    line,
                    column,
                    width,
                };
                column += width;
                cell
            })
            .collect()
    }

    #[test]
    fn clicks_land_on_nearest_boundary() {
        // `git push` starting at column 2 (after "❯ ").
        let line = cells(5, 2, &[1; 8]);
        assert_eq!(click_target(&line, 5, 2.2), Some(0));
        assert_eq!(click_target(&line, 5, 2.7), Some(1));
        assert_eq!(click_target(&line, 5, 6.1), Some(4));
        // Past the end goes to the end.
        assert_eq!(click_target(&line, 5, 40.), Some(8));
        // On the prompt or another row: nothing.
        assert_eq!(click_target(&line, 5, 0.5), None);
        assert_eq!(click_target(&line, 4, 3.), None);
    }

    #[test]
    fn handles_wide_chars_and_wrapped_rows() {
        // "a界b": the wide character takes two cells.
        let line = cells(0, 2, &[1, 2, 1]);
        assert_eq!(click_target(&line, 0, 3.4), Some(1));
        assert_eq!(click_target(&line, 0, 4.2), Some(2));
        assert_eq!(click_target(&line, 0, 5.1), Some(2));
        assert_eq!(click_target(&line, 0, 5.7), Some(3));

        // Line wrapped across two rows: 4 characters on row 3 and 2 on row 4.
        let mut wrapped = cells(3, 76, &[1; 4]);
        wrapped.extend(cells(4, 0, &[1; 2]));
        assert_eq!(click_target(&wrapped, 4, 0.2), Some(4));
        assert_eq!(click_target(&wrapped, 4, 10.), Some(6));
    }

    #[test]
    fn empty_line_has_no_target() {
        assert_eq!(click_target(&[], 0, 3.), None);
    }
}
