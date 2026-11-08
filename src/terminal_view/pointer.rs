//! The mouse: mapping positions to cells, selection, links, the wheel, and reporting it to
//! programs that ask for it (vim, htop…).

use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct Press {
    point: AlacPoint,
    side: Side,
    /// A selection already exists (from a drag, double/triple click, or Shift+click).
    dragged: bool,
}

impl TerminalView {
    /// Grid cell under `position` and which side of it was hit. With `clamp`, a point
    /// off-screen is clamped to the edge (for dragging a selection past it).
    fn grid_point_side(
        &self,
        position: Point<Pixels>,
        term: &Term<Listener>,
        clamp: bool,
    ) -> Option<(AlacPoint, Side)> {
        let geometry = self.geometry.get()?;
        let mut column = (position.x - geometry.origin.x) / geometry.cell_width;
        let mut row = (position.y - geometry.origin.y) / geometry.line_height;
        let (columns, lines) = (term.columns() as f32, term.screen_lines() as f32);
        if clamp {
            column = column.clamp(0., columns - 0.01);
            row = row.clamp(0., lines - 0.01);
        } else if column < 0. || row < 0. || column >= columns || row >= lines {
            return None;
        }
        let side = if column.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        // The terminal's own offset, not the last frame's: clearing the scrollback since then
        // (`clear`, ⌘K) would leave that one pointing at rows that no longer exist.
        let display_offset = term.grid().display_offset() as i32;
        Some((
            AlacPoint::new(
                alacritty_terminal::index::Line(row as i32 - display_offset),
                alacritty_terminal::index::Column(column as usize),
            ),
            side,
        ))
    }

    fn grid_point(&self, position: Point<Pixels>, term: &Term<Listener>) -> Option<AlacPoint> {
        self.grid_point_side(position, term, false)
            .map(|(point, _)| point)
    }

    /// Link under the mouse. The line being typed has no links: clicking there
    /// moves the cursor.
    fn link_under(&self, position: Point<Pixels>, cx: &App) -> Option<HoveredLink> {
        let terminal = self.terminal.read(cx);
        let term = terminal.term.clone();
        let term = term.lock();
        let point = self.grid_point(position, &term)?;
        if let Some(edit) = terminal
            .shell()
            .line
            .as_ref()
            .filter(|_| terminal.at_prompt())
            && locate_input(&term, edit).is_some_and(|cells| cells.contains(&point))
        {
            return None;
        }
        // Over SSH the paths belong to the server: there's no local file to open.
        let cwd = terminal
            .shell()
            .cwd
            .as_deref()
            .filter(|_| terminal.shell().remote.is_none());
        link_at(&term, point, cwd)
    }

    /// The program wants the mouse (vim, htop…) and local selection wasn't forced with Shift.
    fn reports_mouse(&self, shift: bool, cx: &App) -> bool {
        let term = self.terminal.read(cx).term.clone();
        let term = term.lock();
        !shift && mouse::wants_mouse(*term.mode()) && term.grid().display_offset() == 0
    }

    /// Sends the event to the program if its mouse mode asks for it.
    fn send_mouse(
        &mut self,
        button: Option<mouse::Button>,
        action: mouse::Action,
        position: Point<Pixels>,
        modifiers: &Modifiers,
        cx: &mut Context<Self>,
    ) -> bool {
        let bytes = {
            let term = self.terminal.read(cx).term.clone();
            let term = term.lock();
            let Some((point, _)) = self.grid_point_side(position, &term, true) else {
                return false;
            };
            let mods = mouse::Mods {
                shift: modifiers.shift,
                alt: modifiers.alt,
                control: modifiers.control,
            };
            let line = (point.line.0 + term.grid().display_offset() as i32).max(0) as usize;
            if action == mouse::Action::Motion
                && self.last_motion_cell == Some((point.column.0, line))
            {
                return true;
            }
            self.last_motion_cell = Some((point.column.0, line));
            mouse::report(button, action, point.column.0, line, mods, *term.mode())
        };
        match bytes {
            Some(bytes) => {
                self.terminal.update(cx, |t, cx| t.input(bytes, cx));
                true
            }
            None => false,
        }
    }

    pub(super) fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window, cx);
        if self.agent_panel.take().is_some() {
            cx.notify();
        }
        let shift = event.modifiers.shift;
        if self.reports_mouse(shift, cx) {
            self.reported_press = self.send_mouse(
                Some(mouse::Button::Left),
                mouse::Action::Press,
                event.position,
                &event.modifiers,
                cx,
            );
            return;
        }
        // A click that only activates the window doesn't open links.
        if !event.first_mouse
            && !shift
            && event.click_count == 1
            && let Some(link) = self.link_under(event.position, cx)
        {
            self.link_press = Some(link.target);
            return;
        }

        let term = self.terminal.read(cx).term.clone();
        let mut term = term.lock();
        let Some((point, side)) = self.grid_point_side(event.position, &term, false) else {
            return;
        };
        let selection_type = match event.click_count {
            2 => Some(SelectionType::Semantic),
            3.. => Some(SelectionType::Lines),
            _ => None,
        };
        let mut dragged = true;
        match selection_type {
            // Double click: word; triple: line. Dragging afterwards extends by words/lines.
            Some(ty) => term.selection = Some(Selection::new(ty, point, side)),
            // Shift+click extends the existing selection.
            None if shift && term.selection.is_some() => {
                if let Some(selection) = term.selection.as_mut() {
                    selection.update(point, side);
                }
            }
            None => {
                term.selection = None;
                dragged = false;
            }
        }
        drop(term);
        self.press = Some(Press {
            point,
            side,
            dragged,
        });
        cx.notify();
    }

    pub(super) fn mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.pressed_button == Some(MouseButton::Left) {
            if self.reported_press {
                self.send_mouse(
                    Some(mouse::Button::Left),
                    mouse::Action::Motion,
                    event.position,
                    &event.modifiers,
                    cx,
                );
                return;
            }
            if let Some(mut press) = self.press {
                let term = self.terminal.read(cx).term.clone();
                let mut term = term.lock();
                let Some((point, side)) = self.grid_point_side(event.position, &term, true) else {
                    return;
                };
                if !press.dragged {
                    if point == press.point && side == press.side {
                        return;
                    }
                    term.selection = Some(Selection::new(
                        SelectionType::Simple,
                        press.point,
                        press.side,
                    ));
                    press.dragged = true;
                }
                if let Some(selection) = term.selection.as_mut() {
                    selection.update(point, side);
                }
                drop(term);
                self.press = Some(press);
                cx.notify();
                return;
            }
        }
        if event.pressed_button.is_none() && self.reports_mouse(event.modifiers.shift, cx) {
            self.send_mouse(
                None,
                mouse::Action::Motion,
                event.position,
                &event.modifiers,
                cx,
            );
        }
        // Moving within a cell changes nothing, and looking for a link isn't free.
        let point = {
            let term = self.terminal.read(cx).term.clone();
            let term = term.lock();
            self.grid_point(event.position, &term)
        };
        if point == self.hover_point {
            return;
        }
        self.hover_point = point;
        let link = self.link_under(event.position, cx);
        if link != self.hovered_link {
            self.hovered_link = link;
            cx.notify();
        }
    }

    pub(super) fn mouse_up(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if std::mem::take(&mut self.reported_press) {
            self.send_mouse(
                Some(mouse::Button::Left),
                mouse::Action::Release,
                event.position,
                &event.modifiers,
                cx,
            );
            return;
        }
        if let Some(target) = self.link_press.take() {
            if self
                .link_under(event.position, cx)
                .is_some_and(|link| link.target == target)
            {
                match target {
                    LinkTarget::Url(url) => cx.open_url(&url),
                    LinkTarget::File(file) => {
                        window.dispatch_action(Box::new(crate::workspace::OpenFile { file }), cx)
                    }
                    LinkTarget::Reveal(path) => cx.reveal_path(&path),
                }
            }
            return;
        }
        match self.press.take() {
            Some(press) if press.dragged => {
                let term = self.terminal.read(cx).term.clone();
                let mut term = term.lock();
                if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
                    term.selection = None;
                }
            }
            Some(_) => self.move_edit_cursor(event.position, cx),
            None => {}
        }
    }

    /// Right or middle click: only relevant to programs that want the mouse.
    pub(super) fn other_button(
        &mut self,
        button: mouse::Button,
        action: mouse::Action,
        position: Point<Pixels>,
        modifiers: &Modifiers,
        cx: &mut Context<Self>,
    ) {
        if self.reports_mouse(modifiers.shift, cx) {
            self.send_mouse(Some(button), action, position, modifiers, cx);
        }
    }

    /// Click on the line being typed: moves zsh's cursor there, like in an
    /// editor, by sending the needed arrow keys.
    fn move_edit_cursor(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(geometry) = self.geometry.get() else {
            return;
        };
        let terminal = self.terminal.read(cx);
        let Some(edit) = terminal
            .shell()
            .line
            .clone()
            .filter(|_| terminal.at_prompt())
        else {
            return;
        };
        let column = (position.x - geometry.origin.x) / geometry.cell_width;
        let row = ((position.y - geometry.origin.y) / geometry.line_height).floor();
        if column < 0. || row < 0. {
            return;
        }
        let grid_line = row as i32 - geometry.display_offset;

        let term = terminal.term.clone();
        let term = term.lock();
        let Some(points) = locate_input(&term, &edit) else {
            return;
        };
        let cells: Vec<CharCell> = points
            .iter()
            .map(|p| CharCell {
                line: p.line.0,
                column: p.column.0,
                width: if term.grid()[*p].flags.contains(Flags::WIDE_CHAR) {
                    2
                } else {
                    1
                },
            })
            .collect();
        let app_cursor = term.mode().contains(TermMode::APP_CURSOR);
        drop(term);

        let Some(target) = click_target(&cells, grid_line, column) else {
            return;
        };
        let delta = target as isize - edit.cursor.min(cells.len()) as isize;
        if delta == 0 {
            return;
        }
        let arrow = match (delta > 0, app_cursor) {
            (true, true) => "\x1bOC",
            (true, false) => "\x1b[C",
            (false, true) => "\x1bOD",
            (false, false) => "\x1b[D",
        };
        let bytes = arrow.repeat(delta.unsigned_abs()).into_bytes();
        self.terminal.update(cx, |t, cx| t.input(bytes, cx));
    }

    pub(super) fn scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = match event.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => {
                let line_height = self
                    .geometry
                    .get()
                    .map_or(TERMINAL_LINE_HEIGHT, |g| g.line_height);
                p.y / line_height
            }
        };
        self.scroll_remainder += delta;
        let lines = self.scroll_remainder.trunc();
        self.scroll_remainder -= lines;
        if lines == 0. {
            return;
        }

        // If the program wants the mouse, the wheel is sent as wheel events.
        if self.reports_mouse(event.modifiers.shift, cx) {
            let button = if lines > 0. {
                mouse::Button::WheelUp
            } else {
                mouse::Button::WheelDown
            };
            for _ in 0..lines.abs() as usize {
                self.send_mouse(
                    Some(button),
                    mouse::Action::Press,
                    event.position,
                    &event.modifiers,
                    cx,
                );
            }
            return;
        }

        let mode = self.terminal.read(cx).mode();
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            // In full-screen programs (less, vim) the wheel is translated to arrow keys.
            let app = mode.contains(TermMode::APP_CURSOR);
            let key = match (lines > 0., app) {
                (true, true) => "\x1bOA",
                (true, false) => "\x1b[A",
                (false, true) => "\x1bOB",
                (false, false) => "\x1b[B",
            };
            let bytes = key.repeat(lines.abs() as usize).into_bytes();
            self.terminal.update(cx, |t, cx| t.input(bytes, cx));
        } else {
            self.terminal.update(cx, |t, cx| t.scroll(lines as i32, cx));
        }
    }
}
