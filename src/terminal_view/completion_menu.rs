//! The app's suggestions popup at the prompt (⌃Space, or Tab when set to): opening,
//! keeping it in sync with the line, accepting and drawing it.

use super::*;

const COMPLETION_ROWS: usize = 8;

pub(super) struct CompletionMenu {
    completion: completions::Completion,
    selected: usize,
    /// First visible row.
    top: usize,
    /// The line the candidates were computed from.
    line: EditLine,
}

impl CompletionMenu {
    pub(super) fn select(&mut self, delta: isize) {
        let len = self.completion.items.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + COMPLETION_ROWS {
            self.top = self.selected + 1 - COMPLETION_ROWS;
        }
    }
}

impl TerminalView {
    /// Tab: opens the popup. With a single candidate it completes directly; with none it returns
    /// `false` and the Tab goes to the shell (its usual completion).
    pub(super) fn open_completions(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(line) = self.terminal.read(cx).shell().line.clone() else {
            return false;
        };
        let Some(completion) = self.terminal.update(cx, |t, cx| t.complete(cx)) else {
            return false;
        };
        if completion.items.len() == 1 {
            let candidate = completion.items[0].clone();
            self.accept_completion(Some(candidate), false, cx);
            return true;
        }
        self.completion = Some(CompletionMenu {
            completion,
            selected: 0,
            top: 0,
            line,
        });
        cx.notify();
        true
    }

    /// Writes the candidate (the selected one if none is given) into the line; with `run`, runs it.
    pub(super) fn accept_completion(
        &mut self,
        candidate: Option<completions::Candidate>,
        run: bool,
        cx: &mut Context<Self>,
    ) {
        let menu = self.completion.take();
        let Some(candidate) =
            candidate.or_else(|| menu.and_then(|m| m.completion.items.get(m.selected).cloned()))
        else {
            return;
        };
        let Some(line) = self.terminal.read(cx).shell().line.clone() else {
            return;
        };
        let cursor = line
            .buffer
            .char_indices()
            .nth(line.cursor)
            .map_or(line.buffer.len(), |(i, _)| i);
        let (buffer, new_cursor) = completions::apply(&line.buffer, cursor, &candidate);
        let back = buffer.chars().count().saturating_sub(new_cursor);
        let app_cursor = self.terminal.read(cx).mode().contains(TermMode::APP_CURSOR);
        self.terminal.update(cx, |t, cx| {
            t.insert_command(&buffer, run, cx);
            if !run && back > 0 {
                let left = if app_cursor { "\x1bOD" } else { "\x1b[D" };
                t.input(left.repeat(back).into_bytes(), cx);
            }
        });
        // A folder continues: once it's written, the popup reopens with its contents.
        if !run && candidate.text.ends_with('/') {
            self.reopen_completion_on = Some(buffer);
        }
        cx.notify();
    }

    /// Keeps the popup in sync with the line the shell reports.
    pub(super) fn sync_completion(&mut self, cx: &mut Context<Self>) {
        let terminal = self.terminal.read(cx);
        let line = terminal
            .shell()
            .line
            .clone()
            .filter(|_| terminal.at_prompt());
        let Some(line) = line else {
            self.completion = None;
            return;
        };
        if self.reopen_completion_on.as_ref() == Some(&line.buffer) {
            self.reopen_completion_on = None;
            self.open_completions(cx);
            return;
        }
        let Some(menu) = &self.completion else {
            return;
        };
        if menu.line == line {
            return;
        }
        let previous = menu
            .completion
            .items
            .get(menu.selected)
            .map(|c| c.label.clone());
        match self.terminal.update(cx, |t, cx| t.complete(cx)) {
            Some(completion) => {
                let selected = previous
                    .and_then(|label| completion.items.iter().position(|c| c.label == label))
                    .unwrap_or(0);
                let mut menu = CompletionMenu {
                    completion,
                    selected: 0,
                    top: 0,
                    line,
                };
                menu.select(selected as isize);
                self.completion = Some(menu);
            }
            None => self.completion = None,
        }
    }

    /// The rest of the selected candidate, shown in gray on the line.
    pub(super) fn completion_ghost(&self) -> Option<String> {
        let menu = self.completion.as_ref()?;
        let candidate = menu.completion.items.get(menu.selected)?;
        let typed = &menu.completion.token;
        candidate
            .text
            .strip_prefix(typed.as_str())
            .filter(|rest| !rest.is_empty())
            .map(str::to_string)
    }

    pub(super) fn render_completion_menu(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let menu = self.completion.as_ref()?;
        let geometry = self.geometry.get()?;
        let ui = cx.tokens().ui.clone();
        let warning = cx.tokens().status.warning;

        // Anchor: the cell where the word being completed starts.
        let anchor_cell = {
            let term = self.terminal.read(cx).term.clone();
            let term = term.lock();
            let replace_from = menu
                .completion
                .items
                .get(menu.selected)
                .map_or(0, |c| c.replace_from);
            let char_ix = menu
                .line
                .buffer
                .get(..replace_from)
                .map_or(0, |s| s.chars().count());
            locate_input(&term, &menu.line)
                .and_then(|cells| cells.get(char_ix).copied())
                .unwrap_or(term.grid().cursor.point)
        };
        let x = geometry.origin.x + geometry.cell_width * anchor_cell.column.0 as f32 - px(10.);
        let row_top = geometry.origin.y
            + geometry.line_height * (anchor_cell.line.0 + geometry.display_offset) as f32;
        let above = row_top > window.viewport_size().height / 2.;
        let (position, corner) = if above {
            (point(x, row_top - px(6.)), gpui::Anchor::BottomLeft)
        } else {
            (
                point(x, row_top + geometry.line_height + px(6.)),
                gpui::Anchor::TopLeft,
            )
        };

        let items = &menu.completion.items;
        let selected = items.get(menu.selected);
        let rows = items
            .iter()
            .enumerate()
            .skip(menu.top)
            .take(COMPLETION_ROWS)
            .map(|(ix, item)| {
                let is_selected = ix == menu.selected;
                v_flex()
                    .child(
                        h_flex()
                            .id(("completion", ix))
                            .h(px(30.))
                            .px(px(10.))
                            .gap(px(10.))
                            .rounded(metrics::RADIUS_ROW)
                            .cursor_pointer()
                            .when(is_selected, |row| row.bg(ui.element_selected))
                            .when(!is_selected, |row| {
                                row.hover(|style| style.bg(ui.element_hover))
                            })
                            .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                if let Some(menu) = &mut this.completion {
                                    menu.selected = ix;
                                }
                                this.accept_completion(None, false, cx);
                            }))
                            .child(KindBadge::new(item.kind))
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(px(260.))
                                    .overflow_hidden()
                                    .font_family(MONO_FONT_FAMILY)
                                    .child(
                                        HighlightedLabel::new(
                                            item.label.clone(),
                                            item.positions.clone(),
                                        )
                                        .color(if is_selected {
                                            Color::Default
                                        } else {
                                            Color::Muted
                                        })
                                        .truncate(),
                                    ),
                            )
                            .child(div().flex_1())
                            .when_some(item.description.clone(), |row, description| {
                                row.child(
                                    div().min_w_0().child(
                                        Label::new(description)
                                            .size(LabelSize::Small)
                                            .color(if is_selected {
                                                Color::Muted
                                            } else {
                                                Color::Placeholder
                                            })
                                            .truncate(),
                                    ),
                                )
                            }),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let hint = |key: &'static str, label: &'static str| {
            h_flex().gap(px(5.)).child(Kbd::new(key)).child(
                Label::new(label)
                    .size(LabelSize::XSmall)
                    .color(Color::Placeholder),
            )
        };
        let list = v_flex()
            .w(px(440.))
            .child(v_flex().p(px(6.)).gap(px(1.)).children(rows))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(px(12.))
                    .px(px(14.))
                    .py(px(7.))
                    .border_t_1()
                    .border_color(ui.divider)
                    .child(hint("↑↓", "navigate"))
                    .child(hint("Tab", "complete"))
                    .child(hint("⏎", "run"))
                    .child(hint("esc", "close"))
                    .child(div().flex_1())
                    .child(
                        Label::new(match items.len() {
                            1 => "1 result".to_string(),
                            count => format!("{count} results"),
                        })
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder),
                    ),
            );

        let doc = selected.and_then(|c| c.doc.clone()).map(|doc| {
            v_flex()
                .w(px(300.))
                .gap(px(10.))
                .p(px(14.))
                .border_l_1()
                .border_color(ui.divider)
                .bg(ui.chrome)
                .child(
                    div()
                        .font_family(MONO_FONT_FAMILY)
                        .child(Label::new(doc.title).weight(gpui::FontWeight::SEMIBOLD)),
                )
                .when_some(doc.description, |el, d| {
                    el.child(Label::new(d).size(LabelSize::Small).color(Color::Muted))
                })
                .when_some(doc.usage, |el, usage| {
                    el.child(
                        div()
                            .px(px(10.))
                            .py(px(7.))
                            .rounded(metrics::RADIUS_ROW)
                            .bg(ui.background)
                            .border_1()
                            .border_color(ui.border)
                            .font_family(MONO_FONT_FAMILY)
                            .child(Label::new(usage).size(LabelSize::Small).color(Color::Muted)),
                    )
                })
                .children(doc.flags.into_iter().map(|(flag, description)| {
                    h_flex()
                        .gap(px(10.))
                        .child(
                            div()
                                .w(px(72.))
                                .flex_none()
                                .font_family(MONO_FONT_FAMILY)
                                .child(
                                    Label::new(flag)
                                        .size(LabelSize::Small)
                                        .color(Color::Custom(warning)),
                                ),
                        )
                        .child(
                            Label::new(description)
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .truncate(),
                        )
                }))
        });

        let popup = h_flex()
            .items_start()
            .bg(ui.surface)
            .border_1()
            .border_color(ui.border_strong)
            .rounded(px(10.))
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(list)
            .children(doc);

        Some(
            deferred(
                anchored()
                    .position(position)
                    .anchor(corner)
                    .snap_to_window_with_margin(px(8.))
                    .child(popup),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}
