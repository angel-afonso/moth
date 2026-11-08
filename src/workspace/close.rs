//! Closing panes, tabs, windows and the app: what would be killed, and the confirmation.

use super::*;

/// What's running in every window (commands, SSH sessions), which a restart would end.
pub fn busy_everywhere(cx: &App) -> Vec<String> {
    let Some(registry) = cx.try_global::<Workspaces>() else {
        return Vec::new();
    };
    registry
        .0
        .iter()
        .filter_map(WeakEntity::upgrade)
        .flat_map(|workspace| workspace.read(cx).busy(CloseTarget::Window, cx))
        .collect()
}

/// What is being closed; if processes are running, confirmation is asked first.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum CloseTarget {
    Tab(usize),
    Pane(EntityId),
    Window,
    Quit,
}

impl Workspace {
    /// What closing the pane would kill: the SSH session or the running command.
    fn busy_with(pane: &Pane, cx: &App) -> Option<String> {
        let terminal = pane.view.read(cx).terminal().read(cx);
        if terminal.has_exited() {
            return None;
        }
        let shell = terminal.shell();
        if let Some(remote) = &shell.remote {
            return Some(format!("ssh {}", remote.user_at_host()));
        }
        shell
            .running
            .as_ref()
            .map(|running| short_command(&running.text))
    }

    fn busy(&self, target: CloseTarget, cx: &App) -> Vec<String> {
        self.tabs
            .iter()
            .filter(|tab| match target {
                CloseTarget::Tab(id) => tab.id == id,
                CloseTarget::Pane(id) => tab.pane(id).is_some(),
                CloseTarget::Window | CloseTarget::Quit => true,
            })
            .flat_map(|tab| &tab.panes)
            .filter(|pane| match target {
                CloseTarget::Pane(id) => pane.view.entity_id() == id,
                _ => true,
            })
            .filter_map(|pane| Self::busy_with(pane, cx))
            .collect::<Vec<_>>()
            .into_iter()
            .chain(self.busy_elsewhere(target, cx))
            .collect()
    }

    /// When quitting, processes in other windows count too.
    fn busy_elsewhere(&self, target: CloseTarget, cx: &App) -> Vec<String> {
        if target != CloseTarget::Quit {
            return Vec::new();
        }
        let Some(registry) = cx.try_global::<Workspaces>() else {
            return Vec::new();
        };
        registry
            .0
            .iter()
            .filter_map(WeakEntity::upgrade)
            .filter(|workspace| workspace.entity_id() != self.entity_id)
            .flat_map(|workspace| workspace.read(cx).busy(CloseTarget::Window, cx))
            .collect()
    }

    /// Whether it can close right away. If processes would be killed, opens the confirmation
    /// and returns `false`; repeating the request while it's open (⌘W ⌘W, ⌘Q ⌘Q) confirms.
    pub(super) fn should_close(
        &mut self,
        target: CloseTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.pending_close == Some(target) || self.busy(target, cx).is_empty() {
            self.pending_close = None;
            return true;
        }
        self.pending_close = Some(target);
        self.confirm_focus.focus(window, cx);
        cx.notify();
        false
    }

    pub(super) fn request_close(
        &mut self,
        target: CloseTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.should_close(target, window, cx) {
            self.perform_close(target, window, cx);
        }
    }

    pub(super) fn request_close_tab(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.tabs.get(ix) {
            let target = CloseTarget::Tab(tab.id);
            self.request_close(target, window, cx);
        }
    }

    /// ⌘W: closes the active pane, or the tab if it has only one.
    pub(super) fn request_close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let target = if tab.panes.len() > 1 {
            CloseTarget::Pane(tab.active_pane)
        } else {
            CloseTarget::Tab(tab.id)
        };
        self.request_close(target, window, cx);
    }

    fn perform_close(&mut self, target: CloseTarget, window: &mut Window, cx: &mut Context<Self>) {
        match target {
            CloseTarget::Tab(id) => {
                if let Some(ix) = self.tabs.iter().position(|tab| tab.id == id) {
                    self.close(ix, window, cx);
                }
            }
            CloseTarget::Pane(id) => {
                if let Some(ix) = self.tabs.iter().position(|tab| tab.pane(id).is_some()) {
                    self.close_pane(ix, id, window, cx);
                }
            }
            CloseTarget::Window => {
                session::remove(self.entity_id, cx);
                window.remove_window();
            }
            CloseTarget::Quit => cx.quit(),
        }
    }

    fn confirm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(target) = self.pending_close.take() {
            self.perform_close(target, window, cx);
        }
    }

    fn cancel_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_close = None;
        if let Some(tab) = self.tabs.get(self.active) {
            tab.view().focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    pub(super) fn render_close_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let target = self.pending_close?;
        let busy = self.busy(target, cx);
        let (title, confirm) = match target {
            CloseTarget::Tab(_) => ("Close this tab?", "Close"),
            CloseTarget::Pane(_) => ("Close this pane?", "Close"),
            CloseTarget::Window => ("Close this window?", "Close"),
            CloseTarget::Quit => ("Quit Moth?", "Quit"),
        };
        let detail = match busy.as_slice() {
            [] => "No processes are running anymore.".to_string(),
            [only] => format!("This will terminate {only}."),
            many => format!(
                "This will terminate {} processes: {}.",
                many.len(),
                many.join(", ")
            ),
        };
        let ui = cx.tokens().ui.clone();
        Some(
            div()
                .absolute()
                .inset_0()
                .bg(ui.overlay)
                .flex()
                .justify_center()
                .items_start()
                .pt(px(120.))
                .px(px(16.))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.cancel_close(window, cx)),
                )
                .child(
                    v_flex()
                        .id("close-confirm")
                        .key_context("ConfirmDialog")
                        .track_focus(&self.confirm_focus)
                        .on_action(
                            cx.listener(|this, _: &menu::Confirm, w, cx| this.confirm_close(w, cx)),
                        )
                        .on_action(
                            cx.listener(|this, _: &menu::Cancel, w, cx| this.cancel_close(w, cx)),
                        )
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .w(px(420.))
                        .max_w_full()
                        .gap(px(6.))
                        .p(px(16.))
                        .rounded(px(10.))
                        .bg(ui.surface)
                        .border_1()
                        .border_color(ui.border)
                        .shadow_lg()
                        .child(Label::new(title).weight(gpui::FontWeight::SEMIBOLD))
                        .child(Label::new(detail).color(Color::Muted))
                        .child(
                            h_flex()
                                .pt(px(10.))
                                .gap(px(8.))
                                .justify_end()
                                .child(
                                    Button::new("close-cancel", "Cancel")
                                        .style(ButtonStyle::Outlined)
                                        .size(ButtonSize::Large)
                                        .label_size(LabelSize::Small)
                                        .key_binding(ui::KeyBinding::for_action(&menu::Cancel, cx))
                                        .on_click(
                                            cx.listener(|this, _, w, cx| this.cancel_close(w, cx)),
                                        ),
                                )
                                .child(
                                    Button::new("close-confirm-button", confirm)
                                        .style(ButtonStyle::Tinted(ui::TintColor::Error))
                                        .size(ButtonSize::Large)
                                        .label_size(LabelSize::Small)
                                        .key_binding(ui::KeyBinding::for_action(&menu::Confirm, cx))
                                        .on_click(
                                            cx.listener(|this, _, w, cx| this.confirm_close(w, cx)),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
