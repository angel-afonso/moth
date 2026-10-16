//! Ventana: barra de pestañas, terminal activa y barra de estado, con componentes del crate
//! `ui` de Zed (Label, Icon, IconButton, Button, PopoverMenu, ContextMenu, Tooltip).

use std::{path::Path, time::Duration};

use gpui::{
    Action, Anchor, App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight,
    KeyBinding, MouseButton, Pixels, Subscription, Window, actions, div, prelude::*, px,
};
use ui::{
    Button, ButtonSize, ButtonStyle, Color, ContextMenu, Icon, IconButton, IconName,
    IconPosition, IconSize, Label, LabelCommon, LabelSize, PopoverMenu, Tooltip, h_flex,
    prelude::*, v_flex,
};

use crate::{
    palette, state,
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
        // Navegación de los menús de `ui` (ContextMenu usa el contexto "menu").
        KeyBinding::new("up", menu::SelectPrevious, Some("menu")),
        KeyBinding::new("down", menu::SelectNext, Some("menu")),
        KeyBinding::new("ctrl-p", menu::SelectPrevious, Some("menu")),
        KeyBinding::new("ctrl-n", menu::SelectNext, Some("menu")),
        KeyBinding::new("home", menu::SelectFirst, Some("menu")),
        KeyBinding::new("end", menu::SelectLast, Some("menu")),
        KeyBinding::new("enter", menu::Confirm, Some("menu")),
        KeyBinding::new("escape", menu::Cancel, Some("menu")),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
}

/// Alto de la barra de pestañas; los semáforos de macOS se centran en ella (ver main.rs).
pub const TAB_BAR_HEIGHT: Pixels = px(40.);

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

    /// Una pestaña nueva abre en la carpeta de la pestaña activa; la primera, en la última
    /// carpeta usada en la ejecución anterior.
    fn add_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self
            .tabs
            .get(self.active)
            .and_then(|tab| tab.view.read(cx).terminal().read(cx).shell().cwd.clone())
            .or_else(state::last_cwd);
        let builder = match TerminalBuilder::new(cwd) {
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
        let attention = self.tabs[ix].needs_attention;
        let border = palette::hsla(palette::BORDER_VARIANT);

        let indicator = if attention {
            div()
                .size(px(7.))
                .rounded_full()
                .bg(palette::hsla(palette::WARNING))
                .into_any_element()
        } else if self.is_running(ix, cx) {
            div()
                .size(px(13.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(palette::hsla(palette::SUCCESS).opacity(0.15))
                .child(div().size(px(7.)).rounded_full().bg(palette::hsla(palette::SUCCESS)))
                .into_any_element()
        } else {
            Icon::new(IconName::Terminal)
                .size(IconSize::Small)
                .color(if selected { Color::Accent } else { Color::Placeholder })
                .into_any_element()
        };

        h_flex()
            .id(("tab", ix))
            .group("tab")
            .relative()
            .h_full()
            .flex_none()
            .pl(px(14.))
            .pr(px(6.))
            .gap(px(8.))
            .border_r_1()
            .border_color(border)
            .cursor_pointer()
            .map(|tab| {
                if selected {
                    // La pestaña activa se funde con la terminal: mismo fondo, sin borde inferior.
                    tab.bg(palette::hsla(palette::BACKGROUND)).child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h(px(2.))
                            .bg(palette::hsla(palette::ACCENT)),
                    )
                } else {
                    tab.border_b_1()
                        .hover(|style| style.bg(palette::hsla(palette::ELEMENT_HOVER)))
                }
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.activate(ix, window, cx)
            }))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, window, cx| this.close(ix, window, cx)),
            )
            .child(div().size(px(14.)).flex().items_center().justify_center().child(indicator))
            .child(
                div().max_w(px(200.)).child(
                    Label::new(self.tab_title(ix, cx))
                        .color(if selected { Color::Default } else { Color::Muted })
                        .when(selected, |label| label.weight(FontWeight::MEDIUM))
                        .truncate(),
                ),
            )
            .when(ix < 9, |tab| {
                tab.child(
                    div()
                        .font_family(MONO_FONT_FAMILY)
                        .text_size(px(11.))
                        .text_color(palette::hsla(palette::TEXT_PLACEHOLDER))
                        .child(format!("⌘{}", ix + 1)),
                )
            })
            .child(
                IconButton::new(("close-tab", ix), IconName::Close)
                    .icon_size(IconSize::XSmall)
                    .icon_color(if selected { Color::Muted } else { Color::Placeholder })
                    .size(ButtonSize::Medium)
                    .tooltip(Tooltip::for_action_title("Cerrar pestaña", &CloseTab))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.close(ix, window, cx)
                    })),
            )
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
            .h(TAB_BAR_HEIGHT)
            .w_full()
            .flex_none()
            .bg(palette::hsla(palette::CHROME))
            // Espacio para los semáforos de macOS.
            .child(
                self.drag_region("traffic-lights", cx)
                    .w(px(80.))
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
                                    .tooltip(Tooltip::for_action_title("Nueva pestaña", &NewTab))
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
    }

    /// Rama actual con cambios y ahead/behind; al hacer clic, lista las ramas para cambiar.
    fn render_git(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = self.tabs[self.active].view.clone();
        let terminal = view.read(cx).terminal().clone();
        let git = terminal.read(cx).shell().git.clone()?;
        let branch_color = Color::Custom(palette::hsla(palette::SYNTAX_SUBCOMMAND));

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

        let menu = PopoverMenu::new("branch-menu")
            .anchor(Anchor::BottomLeft)
            .trigger_with_tooltip(
                Button::new("branch", git.display_name())
                    .style(ButtonStyle::Subtle)
                    .size(ButtonSize::Compact)
                    .label_size(LabelSize::Small)
                    .color(branch_color)
                    .start_icon(Icon::new(IconName::GitBranch).size(IconSize::XSmall).color(branch_color)),
                Tooltip::text("Cambiar de rama"),
            )
            .menu(move |window, cx| {
                let shell = terminal.read(cx);
                let at_prompt = shell.at_prompt();
                let git = shell.shell().git.clone()?;
                let terminal = terminal.clone();
                let view = view.clone();
                Some(ContextMenu::build(window, cx, move |mut menu, _, _| {
                    menu = menu.header(if at_prompt {
                        "Cambiar de rama"
                    } else {
                        "Termina el comando en curso para cambiar de rama"
                    });
                    if git.branches.is_empty() {
                        menu = menu.label("Sin ramas locales");
                    }
                    for name in &git.branches {
                        let current = git.branch.as_ref() == Some(name);
                        let branch = name.clone();
                        let terminal = terminal.clone();
                        let view = view.clone();
                        menu = menu.toggleable_entry_disabled_when(
                            name.clone(),
                            current,
                            !at_prompt || current,
                            IconPosition::Start,
                            None,
                            move |window, cx| {
                                terminal.update(cx, |t, cx| t.switch_branch(&branch, cx));
                                view.focus_handle(cx).focus(window, cx);
                            },
                        );
                    }
                    menu
                }))
            });

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

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let git = self.render_git(cx);
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
            .h(px(28.))
            .flex_none()
            .pl(px(14.))
            .pr(px(14.))
            .gap_4()
            .justify_between()
            .overflow_hidden()
            .bg(palette::hsla(palette::CHROME))
            .border_t_1()
            .border_color(palette::hsla(palette::BORDER_VARIANT))
            .child(
                h_flex()
                    .gap_4()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .flex_none()
                            .child(div().size(px(6.)).rounded_full().bg(palette::hsla(dot)))
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
