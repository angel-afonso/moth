//! What's drawn over the grid: the production paste guard, the exit banner, the fix hint
//! and the agent panel; and agent mode's panel, beside the grid.

use super::*;

impl TerminalView {
    pub(super) fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let production = self
            .terminal
            .read(cx)
            .shell()
            .remote
            .as_ref()
            .is_some_and(|remote| remote.identity.production);
        // Multiple lines in production would run one after another: ask for confirmation. A
        // lone `\r` is a line break too (`paste` sends every break as one).
        if production && text.trim_end().contains(['\n', '\r']) {
            self.pending_paste = Some(text);
            cx.notify();
            return;
        }
        self.terminal.update(cx, |t, cx| t.paste(&text, cx));
    }

    pub(super) fn confirm_paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.pending_paste.take() {
            self.terminal.update(cx, |t, cx| t.paste(&text, cx));
            cx.notify();
        }
    }

    pub(super) fn cancel_paste(&mut self, cx: &mut Context<Self>) {
        self.pending_paste = None;
        cx.notify();
    }

    pub(super) fn open_agent(
        &mut self,
        task: AgentTask,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.completion = None;
        let terminal = self.terminal.clone();
        let panel = cx.new(|cx| AgentPanel::new(task, terminal, window, cx));
        let subscription = cx.subscribe_in(&panel, window, |this, _, event, window, cx| {
            if let AgentPanelEvent::Insert { command, run } = event {
                this.terminal
                    .update(cx, |t, cx| t.insert_command(command, *run, cx));
            }
            this.agent_panel = None;
            this.focus_handle.focus(window, cx);
            cx.notify();
        });
        self.agent_panel = Some((panel, subscription));
        cx.notify();
    }

    /// ⌘⇧A: opens agent mode beside the terminal, or moves the keyboard to it when it's open.
    pub(super) fn toggle_agent_mode(
        &mut self,
        _: &ToggleAgentMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((panel, _)) = &self.agent_mode {
            panel.read(cx).focus_handle(cx).focus(window, cx);
            return;
        }
        self.completion = None;
        let terminal = self.terminal.clone();
        let panel = cx.new(|cx| AgentModePanel::new(terminal, window, cx));
        let subscription = cx.subscribe_in(&panel, window, |this, _, event, window, cx| {
            if let AgentModeEvent::Close = event {
                this.agent_mode = None;
            }
            this.focus_handle.focus(window, cx);
            cx.notify();
        });
        self.agent_mode = Some((panel, subscription));
        cx.notify();
    }

    /// Where overlays at the prompt go: the row of the cursor, and whether there's more room
    /// above it than below.
    fn prompt_anchor(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<(Pixels, Pixels, bool, GridGeometry)> {
        let geometry = self.geometry.get()?;
        let cursor = self.terminal.read(cx).term.lock().grid().cursor.point;
        let row_top = geometry.origin.y
            + geometry.line_height * (cursor.line.0 + geometry.display_offset) as f32;
        let x = geometry.origin.x + geometry.cell_width * cursor.column.0 as f32;
        let above = row_top > window.viewport_size().height / 2.;
        Some((x, row_top, above, geometry))
    }

    pub(super) fn render_agent_panel(&self, window: &Window, cx: &App) -> Option<gpui::AnyElement> {
        let (panel, _) = self.agent_panel.as_ref()?;
        let (_, row_top, above, geometry) = self.prompt_anchor(window, cx)?;
        let x = geometry.origin.x - px(10.);
        let (position, corner) = if above {
            (point(x, row_top - px(6.)), gpui::Anchor::BottomLeft)
        } else {
            (
                point(x, row_top + geometry.line_height + px(6.)),
                gpui::Anchor::TopLeft,
            )
        };
        Some(
            deferred(
                anchored()
                    .position(position)
                    .anchor(corner)
                    .snap_to_window_with_margin(px(8.))
                    .child(panel.clone()),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    /// A note after the gray fix (→ accepts it, and why it's suggested), or, with nothing
    /// typed after a failure and no fix found here, the shortcut that asks the agent.
    pub(super) fn render_fix_hint(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if self.completion.is_some() || self.agent_panel.is_some() || self.search.is_some() {
            return None;
        }
        let terminal = self.terminal.read(cx);
        let failure = terminal.shell().failure.as_ref()?;
        if !terminal.at_prompt() {
            return None;
        }
        let line = terminal.shell().line.as_ref()?;
        let (key, label, ghost, ai) = match terminal.fix_suggestion() {
            Some(rest) => {
                let reason = failure
                    .fix
                    .as_ref()
                    .map_or(String::new(), |f| f.reason.clone());
                (
                    "→".to_string(),
                    format!("fix · {reason}"),
                    rest.chars().count(),
                    false,
                )
            }
            None if line.buffer.is_empty() && failure.failure.worth_fixing() => {
                let search_path = terminal
                    .shell()
                    .path_env
                    .clone()
                    .or_else(|| std::env::var("PATH").ok())
                    .unwrap_or_default();
                let agent::Resolution::Found(agent) = agent::find(&search_path, cx) else {
                    return None;
                };
                (
                    shortcuts::label("cmd-shift-i"),
                    format!("fix with {}", agent.name),
                    0,
                    true,
                )
            }
            None => return None,
        };
        let (x, row_top, _, geometry) = self.prompt_anchor(window, cx)?;
        let ui = cx.tokens().ui.clone();
        let x = x + geometry.cell_width * (ghost + 2) as f32;
        let y = row_top + (geometry.line_height - px(18.)) / 2.;
        Some(
            deferred(
                anchored()
                    .position(point(x, y))
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        h_flex()
                            .h(px(18.))
                            .gap(px(5.))
                            .px(px(5.))
                            .rounded(px(5.))
                            .bg(ui.surface)
                            .border_1()
                            .border_color(ui.border)
                            .when(ai, |el| el.child(crate::agent_panel::ai_badge(ui.accent)))
                            .child(Kbd::new(key))
                            .child(
                                Label::new(label)
                                    .size(LabelSize::XSmall)
                                    .color(Color::Placeholder),
                            ),
                    ),
            )
            .into_any_element(),
        )
    }

    pub(super) fn render_paste_guard(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let text = self.pending_paste.as_ref()?;
        let terminal = self.terminal.read(cx);
        let remote = terminal.shell().remote.as_ref()?;
        let host_color = remote.identity.color(cx);
        let host = remote.target.alias.clone();
        let lines = text.trim_end().lines().count();
        let preview: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let ui = cx.tokens().ui.clone();

        Some(
            h_flex()
                .absolute()
                .left(metrics::SPACE_4)
                .right(metrics::SPACE_4)
                .bottom(px(12.))
                .gap(px(14.))
                .py(px(10.))
                .pl(px(14.))
                .pr(px(12.))
                .rounded(px(10.))
                .bg(ui.surface)
                .border_1()
                .border_color(tint(host_color, 0.45))
                .shadow_lg()
                .child(
                    Icon::new(IconName::Lock)
                        .size(IconSize::Small)
                        .color(Color::Custom(host_color)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(
                            h_flex()
                                .gap_1()
                                .child(Label::new(format!("Paste {lines} lines into")))
                                .child(
                                    h_flex()
                                        .child(
                                            Label::new(host)
                                                .color(Color::Custom(host_color))
                                                .weight(FontWeight::SEMIBOLD),
                                        )
                                        .child(Label::new("?")),
                                ),
                        )
                        .child(
                            div()
                                .font_family(MONO_FONT_FAMILY)
                                .text_size(px(12.))
                                .text_color(ui.text_placeholder)
                                .child(
                                    Label::new(preview)
                                        .size(LabelSize::Small)
                                        .color(Color::Placeholder)
                                        .truncate(),
                                ),
                        ),
                )
                .child(
                    Button::new("paste-cancel", "Cancel")
                        .style(ButtonStyle::Outlined)
                        .size(ButtonSize::Large)
                        .label_size(LabelSize::Small)
                        .key_binding(ui::KeyBinding::for_action(&menu::Cancel, cx))
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_paste(cx))),
                )
                .child(
                    Button::new("paste-confirm", "Paste Anyway")
                        .style(ButtonStyle::Tinted(TintColor::Error))
                        .size(ButtonSize::Large)
                        .label_size(LabelSize::Small)
                        .on_click(cx.listener(|this, _, _, cx| this.confirm_paste(cx))),
                )
                .into_any_element(),
        )
    }

    /// Notice that the shell exited with an error; the output stays above to show why.
    /// The tab's folder is gone (its disk was unplugged, say). Nothing to do but wait: the
    /// shell goes back into it when it's back.
    pub(super) fn render_missing_cwd_banner(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let terminal = self.terminal.read(cx);
        if terminal.has_exited() {
            return None;
        }
        let shell = terminal.shell();
        let missing = shell.cwd_missing.as_ref()?;
        let folder = shell
            .cwd
            .as_ref()
            .and_then(|cwd| cwd.file_name())
            .map_or("this folder".to_string(), |name| {
                name.to_string_lossy().into_owned()
            });
        let recovers = terminal.recovers_cwd();
        let (title, detail) = match (&missing.volume, recovers) {
            (Some(volume), true) => (
                format!("{volume} was disconnected"),
                format!("This tab will go back to {folder} when the volume is connected again."),
            ),
            (Some(volume), false) => (
                format!("{volume} was disconnected"),
                format!("Once it's connected again, cd back into {folder}."),
            ),
            (None, true) => (
                format!("{folder} is no longer available"),
                "This tab will go back to it if it comes back.".to_string(),
            ),
            (None, false) => (
                format!("{folder} is no longer available"),
                "If it comes back, cd back into it.".to_string(),
            ),
        };
        let ui = cx.tokens().ui.clone();
        let warning = cx.tokens().terminal.ansi.yellow;
        // A card in the top right corner: a new tab's prompt is at the top left, and the
        // latest output at the bottom.
        Some(
            h_flex()
                .absolute()
                .right(metrics::SPACE_4)
                .top(px(12.))
                .max_w(px(440.))
                .gap(px(14.))
                .py(px(10.))
                .pl(px(14.))
                .pr(px(12.))
                .rounded(px(10.))
                .bg(ui.surface)
                .border_1()
                .border_color(tint(warning, 0.45))
                .shadow_lg()
                .child(
                    Icon::new(IconName::Warning)
                        .size(IconSize::Small)
                        .color(Color::Custom(warning)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(Label::new(title))
                        .child(
                            Label::new(detail)
                                .size(LabelSize::Small)
                                .color(Color::Placeholder),
                        ),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_exit_banner(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let message = self.terminal.read(cx).exit_failure()?;
        let ui = cx.tokens().ui.clone();
        let error = cx.tokens().terminal.ansi.red;
        Some(
            h_flex()
                .absolute()
                .left(metrics::SPACE_4)
                .right(metrics::SPACE_4)
                .bottom(px(12.))
                .gap(px(14.))
                .py(px(10.))
                .pl(px(14.))
                .pr(px(12.))
                .rounded(px(10.))
                .bg(ui.surface)
                .border_1()
                .border_color(tint(error, 0.45))
                .shadow_lg()
                .child(
                    Icon::new(IconName::Warning)
                        .size(IconSize::Small)
                        .color(Color::Custom(error)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(Label::new(message))
                        .child(
                            Label::new("The output above may explain why.")
                                .size(LabelSize::Small)
                                .color(Color::Placeholder),
                        ),
                )
                .child(
                    Button::new("exited-close", "Close")
                        .style(ButtonStyle::Outlined)
                        .size(ButtonSize::Large)
                        .label_size(LabelSize::Small)
                        .key_binding(ui::KeyBinding::for_action(&crate::workspace::CloseTab, cx))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(crate::workspace::CloseTab), cx)
                        }),
                )
                .child(
                    Button::new("exited-restart", "Open New Shell ⏎")
                        .style(ButtonStyle::Filled)
                        .size(ButtonSize::Large)
                        .label_size(LabelSize::Small)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(RestartShell), cx)
                        }),
                )
                .into_any_element(),
        )
    }
}
