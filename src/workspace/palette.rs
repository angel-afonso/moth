//! The modal pickers: the command palette (its list of actions as data), history search,
//! branches, themes and "Connect to…".

use super::*;

/// The history search's filters, after "All": positions in its `PickerConfig::filters`.
const HISTORY_THIS_FOLDER: usize = 1;
const HISTORY_FAILED: usize = 2;

#[derive(Clone, Copy, PartialEq)]
enum ModalKind {
    CommandPalette,
    History,
    Branches,
    Themes,
    Hosts,
}

pub(super) struct Modal {
    kind: ModalKind,
    picker: Entity<Picker>,
    /// Theme to restore if the picker closes without a choice (undoes the preview).
    restore_theme: Option<Arc<Tokens>>,
    _subscription: Subscription,
}

impl Workspace {
    fn open_modal(
        &mut self,
        kind: ModalKind,
        items: Vec<PickerItem>,
        config: PickerConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.modal.as_ref().is_some_and(|m| m.kind == kind) {
            self.dismiss_modal(window, cx);
            return;
        }
        let picker = cx.new(|cx| Picker::new(items, config, cx));
        let subscription = cx.subscribe_in(&picker, window, Self::on_picker_event);
        picker.focus_handle(cx).focus(window, cx);
        self.modal = Some(Modal {
            kind,
            picker,
            restore_theme: (kind == ModalKind::Themes).then(|| cx.tokens().clone()),
            _subscription: subscription,
        });
        cx.notify();
    }

    fn dismiss_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(modal) = self.modal.take() {
            if let Some(previous) = modal.restore_theme {
                theme::apply(previous, cx);
            }
            self.active_tab().view().focus_handle(cx).focus(window, cx);
            cx.notify();
        }
    }

    fn on_picker_event(
        &mut self,
        _: &Entity<Picker>,
        event: &PickerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (action, secondary) = match event {
            PickerEvent::Selected(PickerAction::SetTheme(name)) => {
                // Preview: apply without saving.
                let preview = cx
                    .global::<Themes>()
                    .0
                    .iter()
                    .find(|t| &t.name == name)
                    .cloned();
                if let Some(preview) = preview {
                    theme::apply(preview, cx);
                }
                return;
            }
            PickerEvent::Selected(_) => return,
            PickerEvent::Dismiss => {
                self.dismiss_modal(window, cx);
                return;
            }
            PickerEvent::Confirm { action, secondary } => (action.clone(), *secondary),
        };
        if let Some(modal) = &mut self.modal {
            modal.restore_theme = None;
        }
        self.dismiss_modal(window, cx);
        let terminal = self.active_terminal(cx);
        match action {
            // Dispatch after focus returns to the terminal, so the action reaches it.
            PickerAction::Dispatch(action) => {
                window.defer(cx, move |window, cx| window.dispatch_action(action, cx))
            }
            PickerAction::ActivateTab(ix) => self.activate(ix, window, cx),
            PickerAction::OpenDirInNewTab(dir) => self.add_tab_in(Some(dir), window, cx),
            PickerAction::InsertCommand(command) => {
                terminal.update(cx, |t, cx| t.insert_command(&command, secondary, cx));
            }
            PickerAction::SwitchBranch(branch) => {
                terminal.update(cx, |t, cx| t.switch_branch(&branch, cx));
            }
            PickerAction::SetTheme(name) => theme::select(&name, cx),
            PickerAction::RunCommand(command) => {
                if secondary {
                    terminal.update(cx, |t, cx| t.insert_command(&command, true, cx));
                } else {
                    let cwd = terminal.read(cx).shell().cwd.clone();
                    self.add_tab_in(cwd, window, cx);
                    let terminal = self.active_terminal(cx);
                    terminal.update(cx, |t, cx| t.run_when_ready(command, cx));
                }
            }
            PickerAction::ReconnectTab(ix) => self.reconnect_tab(ix, window, cx),
        }
    }

    pub(super) fn toggle_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = self.active_terminal(cx);
        let shell = terminal.read(cx).shell();
        let has_git = shell.git.is_some();
        let cwd = shell.cwd.clone();

        let mut items = vec![
            PickerItem::new(
                "Actions",
                "New Tab",
                PickerAction::Dispatch(Box::new(NewTab)),
            )
            .icon(IconName::Plus)
            .shortcut(NewTab),
            PickerItem::new(
                "Actions",
                "Search History",
                PickerAction::Dispatch(Box::new(SearchHistory)),
            )
            .icon(IconName::HistoryRerun)
            .shortcut(SearchHistory),
        ];
        if self
            .active_terminal(cx)
            .read(cx)
            .shell()
            .disconnected
            .is_some()
        {
            items.push(
                PickerItem::new(
                    "Actions",
                    "Reconnect",
                    PickerAction::Dispatch(Box::new(Reconnect)),
                )
                .icon(IconName::RotateCw)
                .shortcut(Reconnect),
            );
        }
        if has_git {
            items.push(
                PickerItem::new(
                    "Actions",
                    "Switch Branch…",
                    PickerAction::Dispatch(Box::new(SwitchBranch)),
                )
                .icon(IconName::GitBranch),
            );
        }
        items.extend([
            PickerItem::new(
                "Actions",
                "Split Right",
                PickerAction::Dispatch(Box::new(SplitRight)),
            )
            .shortcut(SplitRight),
            PickerItem::new(
                "Actions",
                "Split Down",
                PickerAction::Dispatch(Box::new(SplitDown)),
            )
            .shortcut(SplitDown),
            PickerItem::new(
                "Actions",
                "Maximize or Restore Pane",
                PickerAction::Dispatch(Box::new(TogglePaneZoom)),
            )
            .shortcut(TogglePaneZoom),
            PickerItem::new(
                "Actions",
                "Reopen Closed Tab",
                PickerAction::Dispatch(Box::new(ReopenTab)),
            )
            .shortcut(ReopenTab),
            PickerItem::new(
                "Actions",
                "Find in Output",
                PickerAction::Dispatch(Box::new(terminal_view::FindOutput)),
            )
            .shortcut(terminal_view::FindOutput),
            PickerItem::new(
                "Actions",
                "Show Suggestions",
                PickerAction::Dispatch(Box::new(terminal_view::ShowCompletions)),
            )
            .shortcut(terminal_view::ShowCompletions),
            PickerItem::new(
                "Actions",
                "Ask Agent for a Command…",
                PickerAction::Dispatch(Box::new(terminal_view::AskAgent)),
            )
            .icon(IconName::Sparkle)
            .shortcut(terminal_view::AskAgent),
            PickerItem::new(
                "Actions",
                "Fix Last Command with Agent",
                PickerAction::Dispatch(Box::new(terminal_view::FixWithAgent)),
            )
            .icon(IconName::Sparkle)
            .shortcut(terminal_view::FixWithAgent),
            PickerItem::new(
                "Actions",
                "Agent Mode",
                PickerAction::Dispatch(Box::new(terminal_view::ToggleAgentMode)),
            )
            .icon(IconName::Sparkle)
            .shortcut(terminal_view::ToggleAgentMode),
            PickerItem::new(
                "Actions",
                "Copy Last Command Output",
                PickerAction::Dispatch(Box::new(terminal_view::CopyCommandOutput)),
            )
            .shortcut(terminal_view::CopyCommandOutput),
            PickerItem::new(
                "Actions",
                "Go to Previous Command",
                PickerAction::Dispatch(Box::new(terminal_view::PreviousCommand)),
            )
            .shortcut(terminal_view::PreviousCommand),
            PickerItem::new(
                "Actions",
                "Zoom In",
                PickerAction::Dispatch(Box::new(terminal_view::ZoomIn)),
            ),
            PickerItem::new(
                "Actions",
                "Zoom Out",
                PickerAction::Dispatch(Box::new(terminal_view::ZoomOut)),
            ),
            PickerItem::new(
                "Actions",
                "Reset Zoom",
                PickerAction::Dispatch(Box::new(terminal_view::ResetZoom)),
            ),
            PickerItem::new(
                "Actions",
                if cx.global::<terminal_view::OptionAsMeta>().0 {
                    "Stop Using ⌥ as Meta"
                } else {
                    "Use ⌥ as Meta (⌥B, ⌥F, ⌥.)"
                },
                PickerAction::Dispatch(Box::new(terminal_view::ToggleOptionAsMeta)),
            ),
            PickerItem::new(
                "Actions",
                "Connect to…",
                PickerAction::Dispatch(Box::new(ConnectToHost)),
            )
            .icon(IconName::Server),
            PickerItem::new(
                "Actions",
                "Change Theme…",
                PickerAction::Dispatch(Box::new(SelectTheme)),
            )
            .icon(IconName::Sparkle),
            PickerItem::new(
                "Actions",
                "Open Settings (font, editor, scrollback)",
                PickerAction::Dispatch(Box::new(OpenConfig)),
            )
            .icon(IconName::Settings),
            PickerItem::new(
                "Actions",
                if updates::ready(cx) {
                    "Restart to Update"
                } else {
                    "Check for Updates…"
                },
                if updates::ready(cx) {
                    PickerAction::Dispatch(Box::new(updates::RestartToUpdate))
                } else {
                    PickerAction::Dispatch(Box::new(updates::CheckForUpdates))
                },
            )
            .icon(IconName::Download),
            PickerItem::new(
                "Actions",
                "Clear Screen",
                PickerAction::Dispatch(Box::new(ClearScreen)),
            )
            .icon(IconName::Eraser)
            .shortcut(ClearScreen),
            PickerItem::new(
                "Actions",
                "Copy Screen",
                PickerAction::Dispatch(Box::new(terminal_view::Copy)),
            )
            .icon(IconName::Copy),
            PickerItem::new(
                "Actions",
                "Paste",
                PickerAction::Dispatch(Box::new(terminal_view::Paste)),
            )
            .icon(IconName::Download),
            PickerItem::new(
                "Actions",
                "Next Tab",
                PickerAction::Dispatch(Box::new(NextTab)),
            )
            .icon(IconName::ArrowRight)
            .shortcut(NextTab),
            PickerItem::new(
                "Actions",
                "Previous Tab",
                PickerAction::Dispatch(Box::new(PreviousTab)),
            )
            .icon(IconName::ArrowLeft)
            .shortcut(PreviousTab),
            PickerItem::new(
                "Actions",
                "Close Tab",
                PickerAction::Dispatch(Box::new(CloseTab)),
            )
            .icon(IconName::Close)
            .shortcut(CloseTab),
            PickerItem::new(
                "Actions",
                "Quit Moth",
                PickerAction::Dispatch(Box::new(Quit)),
            )
            .icon(IconName::Exit)
            .shortcut(Quit),
        ]);

        let activate: [Box<dyn Action>; 8] = [
            Box::new(ActivateTab1),
            Box::new(ActivateTab2),
            Box::new(ActivateTab3),
            Box::new(ActivateTab4),
            Box::new(ActivateTab5),
            Box::new(ActivateTab6),
            Box::new(ActivateTab7),
            Box::new(ActivateTab8),
        ];
        for ix in 0..self.tabs.len() {
            let tab_terminal = self.tabs[ix].view().read(cx).terminal().read(cx);
            let mut item = PickerItem::new(
                "Open Tabs",
                format!("Go to {}", self.tab_title(ix, cx)),
                PickerAction::ActivateTab(ix),
            )
            .icon(IconName::Terminal);
            if let Some(dir) = &tab_terminal.shell().cwd {
                item = item.detail(abbreviate_home(dir));
            }
            if let Some(action) = activate.get(ix) {
                item.shortcut = Some(action.boxed_clone());
            }
            items.push(item);
        }

        for dir in state::recent_dirs()
            .into_iter()
            .filter(|d| Some(d) != cwd.as_ref())
            .take(8)
        {
            items.push(
                PickerItem::new(
                    "Recent Folders",
                    abbreviate_home(&dir),
                    PickerAction::OpenDirInNewTab(dir),
                )
                .kind(SuggestionKind::Path)
                .detail("open in new tab")
                .mono(),
            );
        }

        let config = PickerConfig {
            placeholder: "Search commands, tabs, or folders…".into(),
            query: String::new(),
            empty_text: "No matches".into(),
            hints: vec![
                ("↑↓".into(), "navigate".into()),
                ("⏎".into(), "run".into()),
                ("esc".into(), "close".into()),
            ],
            disabled_reason: None,
            selected_item: None,
            filters: Vec::new(),
        };
        self.open_modal(ModalKind::CommandPalette, items, config, window, cx);
    }

    pub(super) fn search_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = self.active_terminal(cx);
        let terminal = terminal.read(cx);
        // Start with whatever was already typed at the prompt, like ⌃R.
        let query = terminal
            .shell()
            .line
            .as_ref()
            .map(|line| line.buffer.clone())
            .unwrap_or_default();
        let cwd = terminal.shell().cwd.as_deref();
        let items: Vec<PickerItem> = shell::history(cx)
            .map(|history| {
                history
                    .entries
                    .iter()
                    .rev()
                    .map(|command| {
                        let mut item = PickerItem::new(
                            "",
                            command.clone(),
                            PickerAction::InsertCommand(command.clone()),
                        )
                        .kind(SuggestionKind::History)
                        .mono();
                        // Where it last ran and whether it failed, for what ran in Moth.
                        let Some(usage) = history.usage(command) else {
                            return item;
                        };
                        if cwd.is_some_and(|cwd| usage.ran_in(cwd)) {
                            item = item.in_filter(HISTORY_THIS_FOLDER);
                        }
                        let mut detail = usage
                            .last_dir()
                            .map(chrome::abbreviate_home)
                            .unwrap_or_default();
                        if let Some(code) = usage.failure() {
                            item = item.in_filter(HISTORY_FAILED);
                            if !detail.is_empty() {
                                detail.push_str(" · ");
                            }
                            detail.push_str(&format!("exit {code}"));
                        }
                        if detail.is_empty() {
                            item
                        } else {
                            item.detail(detail)
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let config = PickerConfig {
            placeholder: "Search history…".into(),
            query,
            empty_text: if items.is_empty() {
                "History hasn't loaded yet or is empty".into()
            } else {
                "No matching commands".into()
            },
            hints: vec![
                ("↑↓".into(), "navigate".into()),
                ("Tab".into(), "filter".into()),
                ("⏎".into(), "insert at prompt".into()),
                (shortcuts::label("cmd-enter").into(), "run".into()),
                ("esc".into(), "close".into()),
            ],
            disabled_reason: (!terminal.at_prompt())
                .then(|| "A command is running; wait for it to finish to use history".into()),
            selected_item: None,
            filters: vec!["All".into(), "This folder".into(), "Failed".into()],
        };
        self.open_modal(ModalKind::History, items, config, window, cx);
    }

    pub(super) fn switch_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = self.active_terminal(cx);
        let terminal = terminal.read(cx);
        let Some(git) = terminal.shell().git.clone() else {
            return;
        };
        let items = git
            .branches
            .iter()
            .map(|branch| {
                let current = git.branch.as_ref() == Some(branch);
                let item = PickerItem::new(
                    "",
                    branch.clone(),
                    PickerAction::SwitchBranch(branch.clone()),
                )
                .kind(SuggestionKind::Branch);
                if current {
                    item.detail("current")
                } else {
                    item
                }
            })
            .collect();
        let config = PickerConfig {
            placeholder: "Switch to branch…".into(),
            query: String::new(),
            empty_text: "No matching branches".into(),
            hints: vec![
                ("↑↓".into(), "navigate".into()),
                ("⏎".into(), "git switch".into()),
                ("esc".into(), "close".into()),
            ],
            disabled_reason: (!terminal.at_prompt())
                .then(|| "A command is running; wait for it to finish to switch branches".into()),
            selected_item: None,
            filters: Vec::new(),
        };
        self.open_modal(ModalKind::Branches, items, config, window, cx);
    }

    pub(super) fn select_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Reload them in case the user added or edited a theme since last time.
        let themes = theme::reload_themes(cx);
        let current = cx.tokens().name.clone();
        let items = themes
            .iter()
            .map(|t| {
                let appearance = match t.appearance {
                    Appearance::Dark => "dark",
                    Appearance::Light => "light",
                };
                let mut item =
                    PickerItem::new("", t.name.clone(), PickerAction::SetTheme(t.name.clone()))
                        .icon(if t.name == current {
                            IconName::Check
                        } else {
                            IconName::Sparkle
                        })
                        .detail(appearance);
                if t.name == current {
                    item = item.detail(format!("{appearance} · current"));
                }
                item
            })
            .collect();
        let config = PickerConfig {
            placeholder: "Choose a theme…".into(),
            query: String::new(),
            empty_text: "No matching themes".into(),
            hints: vec![
                ("↑↓".into(), "preview".into()),
                ("⏎".into(), "apply".into()),
                ("esc".into(), "revert".into()),
            ],
            disabled_reason: None,
            selected_item: themes.iter().position(|t| t.name == current),
            filters: Vec::new(),
        };
        self.open_modal(ModalKind::Themes, items, config, window, cx);
    }

    pub(super) fn connect_to_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        hosts::reload(cx);
        let rules = hosts::rules(cx);
        let terminal_colors = cx.tokens().terminal.clone();
        let identity_parts = |alias: &str, hostname: Option<&str>| {
            let identity = rules.identify(alias, hostname);
            (identity.color.resolve(&terminal_colors), identity.label)
        };
        let mut items = Vec::new();

        // Dropped sessions, from any tab.
        for (ix, tab) in self.tabs.iter().enumerate() {
            let shell = tab.shell(cx);
            let Some(d) = shell
                .disconnected
                .as_ref()
                .filter(|_| shell.running.is_none())
            else {
                continue;
            };
            let minutes = d.at.elapsed().as_secs() / 60;
            let ago = if minutes == 0 {
                "just now".to_string()
            } else {
                format!("{minutes} min ago")
            };
            let mut item = PickerItem::new(
                "Reconnect",
                d.target.alias.clone(),
                PickerAction::ReconnectTab(ix),
            )
            .icon(IconName::RotateCw)
            .detail(format!(
                "disconnected {ago} · exit {}",
                d.exit_code.unwrap_or(255)
            ))
            .mono();
            if let Some(label) = d.identity.label.clone() {
                item = item.chip(label, d.identity.color(cx));
            }
            if ix == self.active {
                item.shortcut = Some(Box::new(Reconnect));
            }
            items.push(item);
        }

        // Tabs with an open session, so each host can say so.
        let open_tabs: Vec<(usize, String)> = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(ix, tab)| {
                let shell = tab.shell(cx);
                shell.remote.as_ref().map(|r| (ix, r.target.alias.clone()))
            })
            .collect();

        let config_hosts = ssh::config_hosts();
        for host in &config_hosts {
            let (color, label) = identity_parts(&host.alias, host.hostname.as_deref());
            let open = open_tabs
                .iter()
                .find(|(_, alias)| alias == &host.alias)
                .map(|(ix, _)| {
                    format!(
                        "open in {} · ",
                        shortcuts::label(&format!("cmd-{}", ix + 1))
                    )
                })
                .unwrap_or_default();
            let mut item = PickerItem::new(
                "~/.ssh/config",
                host.alias.clone(),
                PickerAction::RunCommand(format!("ssh {}", host.alias)),
            )
            .dot(color)
            .detail(format!("{open}{}", host.describe()))
            .mono();
            if let Some(label) = label {
                item = item.chip(label, color);
            }
            items.push(item);
        }

        // `ssh …` commands from history to targets that aren't in the config.
        let mut seen: Vec<String> = config_hosts.iter().map(|h| h.alias.clone()).collect();
        if let Some(history) = shell::history(cx) {
            for command in history.entries.iter().rev() {
                let Some(target) = ssh::parse_ssh_command(command).filter(|t| !t.remote_command)
                else {
                    continue;
                };
                let key = command.trim().to_string();
                if seen.contains(&key)
                    || seen.contains(&target.alias)
                        && command.trim() == format!("ssh {}", target.alias)
                {
                    continue;
                }
                seen.push(key);
                let (color, label) = identity_parts(&target.alias, None);
                let mut item = PickerItem::new(
                    "From History",
                    command.clone(),
                    PickerAction::RunCommand(command.clone()),
                )
                .kind(SuggestionKind::History)
                .mono();
                if let Some(label) = label {
                    item = item.chip(label, color);
                }
                items.push(item);
                if items.len() > 80 {
                    break;
                }
            }
        }

        let at_prompt = self.active_terminal(cx).read(cx).at_prompt();
        let config = PickerConfig {
            placeholder: "Connect to… alias, user@host, or IP".into(),
            query: String::new(),
            empty_text: if items.is_empty() {
                "No hosts in ~/.ssh/config or history".into()
            } else {
                "No matching hosts".into()
            },
            hints: vec![
                ("↑↓".into(), "navigate".into()),
                ("⏎".into(), "open in new tab".into()),
                (
                    shortcuts::label("cmd-enter").into(),
                    if at_prompt {
                        "in this tab".into()
                    } else {
                        "in this tab (unavailable)".into()
                    },
                ),
                ("esc".into(), "close".into()),
            ],
            disabled_reason: None,
            selected_item: None,
            filters: Vec::new(),
        };
        self.open_modal(ModalKind::Hosts, items, config, window, cx);
    }

    pub(super) fn render_modal(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let picker = self.modal.as_ref()?.picker.clone();
        Some(
            div()
                .absolute()
                .top(TAB_BAR_HEIGHT)
                .left_0()
                .right_0()
                .bottom_0()
                .bg(cx.tokens().ui.overlay)
                .flex()
                .justify_center()
                .items_start()
                .pt(px(72.))
                .px(px(16.))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.dismiss_modal(window, cx)),
                )
                .child(picker),
        )
    }
}
