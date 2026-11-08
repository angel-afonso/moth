//! ⌘F: a literal search through the output and scrollback.

use super::*;

pub(super) struct OutputSearch {
    pub(super) input: Entity<crate::text_input::TextInput>,
    pub(super) current: Option<SearchMatch>,
    pub(super) message: String,
    _changed: Subscription,
    /// The search for what's typed, after [`SEARCH_DELAY`]; replacing it cancels the last one.
    _pending: Option<gpui::Task<()>>,
}

/// How long ⌘F waits after a keystroke before searching.
const SEARCH_DELAY: std::time::Duration = std::time::Duration::from_millis(120);

/// A ⌘F match, along with the scrollback size at the time it was found.
#[derive(Clone)]
pub(super) struct SearchMatch {
    range: std::ops::RangeInclusive<AlacPoint>,
    history_size: usize,
}

impl SearchMatch {
    fn new(range: std::ops::RangeInclusive<AlacPoint>, term: &Term<Listener>) -> Self {
        Self {
            range,
            history_size: term.grid().history_size(),
        }
    }

    /// Where it is now: every line that enters the scrollback shifts the grid up one row.
    /// If it fell out of the scrollback or the screen was cleared, it's gone.
    pub(super) fn resolve(
        &self,
        term: &Term<Listener>,
    ) -> Option<std::ops::RangeInclusive<AlacPoint>> {
        let shift = term.grid().history_size().checked_sub(self.history_size)? as i32;
        let moved = |point: &AlacPoint| AlacPoint::new(point.line - shift, point.column);
        let (start, end) = (moved(self.range.start()), moved(self.range.end()));
        (start.line >= term.topmost_line()).then_some(start..=end)
    }
}

fn literal_pattern(query: &str) -> String {
    let mut pattern = String::new();
    for c in query.chars() {
        if "\\.^$|?*+()[]{}".contains(c) {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern
}

impl TerminalView {
    pub(super) fn find_output(
        &mut self,
        _: &FindOutput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search.is_none() {
            self.completion = None;
            let input = cx.new(|cx| crate::text_input::TextInput::new("Find in output…", cx));
            // A search can go through the whole scrollback, so it waits for a pause in typing.
            let changed = cx.subscribe(&input, |this, _, _: &crate::text_input::Changed, cx| {
                let task = cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(SEARCH_DELAY).await;
                    this.update(cx, |this, cx| this.find_match(false, true, cx))
                        .ok();
                });
                if let Some(search) = &mut this.search {
                    search._pending = Some(task);
                }
            });
            self.search = Some(OutputSearch {
                input,
                current: None,
                message: String::new(),
                _changed: changed,
                _pending: None,
            });
        }
        self.search
            .as_ref()
            .unwrap()
            .input
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    pub(super) fn find_match(&mut self, backwards: bool, reset: bool, cx: &mut Context<Self>) {
        let Some(search) = &mut self.search else {
            return;
        };
        let query = search.input.read(cx).text().to_string();
        if query.is_empty() {
            search.current = None;
            search.message.clear();
            cx.notify();
            return;
        }
        let Ok(mut regex) = RegexSearch::new(&literal_pattern(&query)) else {
            search.current = None;
            search.message = "Search too long".into();
            cx.notify();
            return;
        };
        let term = self.terminal.read(cx).term.clone();
        let mut term = term.lock();
        let direction = if backwards {
            Direction::Left
        } else {
            Direction::Right
        };
        let origin = if !reset {
            search
                .current
                .as_ref()
                .and_then(|found| found.resolve(&term))
                .map(|found| {
                    if backwards {
                        found.start().sub(&*term, Boundary::None, 1)
                    } else {
                        found.end().add(&*term, Boundary::None, 1)
                    }
                })
        } else {
            None
        }
        .unwrap_or_else(|| {
            AlacPoint::new(term.topmost_line(), alacritty_terminal::index::Column(0))
        });
        let found = term.search_next(&mut regex, origin, direction, Side::Left, None);
        search.current = found.map(|range| SearchMatch::new(range, &term));
        search.message = if let Some(found) = &search.current {
            term.scroll_to_point(*found.range.start());
            "⏎ next · ⇧⏎ previous · esc close".into()
        } else {
            "No matches".into()
        };
        cx.notify();
    }

    pub(super) fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search = None;
        self.focus_handle.focus(window, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_search_is_literal_and_finds_unicode_across_wrapped_rows() {
        use alacritty_terminal::{
            grid::Dimensions,
            index::{Column, Direction, Point, Side},
            term::search::RegexSearch,
        };
        let term = search_term("prefix ñ.[x] suffix", 10, 4);
        let mut regex = RegexSearch::new(&super::literal_pattern("ñ.[x]")).unwrap();
        let found = term
            .search_next(
                &mut regex,
                Point::new(term.topmost_line(), Column(0)),
                Direction::Right,
                Side::Left,
                None,
            )
            .unwrap();
        assert_eq!(found.start().column, Column(7));
        assert!(found.end().line > found.start().line);
        let mut regex = RegexSearch::new(&super::literal_pattern("ñ.[y]")).unwrap();
        assert!(
            term.search_next(
                &mut regex,
                Point::new(term.topmost_line(), Column(0)),
                Direction::Right,
                Side::Left,
                None
            )
            .is_none()
        );
    }

    #[test]
    fn output_search_finds_scrollback_and_wraps_both_directions() {
        use alacritty_terminal::{
            grid::Dimensions,
            index::{Boundary, Column, Direction, Point, Side},
            term::search::RegexSearch,
        };
        let term = search_term("match first\r\nother\r\nother\r\nmatch last", 20, 2);
        let mut regex = RegexSearch::new("match").unwrap();
        let first = term
            .search_next(
                &mut regex,
                Point::new(term.topmost_line(), Column(0)),
                Direction::Right,
                Side::Left,
                None,
            )
            .unwrap();
        assert!(first.start().line.0 < 0);
        let second = term
            .search_next(
                &mut regex,
                first.end().add(&term, Boundary::None, 1),
                Direction::Right,
                Side::Left,
                None,
            )
            .unwrap();
        assert!(second.start() > first.start());
        let wrapped = term
            .search_next(
                &mut regex,
                second.end().add(&term, Boundary::None, 1),
                Direction::Right,
                Side::Left,
                None,
            )
            .unwrap();
        assert_eq!(wrapped, first);
        let previous = term
            .search_next(
                &mut regex,
                first.start().sub(&term, Boundary::None, 1),
                Direction::Left,
                Side::Left,
                None,
            )
            .unwrap();
        assert_eq!(previous, second);
    }
}
