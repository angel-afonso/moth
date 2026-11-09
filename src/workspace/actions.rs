//! Key bindings and the window's actions.

use super::*;

pub fn init(cx: &mut App) {
    cx.bind_keys(
        [
            shortcuts::bind("cmd-t", NewTab, None),
            shortcuts::bind("cmd-n", NewWindow, None),
            shortcuts::bind("cmd-m", Minimize, None),
            shortcuts::bind("cmd-w", CloseTab, None),
            shortcuts::bind("cmd-shift-t", ReopenTab, None),
            shortcuts::bind("cmd-d", SplitRight, None),
            shortcuts::bind("cmd-shift-d", SplitDown, None),
            shortcuts::bind("cmd-alt-left", FocusPaneLeft, None),
            shortcuts::bind("cmd-alt-right", FocusPaneRight, None),
            shortcuts::bind("cmd-alt-up", FocusPaneUp, None),
            shortcuts::bind("cmd-alt-down", FocusPaneDown, None),
            shortcuts::bind("cmd-]", NextPane, None),
            shortcuts::bind("cmd-[", PreviousPane, None),
            shortcuts::bind("cmd-shift-enter", TogglePaneZoom, None),
            shortcuts::bind("cmd-f", terminal_view::FindOutput, Some("Terminal")),
            shortcuts::bind("cmd-=", terminal_view::ZoomIn, Some("Terminal")),
            shortcuts::bind("cmd-+", terminal_view::ZoomIn, Some("Terminal")),
            shortcuts::bind("cmd--", terminal_view::ZoomOut, Some("Terminal")),
            shortcuts::bind("cmd-0", terminal_view::ResetZoom, Some("Terminal")),
            shortcuts::bind("cmd-up", terminal_view::PreviousCommand, Some("Terminal")),
            shortcuts::bind("cmd-down", terminal_view::NextCommand, Some("Terminal")),
            shortcuts::bind(
                "cmd-shift-c",
                terminal_view::CopyCommandOutput,
                Some("Terminal"),
            ),
            shortcuts::bind(
                "ctrl-space",
                terminal_view::ShowCompletions,
                Some("Terminal"),
            ),
            shortcuts::bind("cmd-i", terminal_view::AskAgent, Some("Terminal")),
            shortcuts::bind("cmd-shift-i", terminal_view::FixWithAgent, Some("Terminal")),
            // The last binding for each action is the one shown in menus and buttons.
            shortcuts::bind("ctrl-tab", NextTab, None),
            shortcuts::bind("ctrl-shift-tab", PreviousTab, None),
            shortcuts::bind("cmd-}", NextTab, None),
            shortcuts::bind("cmd-{", PreviousTab, None),
            shortcuts::bind("cmd-1", ActivateTab1, None),
            shortcuts::bind("cmd-2", ActivateTab2, None),
            shortcuts::bind("cmd-3", ActivateTab3, None),
            shortcuts::bind("cmd-4", ActivateTab4, None),
            shortcuts::bind("cmd-5", ActivateTab5, None),
            shortcuts::bind("cmd-6", ActivateTab6, None),
            shortcuts::bind("cmd-7", ActivateTab7, None),
            shortcuts::bind("cmd-8", ActivateTab8, None),
            shortcuts::bind("cmd-9", ActivateLastTab, None),
            shortcuts::bind("cmd-q", Quit, None),
            shortcuts::bind("cmd-v", terminal_view::Paste, Some("Terminal")),
            shortcuts::bind("cmd-c", terminal_view::Copy, Some("Terminal")),
            shortcuts::bind("cmd-shift-p", ToggleCommandPalette, None),
            shortcuts::bind("cmd-k", ToggleCommandPalette, None),
            shortcuts::bind("cmd-r", SearchHistory, None),
            shortcuts::bind("cmd-l", ClearScreen, None),
            shortcuts::bind("cmd-shift-r", Reconnect, None),
            shortcuts::bind("enter", menu::Confirm, Some("ConfirmDialog")),
            shortcuts::bind("escape", menu::Cancel, Some("ConfirmDialog")),
        ]
        .into_iter()
        .flatten(),
    );
    // On Linux and Windows, also the keys their terminals use for tabs and the clipboard.
    if !cfg!(target_os = "macos") {
        cx.bind_keys([
            KeyBinding::new("ctrl-pagedown", NextTab, None),
            KeyBinding::new("ctrl-pageup", PreviousTab, None),
            KeyBinding::new("shift-insert", terminal_view::Paste, Some("Terminal")),
            KeyBinding::new("ctrl-insert", terminal_view::Copy, Some("Terminal")),
        ]);
    }
    hosts::reload(cx);
    crate::completions::init(cx);
    cx.on_action(|_: &Quit, cx| cx.quit());
    picker::init(cx);
}

impl Workspace {
    /// The window's actions, on its root element.
    pub(super) fn register_actions(root: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        root.on_action(cx.listener(|this, _: &NewTab, window, cx| this.add_tab(window, cx)))
            .on_action(cx.listener(|this, _: &NewWindow, w, cx| this.new_window(w, cx)))
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &ZoomWindow, window, _| window.zoom_window())
            .on_action(cx.listener(|this, _: &ReopenTab, w, cx| this.reopen_tab(w, cx)))
            .on_action(
                cx.listener(|this, _: &CloseTab, window, cx| this.request_close_active(window, cx)),
            )
            .on_action(cx.listener(|this, _: &SplitRight, w, cx| this.split(Axis::Row, w, cx)))
            .on_action(cx.listener(|this, _: &SplitDown, w, cx| this.split(Axis::Column, w, cx)))
            .on_action(cx.listener(|this, _: &FocusPaneLeft, w, cx| {
                this.focus_neighbor(Direction::Left, w, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusPaneRight, w, cx| {
                this.focus_neighbor(Direction::Right, w, cx)
            }))
            .on_action(
                cx.listener(|this, _: &FocusPaneUp, w, cx| {
                    this.focus_neighbor(Direction::Up, w, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &FocusPaneDown, w, cx| {
                this.focus_neighbor(Direction::Down, w, cx)
            }))
            .on_action(cx.listener(|this, _: &NextPane, w, cx| this.cycle_pane(1, w, cx)))
            .on_action(cx.listener(|this, _: &PreviousPane, w, cx| this.cycle_pane(-1, w, cx)))
            .on_action(cx.listener(|this, _: &TogglePaneZoom, _, cx| this.toggle_zoom(cx)))
            .on_action(cx.listener(|this, _: &Quit, window, cx| {
                this.request_close(CloseTarget::Quit, window, cx)
            }))
            .on_action(
                cx.listener(|this, action: &OpenFile, w, cx| this.open_file(&action.file, w, cx)),
            )
            .on_action(cx.listener(|this, action: &OpenFolderInNewTab, w, cx| {
                this.add_tab_in(Some(action.dir.clone()), w, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenConfig, w, cx| this.open_config(w, cx)))
            .on_action(
                cx.listener(|this, _: &terminal_view::RestartShell, w, cx| {
                    this.restart_shell(w, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &NextTab, window, cx| {
                let next = (this.active + 1) % this.tabs.len();
                this.activate(next, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| {
                let len = this.tabs.len();
                this.activate((this.active + len - 1) % len, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ActivateTab1, w, cx| this.activate(0, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab2, w, cx| this.activate(1, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab3, w, cx| this.activate(2, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab4, w, cx| this.activate(3, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab5, w, cx| this.activate(4, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab6, w, cx| this.activate(5, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab7, w, cx| this.activate(6, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab8, w, cx| this.activate(7, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateLastTab, w, cx| {
                let last = this.tabs.len().saturating_sub(1);
                this.activate(last, w, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleCommandPalette, w, cx| {
                this.toggle_command_palette(w, cx)
            }))
            .on_action(cx.listener(|this, _: &SearchHistory, w, cx| this.search_history(w, cx)))
            .on_action(cx.listener(|this, _: &SwitchBranch, w, cx| this.switch_branch(w, cx)))
            .on_action(cx.listener(|this, _: &SelectTheme, w, cx| this.select_theme(w, cx)))
            .on_action(cx.listener(|this, _: &ClearScreen, _, cx| {
                this.active_terminal(cx).update(cx, |t, cx| t.clear(cx))
            }))
            .on_action(cx.listener(|this, _: &ConnectToHost, w, cx| this.connect_to_host(w, cx)))
            .on_action(cx.listener(|this, _: &Reconnect, w, cx| {
                let active = this.active;
                this.reconnect_tab(active, w, cx)
            }))
    }
}
