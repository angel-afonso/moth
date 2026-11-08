//! The window's chrome around the panes: tab bar, status bar, the remote session's status bar
//! and production strip, and the screen shown when no terminal could start.

use super::*;

pub(super) fn short_command(command: &str) -> String {
    let words: Vec<&str> = command.split_whitespace().take(3).collect();
    let mut text = words.join(" ");
    if text.chars().count() > 28 {
        text = text.chars().take(27).collect::<String>() + "…";
    }
    text
}

fn dir_name(path: &Path) -> String {
    if state::home_dir().is_some_and(|home| home == path) {
        return "~".into();
    }
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// `~` in place of home and, for long paths, only the last three folders (like the prompt).
pub(super) fn abbreviate_home(path: &Path) -> String {
    let (prefix, rest) = match state::home_dir()
        .and_then(|home| path.strip_prefix(&home).ok().map(Path::to_path_buf))
    {
        Some(rest) if rest.as_os_str().is_empty() => return "~".into(),
        Some(rest) => ("~/", rest),
        None => ("/", path.strip_prefix("/").unwrap_or(path).to_path_buf()),
    };
    let parts: Vec<_> = rest
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect();
    if parts.len() > 3 {
        format!("…/{}", parts[parts.len() - 3..].join("/"))
    } else {
        format!("{prefix}{}", parts.join("/"))
    }
}

impl Workspace {
    /// The running command, else the current folder, else the title set by the shell.
    pub(super) fn tab_title(&self, ix: usize, cx: &App) -> SharedString {
        let terminal = self.tabs[ix].view().read(cx).terminal().read(cx);
        let shell = terminal.shell();
        // Over SSH, the host; if disconnected, still the host (struck through in the tab).
        if let Some(remote) = &shell.remote {
            return remote.target.alias.clone().into();
        }
        if let (Some(disconnected), None) = (&shell.disconnected, &shell.running) {
            return disconnected.target.alias.clone().into();
        }
        if let Some(running) = shell.running.as_ref().filter(|r| !r.text.trim().is_empty()) {
            return short_command(&running.text).into();
        }
        if let Some(cwd) = &shell.cwd {
            return dir_name(cwd).into();
        }
        match terminal.title() {
            Some(title) if !title.trim().is_empty() => title.trim().to_string().into(),
            _ => self.shell_name.clone().into(),
        }
    }

    /// Whether any pane in the tab has a running command.
    fn is_running(&self, ix: usize, cx: &App) -> bool {
        self.tabs[ix].panes.iter().any(|pane| {
            pane.view
                .read(cx)
                .terminal()
                .read(cx)
                .shell()
                .running
                .is_some()
        })
    }

    fn render_tab(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let state = if self.tabs[ix].needs_attention {
            TabState::Attention
        } else if self.is_running(ix, cx) {
            TabState::Running
        } else {
            TabState::Idle
        };
        let shell = self.tabs[ix].shell(cx);
        let remote = shell
            .remote
            .as_ref()
            .map(|r| (r.identity.color(cx), r.identity.label.clone()))
            .or_else(|| {
                shell
                    .disconnected
                    .as_ref()
                    .filter(|_| shell.running.is_none())
                    .map(|d| (d.identity.color(cx), d.identity.label.clone()))
            });
        let disconnected =
            shell.remote.is_none() && shell.running.is_none() && shell.disconnected.is_some();
        TabItem::new(("tab", ix), self.tab_title(ix, cx))
            .selected(ix == self.active)
            .state(state)
            .when_some(remote, |tab, (color, label)| {
                tab.remote(color, label.map(Into::into))
            })
            .when(disconnected, |tab| {
                tab.disconnected(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.reconnect_tab(ix, window, cx)
                }))
            })
            .when(ix < 9, |tab| {
                tab.shortcut(shortcuts::label(&format!("cmd-{}", ix + 1)))
            })
            .on_click(
                cx.listener(move |this, _: &ClickEvent, window, cx| this.activate(ix, window, cx)),
            )
            .on_close(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.request_close_tab(ix, window, cx)
            }))
            .on_middle_click(cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.request_close_tab(ix, window, cx)
            }))
    }

    /// Draggable region (transparent title bar on macOS).
    fn drag_region(&self, id: &'static str, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .h_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.should_move = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.should_move = false),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, _| this.should_move = false))
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.should_move {
                    this.should_move = false;
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    window.titlebar_double_click();
                }
            })
    }

    pub(super) fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = (0..self.tabs.len())
            .map(|ix| self.render_tab(ix, cx).into_any_element())
            .collect::<Vec<_>>();
        let ui = cx.tokens().ui.clone();
        let border = ui.border;

        h_flex()
            .h(TAB_BAR_HEIGHT)
            .w_full()
            .flex_none()
            .bg(ui.chrome)
            // Room for the macOS traffic lights; elsewhere the system draws the title bar.
            .child(
                self.drag_region("traffic-lights", cx)
                    .w(px(if cfg!(target_os = "macos") { 80. } else { 8. }))
                    .flex_none()
                    .border_b_1()
                    .border_color(border),
            )
            .child(
                h_flex()
                    .id("tabs")
                    .h_full()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_scroll()
                    .children(tabs)
                    .child(
                        h_flex()
                            .h_full()
                            .flex_none()
                            .px(px(6.))
                            .border_b_1()
                            .border_color(border)
                            .child(
                                IconButton::new("new-tab", IconName::Plus)
                                    .icon_size(IconSize::Small)
                                    .size(ButtonSize::Medium)
                                    .tooltip(Tooltip::for_action_title("New Tab", &NewTab))
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(NewTab.boxed_clone(), cx)
                                    }),
                            ),
                    )
                    .child(
                        self.drag_region("tab-bar-drag", cx)
                            .flex_1()
                            .min_w(px(24.))
                            .border_b_1()
                            .border_color(border),
                    ),
            )
            .child(
                h_flex()
                    .h_full()
                    .flex_none()
                    .px(px(10.))
                    .border_b_1()
                    .border_color(border)
                    .child(
                        Button::new("command-palette", "Commands")
                            .style(ButtonStyle::Outlined)
                            .size(ButtonSize::Medium)
                            .label_size(LabelSize::Small)
                            .color(Color::Muted)
                            .start_icon(
                                Icon::new(IconName::MagnifyingGlass)
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .key_binding(ui::KeyBinding::for_action(&ToggleCommandPalette, cx))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(ToggleCommandPalette.boxed_clone(), cx)
                            }),
                    ),
            )
    }

    /// Current branch with changes and ahead/behind; clicking lists branches to switch to.
    fn render_git(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let terminal = self.active_terminal(cx);
        let git = terminal.read(cx).shell().git.clone()?;
        let branch_color = Color::Custom(cx.tokens().syntax.subcommand);

        let mut sync = Vec::new();
        if git.behind > 0 {
            sync.push(format!("↓{}", git.behind));
        }
        if git.ahead > 0 {
            sync.push(format!("↑{}", git.ahead));
        }
        let mut changes = Vec::new();
        if git.staged > 0 {
            changes.push(format!("+{}", git.staged));
        }
        if git.modified > 0 {
            changes.push(format!("~{}", git.modified));
        }
        if git.untracked > 0 {
            changes.push(format!("?{}", git.untracked));
        }
        if git.conflicted > 0 {
            changes.push(format!("!{}", git.conflicted));
        }

        let menu = Button::new("branch", git.display_name())
            .style(ButtonStyle::Subtle)
            .size(ButtonSize::Compact)
            .label_size(LabelSize::Small)
            .color(branch_color)
            .start_icon(
                Icon::new(IconName::GitBranch)
                    .size(IconSize::XSmall)
                    .color(branch_color),
            )
            .tooltip(Tooltip::for_action_title("Switch Branch", &SwitchBranch))
            .on_click(|_, window, cx| window.dispatch_action(SwitchBranch.boxed_clone(), cx));

        Some(
            h_flex()
                .gap_2()
                .child(menu)
                .when(!sync.is_empty(), |this| {
                    this.child(
                        Label::new(sync.join(" "))
                            .size(LabelSize::Small)
                            .color(Color::Warning),
                    )
                })
                .when(!changes.is_empty(), |this| {
                    this.child(
                        Label::new(changes.join(" "))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                })
                .into_any_element(),
        )
    }

    pub(super) fn render_status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(remote) = self.render_remote_status_bar(cx) {
            return remote;
        }
        let git = self.render_git(cx);
        let ui = cx.tokens().ui.clone();
        let terminal = self.active_terminal(cx);
        let terminal = terminal.read(cx);
        let size = terminal.size();
        let shell = terminal.shell();
        let tab_count = self.tabs.len();

        let (dot, block) = match (&shell.running, &shell.last) {
            (Some(running), _) => (
                DotState::Attention,
                Some(BlockStatus::Running(running.started.elapsed())),
            ),
            (None, Some(last)) => {
                let exit_code = last.exit_code.unwrap_or(0);
                let dot = if exit_code == 0 {
                    DotState::Success
                } else {
                    DotState::Error
                };
                (
                    dot,
                    Some(BlockStatus::Finished {
                        exit_code,
                        duration: last.duration,
                    }),
                )
            }
            (None, None) => (DotState::Success, None),
        };

        h_flex()
            .h(px(28.))
            .flex_none()
            .px(px(14.))
            .gap(metrics::SPACE_4)
            .justify_between()
            .overflow_hidden()
            .bg(ui.chrome)
            .border_t_1()
            .border_color(ui.border)
            .child(
                h_flex()
                    .gap(metrics::SPACE_4)
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .flex_none()
                            .child(StatusDot::new(dot).small())
                            .child(
                                Label::new(self.shell_name.clone())
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                    )
                    .when_some(shell.cwd.as_ref(), |this, cwd| {
                        this.child(
                            div().min_w_0().child(
                                Label::new(abbreviate_home(cwd))
                                    .size(LabelSize::Small)
                                    .color(Color::Placeholder)
                                    .truncate(),
                            ),
                        )
                    })
                    .children(git),
            )
            .child(
                h_flex()
                    .gap(metrics::SPACE_4)
                    .flex_none()
                    .font_family(MONO_FONT_FAMILY)
                    .children(block)
                    .child(
                        Label::new(if tab_count == 1 {
                            "1 tab".to_string()
                        } else {
                            format!("{tab_count} tabs")
                        })
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder),
                    )
                    .child(
                        Label::new(format!("{}×{}", size.columns, size.lines))
                            .size(LabelSize::XSmall)
                            .color(Color::Placeholder),
                    )
                    .child(updates::status_item(cx)),
            )
            .into_any_element()
    }

    pub(super) fn active_is_production(&self, cx: &App) -> bool {
        self.tabs
            .get(self.active)
            .and_then(|tab| {
                tab.view()
                    .read(cx)
                    .terminal()
                    .read(cx)
                    .shell()
                    .remote
                    .as_ref()
                    .map(|r| r.identity.production)
            })
            .unwrap_or(false)
    }

    /// Banner below the tabs when the active session is a production one.
    pub(super) fn render_production_strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let terminal = self.active_terminal(cx);
        let terminal = terminal.read(cx);
        let remote = terminal
            .shell()
            .remote
            .as_ref()
            .filter(|r| r.identity.production)?;
        let color = remote.identity.color(cx);
        let label = remote
            .identity
            .label
            .clone()
            .map(|l| {
                if l.eq_ignore_ascii_case("prod") {
                    "PRODUCTION".to_string()
                } else {
                    l.to_uppercase()
                }
            })
            .unwrap_or_else(|| "PRODUCTION".into());
        let reason = match (&remote.identity.matched, &hosts::rules(cx).source) {
            (Some(pattern), RulesSource::File(_)) => {
                format!("· matches {pattern} in hosts.json")
            }
            (Some(pattern), RulesSource::Defaults) => {
                format!("· matches {pattern} (default rule)")
            }
            (None, _) => String::new(),
        };
        Some(
            h_flex()
                .h(px(30.))
                .flex_none()
                .px(metrics::SPACE_4)
                .gap(px(10.))
                .justify_between()
                .overflow_hidden()
                .bg(tint(color, 0.12))
                .border_b_1()
                .border_color(tint(color, 0.35))
                .child(
                    h_flex()
                        .gap(px(10.))
                        .min_w_0()
                        .child(
                            Icon::new(IconName::Warning)
                                .size(IconSize::Small)
                                .color(Color::Custom(color)),
                        )
                        .child(
                            Label::new(label)
                                .size(LabelSize::Small)
                                .weight(gpui::FontWeight::SEMIBOLD)
                                .color(Color::Custom(color)),
                        )
                        .child(
                            div()
                                .font_family(MONO_FONT_FAMILY)
                                .child(Label::new(remote.user_at_host()).size(LabelSize::Small)),
                        )
                        .child(
                            Label::new(reason)
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .truncate(),
                        ),
                )
                .child(
                    h_flex()
                        .gap(px(6.))
                        .flex_none()
                        .child(
                            Icon::new(IconName::Lock)
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        )
                        .child(
                            Label::new("Paste protection")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_remote_status_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ui = cx.tokens().ui.clone();
        let terminal = self.active_terminal(cx);
        let terminal = terminal.read(cx);
        let size = terminal.size();
        let remote = terminal.shell().remote.as_ref()?;
        let color = remote.identity.color(cx);
        let address = remote
            .resolved
            .as_ref()
            .map(|r| format!("{}:{}", r.hostname, r.port));
        let minutes = remote.started.elapsed().as_secs() / 60;
        let connected = if minutes == 0 {
            "connected · just now".to_string()
        } else if minutes < 60 {
            format!("connected · {minutes} min")
        } else {
            format!("connected · {}h {}min", minutes / 60, minutes % 60)
        };
        let tab_count = self.tabs.len();

        Some(
            h_flex()
                .h(px(28.))
                .flex_none()
                .px(px(14.))
                .gap(metrics::SPACE_4)
                .justify_between()
                .overflow_hidden()
                .bg(ui.chrome)
                .border_t_1()
                .border_color(ui.border)
                .child(
                    h_flex()
                        .gap(metrics::SPACE_4)
                        .min_w_0()
                        .child(
                            h_flex()
                                .gap(px(6.))
                                .flex_none()
                                .child(div().size(px(6.)).rounded_full().bg(color))
                                .child(
                                    Label::new("ssh").size(LabelSize::Small).color(Color::Muted),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap(px(6.))
                                .flex_none()
                                .child(
                                    Icon::new(IconName::ArrowRightLeft)
                                        .size(IconSize::XSmall)
                                        .color(Color::Custom(color)),
                                )
                                .child(
                                    div().font_family(MONO_FONT_FAMILY).child(
                                        Label::new(remote.user_at_host())
                                            .size(LabelSize::Small)
                                            .color(Color::Custom(color)),
                                    ),
                                ),
                        )
                        .when_some(address, |this, address| {
                            this.child(
                                div().font_family(MONO_FONT_FAMILY).child(
                                    Label::new(address)
                                        .size(LabelSize::XSmall)
                                        .color(Color::Placeholder),
                                ),
                            )
                        })
                        .when_some(remote.remote_cwd.clone(), |this, cwd| {
                            this.child(
                                div().min_w_0().child(
                                    Label::new(cwd)
                                        .size(LabelSize::Small)
                                        .color(Color::Placeholder)
                                        .truncate(),
                                ),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .gap(metrics::SPACE_4)
                        .flex_none()
                        .font_family(MONO_FONT_FAMILY)
                        .child(
                            Label::new(connected)
                                .size(LabelSize::XSmall)
                                .color(Color::Placeholder),
                        )
                        .child(
                            Label::new(if tab_count == 1 {
                                "1 tab".to_string()
                            } else {
                                format!("{tab_count} tabs")
                            })
                            .size(LabelSize::XSmall)
                            .color(Color::Placeholder),
                        )
                        .child(
                            Label::new(format!("{}×{}", size.columns, size.lines))
                                .size(LabelSize::XSmall)
                                .color(Color::Placeholder),
                        ),
                )
                .into_any_element(),
        )
    }

    /// What the window shows when not even one terminal could start: the reason, and ways
    /// to fix the settings and try again. Everything else assumes there's a tab.
    pub(super) fn render_without_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let reason = self
            .command_notice
            .clone()
            .unwrap_or_else(|| "Couldn't open the terminal.".to_string());
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .bg(cx.tokens().ui.background)
            .text_color(cx.tokens().ui.text)
            .font_family(UI_FONT_FAMILY)
            .text_size(UI_FONT_SIZE)
            .child(Label::new(reason))
            .child(
                Label::new("Check \"shell\" in the settings, or see the log for details.")
                    .color(Color::Muted)
                    .size(LabelSize::Small),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        Button::new("no-tabs-settings", "Open Settings")
                            .style(ButtonStyle::Outlined)
                            .size(ButtonSize::Large)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| match config::ensure_file() {
                                Ok(path) => cx.open_with_system(&path),
                                Err(err) => {
                                    this.command_notice =
                                        Some(format!("Couldn't create config.json: {err}"));
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        Button::new("no-tabs-retry", "Try Again")
                            .style(ButtonStyle::Filled)
                            .size(ButtonSize::Large)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_tab_in(state::last_cwd(), window, cx);
                            })),
                    ),
            )
    }
}
