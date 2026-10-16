//! Ventana: barra de pestañas (componentes `Tab`/`TabBar` de Zed), terminal activa y barra de estado.

use std::{path::Path, time::Duration};

use gpui::{
    Action, App, ClickEvent, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseButton,
    Subscription, Window, actions, div, prelude::*, px,
};
use ui::{
    Color, Icon, IconButton, IconName, IconSize, Label, LabelCommon, LabelSize, Tab, TabBar,
    TabCloseSide, TabPosition, Tooltip, h_flex, prelude::*, v_flex,
};

use crate::{
    palette,
    terminal::{Terminal, TerminalBuilder, TerminalEvent},
    terminal_view::{self, TerminalView},
    theme::{MONO_FONT_FAMILY, UI_FONT_FAMILY, UI_FONT_SIZE},
};

actions!(
    workspace,
    [
        NewTab,
        CloseTab,
        NextTab,
        PreviousTab,
        ActivateTab1,
        ActivateTab2,
        ActivateTab3,
        ActivateTab4,
        ActivateTab5,
        ActivateTab6,
        ActivateTab7,
        ActivateTab8,
        ActivateLastTab,
        Quit,
    ]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("cmd-}", NextTab, None),
        KeyBinding::new("cmd-{", PreviousTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
        KeyBinding::new("cmd-1", ActivateTab1, None),
        KeyBinding::new("cmd-2", ActivateTab2, None),
        KeyBinding::new("cmd-3", ActivateTab3, None),
        KeyBinding::new("cmd-4", ActivateTab4, None),
        KeyBinding::new("cmd-5", ActivateTab5, None),
        KeyBinding::new("cmd-6", ActivateTab6, None),
        KeyBinding::new("cmd-7", ActivateTab7, None),
        KeyBinding::new("cmd-8", ActivateTab8, None),
        KeyBinding::new("cmd-9", ActivateLastTab, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-v", terminal_view::Paste, Some("Terminal")),
        KeyBinding::new("cmd-c", terminal_view::Copy, Some("Terminal")),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
}

struct TabEntry {
    view: Entity<TerminalView>,
    /// La sesión sonó la campana mientras no estaba visible.
    needs_attention: bool,
    _subscription: Subscription,
}

pub struct Workspace {
    tabs: Vec<TabEntry>,
    active: usize,
    should_move: bool,
    shell_name: String,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shell_name = std::env::var("SHELL")
            .ok()
            .and_then(|s| s.rsplit('/').next().map(str::to_string))
            .unwrap_or_else(|| "shell".into());
        let mut this = Self {
            tabs: Vec::new(),
            active: 0,
            should_move: false,
            shell_name,
        };
        this.add_tab(window, cx);
        this
    }

    fn add_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let builder = match TerminalBuilder::new() {
            Ok(builder) => builder,
            Err(err) => {
                eprintln!("no se pudo abrir la terminal: {err:#}");
                return;
            }
        };
        let terminal = cx.new(|cx| builder.build(cx));
        let subscription = cx.subscribe_in(&terminal, window, Self::on_terminal_event);
        let view = cx.new(|cx| TerminalView::new(terminal, cx));
        self.tabs.push(TabEntry {
            view,
            needs_attention: false,
            _subscription: subscription,
        });
        self.activate(self.tabs.len() - 1, window, cx);
    }

    fn on_terminal_event(
        &mut self,
        terminal: &Entity<Terminal>,
        event: &TerminalEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self
            .tabs
            .iter()
            .position(|tab| tab.view.read(cx).terminal() == terminal)
        else {
            return;
        };
        match event {
            TerminalEvent::TitleChanged => cx.notify(),
            TerminalEvent::Bell => {
                if ix != self.active {
                    self.tabs[ix].needs_attention = true;
                    cx.notify();
                }
            }
            TerminalEvent::Exited => self.close(ix, window, cx),
        }
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(ix) else {
            return;
        };
        tab.needs_attention = false;
        self.active = ix;
        tab.view.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn close(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        self.tabs.remove(ix);
        if self.tabs.is_empty() {
            window.remove_window();
            return;
        }
        let next = if self.active > ix || self.active >= self.tabs.len() {
            self.active.saturating_sub(1)
        } else {
            self.active
        };
        self.activate(next, window, cx);
    }

    /// El comando en ejecución, si no la carpeta actual, si no el título que puso la shell.
    fn tab_title(&self, ix: usize, cx: &App) -> SharedString {
        let terminal = self.tabs[ix].view.read(cx).terminal().read(cx);
        let shell = terminal.shell();
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

    fn is_running(&self, ix: usize, cx: &App) -> bool {
        self.tabs[ix]
            .view
            .read(cx)
            .terminal()
            .read(cx)
            .shell()
            .running
            .is_some()
    }

    fn render_tab(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = ix == self.active;
        let last = self.tabs.len() - 1;
        let position = if ix == 0 {
            TabPosition::First
        } else if ix == last {
            TabPosition::Last
        } else {
            TabPosition::Middle(ix.cmp(&self.active))
        };
        let attention = self.tabs[ix].needs_attention;

        let start = if attention {
            div()
                .size(px(7.))
                .rounded_full()
                .bg(palette::hsla(palette::WARNING))
                .into_any_element()
        } else if self.is_running(ix, cx) {
            div()
                .size(px(7.))
                .rounded_full()
                .bg(palette::hsla(palette::SUCCESS))
                .border_2()
                .border_color(palette::hsla(palette::SUCCESS).opacity(0.2))
                .into_any_element()
        } else {
            Icon::new(IconName::Terminal)
                .size(IconSize::XSmall)
                .color(if selected { Color::Accent } else { Color::Muted })
                .into_any_element()
        };

        Tab::new(("tab", ix))
            .position(position)
            .close_side(TabCloseSide::End)
            .toggle_state(selected)
            .when(!selected, |tab| {
                tab.hover(|style| style.bg(palette::hsla(palette::ELEMENT_HOVER)))
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.activate(ix, window, cx)
            }))
            .start_slot(start)
            .end_slot(
                IconButton::new(("close-tab", ix), IconName::Close)
                    .icon_size(IconSize::XSmall)
                    .icon_color(Color::Muted)
                    .tooltip(Tooltip::for_action_title("Cerrar pestaña", &CloseTab))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.close(ix, window, cx)
                    })),
            )
            .child(
                h_flex()
                    .gap_2()
                    .pl_1()
                    .max_w(px(220.))
                    .child(
                        Label::new(self.tab_title(ix, cx))
                            .size(LabelSize::Small)
                            .color(if selected { Color::Default } else { Color::Muted })
                            .truncate(),
                    )
                    .when(ix < 9, |this| {
                        this.child(
                            Label::new(format!("⌘{}", ix + 1))
                                .size(LabelSize::XSmall)
                                .color(Color::Placeholder),
                        )
                    }),
            )
            .when(selected, |tab| {
                tab.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .bg(palette::hsla(palette::ACCENT)),
                )
            })
    }

    /// Zona arrastrable (barra de título transparente en macOS).
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

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = (0..self.tabs.len())
            .map(|ix| self.render_tab(ix, cx).into_any_element())
            .collect::<Vec<_>>();
        let border = palette::hsla(palette::BORDER_VARIANT);

        h_flex()
            .w_full()
            .flex_none()
            .bg(palette::hsla(palette::CHROME))
            // Espacio para los semáforos de macOS.
            .child(
                self.drag_region("traffic-lights", cx)
                    .w(px(78.))
                    .flex_none()
                    .border_b_1()
                    .border_color(border),
            )
            .child(
                TabBar::new("tab-bar")
                    .children(tabs)
                    .child(
                        self.drag_region("tab-bar-drag", cx)
                            .flex_1()
                            .min_w(px(24.))
                            .border_b_1()
                            .border_color(border),
                    )
                    .end_child(
                        IconButton::new("new-tab", IconName::Plus)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::for_action_title("Nueva pestaña", &NewTab))
                            .on_click(|_, window, cx| window.dispatch_action(NewTab.boxed_clone(), cx)),
                    ),
            )
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let terminal = self.tabs[self.active].view.read(cx).terminal().read(cx);
        let size = terminal.size();
        let shell = terminal.shell();
        let tab_count = self.tabs.len();

        let (dot, last) = match (&shell.running, &shell.last) {
            (Some(running), _) => (
                palette::WARNING,
                Some((
                    format!("● {}", format_duration(running.started.elapsed())),
                    Color::Warning,
                )),
            ),
            (None, Some(last)) if last.exit_code.unwrap_or(0) != 0 => (
                palette::ERROR,
                Some((
                    format!(
                        "✕ exit {} · {}",
                        last.exit_code.unwrap_or_default(),
                        format_duration(last.duration)
                    ),
                    Color::Error,
                )),
            ),
            (None, Some(last)) => (
                palette::SUCCESS,
                Some((format!("✓ {}", format_duration(last.duration)), Color::Success)),
            ),
            (None, None) => (palette::SUCCESS, None),
        };

        h_flex()
            .h(px(26.))
            .flex_none()
            .px_3()
            .gap_4()
            .justify_between()
            .overflow_hidden()
            .bg(palette::hsla(palette::CHROME))
            .border_t_1()
            .border_color(palette::hsla(palette::BORDER_VARIANT))
            .child(
                h_flex()
                    .gap_3()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(div().size(px(6.)).rounded_full().bg(palette::hsla(dot)))
                            .child(
                                Label::new(self.shell_name.clone())
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                    )
                    .when_some(shell.cwd.as_ref(), |this, cwd| {
                        this.child(
                            Label::new(abbreviate_home(cwd))
                                .size(LabelSize::Small)
                                .color(Color::Placeholder)
                                .truncate(),
                        )
                    })
                    .when_some(shell.git_branch.clone(), |this, branch| {
                        this.child(
                            h_flex()
                                .gap_1()
                                .child(
                                    Icon::new(IconName::GitBranch)
                                        .size(IconSize::XSmall)
                                        .color(Color::Custom(palette::hsla(palette::SYNTAX_SUBCOMMAND))),
                                )
                                .child(
                                    Label::new(branch)
                                        .size(LabelSize::Small)
                                        .color(Color::Custom(palette::hsla(palette::SYNTAX_SUBCOMMAND))),
                                ),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_4()
                    .flex_none()
                    .font_family(MONO_FONT_FAMILY)
                    .when_some(last, |this, (text, color)| {
                        this.child(Label::new(text).size(LabelSize::XSmall).color(color))
                    })
                    .child(
                        Label::new(if tab_count == 1 {
                            "1 pestaña".to_string()
                        } else {
                            format!("{tab_count} pestañas")
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
    }
}

fn short_command(command: &str) -> String {
    let words: Vec<&str> = command.split_whitespace().take(3).collect();
    let mut text = words.join(" ");
    if text.chars().count() > 28 {
        text = text.chars().take(27).collect::<String>() + "…";
    }
    text
}

fn dir_name(path: &Path) -> String {
    if std::env::var_os("HOME").is_some_and(|home| Path::new(&home) == path) {
        return "~".into();
    }
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn abbreviate_home(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME")
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".into()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.display().to_string()
}

fn format_duration(duration: Duration) -> String {
    let ms = duration.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", duration.as_secs_f32())
    } else {
        format!("{}m {}s", ms / 60_000, (ms / 1000) % 60)
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.tabs[self.active].view.focus_handle(cx)
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.tabs.get(self.active).map(|tab| tab.view.clone());

        v_flex()
            .size_full()
            .key_context("Workspace")
            .bg(palette::hsla(palette::BACKGROUND))
            .text_color(palette::hsla(palette::TEXT))
            .font_family(UI_FONT_FAMILY)
            .text_size(UI_FONT_SIZE)
            .on_action(cx.listener(|this, _: &NewTab, window, cx| this.add_tab(window, cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                this.close(this.active, window, cx)
            }))
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
            .child(self.render_tab_bar(cx))
            .child(div().flex_1().min_h_0().children(active))
            .when(!self.tabs.is_empty(), |this| this.child(self.render_status_bar(cx)))
    }
}
