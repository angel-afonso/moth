//! Ventana: barra de pestañas, terminal activa y barra de estado, con componentes del crate
//! `ui` de Zed (Label, HighlightedLabel, Icon, IconButton, Button, KeyBinding, Tooltip), más
//! los overlays de búsqueda: paleta de comandos, historial y ramas.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use gpui::{
    Action, App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, KeyBinding,
    MouseButton, Pixels, Subscription, Window, actions, div, prelude::*, px, rgba,
};
use ui::{
    Button, ButtonSize, ButtonStyle, Color, Icon, IconButton, IconName, IconSize, Label,
    LabelCommon, LabelSize, Tooltip, h_flex, prelude::*, v_flex,
};

use crate::{
    palette,
    picker::{self, Picker, PickerAction, PickerConfig, PickerEvent, PickerItem},
    shell, state,
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
        ToggleCommandPalette,
        SearchHistory,
        SwitchBranch,
        ClearScreen,
        Quit,
    ]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        // El último atajo de cada acción es el que se muestra en menús y botones.
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
        KeyBinding::new("cmd-}", NextTab, None),
        KeyBinding::new("cmd-{", PreviousTab, None),
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
        KeyBinding::new("cmd-shift-p", ToggleCommandPalette, None),
        KeyBinding::new("cmd-k", ToggleCommandPalette, None),
        KeyBinding::new("cmd-r", SearchHistory, None),
        KeyBinding::new("cmd-l", ClearScreen, None),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    picker::init(cx);
}

/// Alto de la barra de pestañas; los semáforos de macOS se centran en ella (ver main.rs).
pub const TAB_BAR_HEIGHT: Pixels = px(40.);

struct TabEntry {
    view: Entity<TerminalView>,
    /// La sesión sonó la campana mientras no estaba visible.
    needs_attention: bool,
    _subscription: Subscription,
}

#[derive(Clone, Copy, PartialEq)]
enum ModalKind {
    CommandPalette,
    History,
    Branches,
}

struct Modal {
    kind: ModalKind,
    picker: Entity<Picker>,
    _subscription: Subscription,
}

pub struct Workspace {
    tabs: Vec<TabEntry>,
    active: usize,
    should_move: bool,
    shell_name: String,
    modal: Option<Modal>,
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
            modal: None,
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
        self.add_tab_in(cwd, window, cx);
    }

    fn add_tab_in(&mut self, cwd: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
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
            .child(
                h_flex()
                    .h_full()
                    .flex_none()
                    .px(px(10.))
                    .border_b_1()
                    .border_color(border)
                    .child(
                        Button::new("command-palette", "Comandos")
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

    /// Rama actual con cambios y ahead/behind; al hacer clic, lista las ramas para cambiar.
    fn render_git(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let terminal = self.tabs[self.active].view.read(cx).terminal().clone();
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

        let menu = Button::new("branch", git.display_name())
            .style(ButtonStyle::Subtle)
            .size(ButtonSize::Compact)
            .label_size(LabelSize::Small)
            .color(branch_color)
            .start_icon(Icon::new(IconName::GitBranch).size(IconSize::XSmall).color(branch_color))
            .tooltip(Tooltip::for_action_title("Cambiar de rama", &SwitchBranch))
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

impl Workspace {
    fn active_terminal(&self, cx: &App) -> Entity<Terminal> {
        self.tabs[self.active].view.read(cx).terminal().clone()
    }

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
            _subscription: subscription,
        });
        cx.notify();
    }

    fn dismiss_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.take().is_some() {
            self.tabs[self.active].view.focus_handle(cx).focus(window, cx);
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
        let PickerEvent::Confirm { action, secondary } = event else {
            self.dismiss_modal(window, cx);
            return;
        };
        let (action, secondary) = (action.clone(), *secondary);
        self.dismiss_modal(window, cx);
        let terminal = self.active_terminal(cx);
        match action {
            // Se despacha después de devolver el foco a la terminal, para que llegue a ella.
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
        }
    }

    fn toggle_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = self.active_terminal(cx);
        let shell = terminal.read(cx).shell();
        let has_git = shell.git.is_some();
        let cwd = shell.cwd.clone();

        let mut items = vec![
            PickerItem::new("Acciones", "Nueva pestaña", PickerAction::Dispatch(Box::new(NewTab)))
                .icon(IconName::Plus)
                .shortcut(NewTab),
            PickerItem::new("Acciones", "Buscar en el historial", PickerAction::Dispatch(Box::new(SearchHistory)))
                .icon(IconName::HistoryRerun)
                .shortcut(SearchHistory),
        ];
        if has_git {
            items.push(
                PickerItem::new("Acciones", "Cambiar de rama…", PickerAction::Dispatch(Box::new(SwitchBranch)))
                    .icon(IconName::GitBranch),
            );
        }
        items.extend([
            PickerItem::new("Acciones", "Limpiar pantalla", PickerAction::Dispatch(Box::new(ClearScreen)))
                .icon(IconName::Eraser)
                .shortcut(ClearScreen),
            PickerItem::new("Acciones", "Copiar pantalla", PickerAction::Dispatch(Box::new(terminal_view::Copy)))
                .icon(IconName::Copy),
            PickerItem::new("Acciones", "Pegar", PickerAction::Dispatch(Box::new(terminal_view::Paste)))
                .icon(IconName::Download),
            PickerItem::new("Acciones", "Pestaña siguiente", PickerAction::Dispatch(Box::new(NextTab)))
                .icon(IconName::ArrowRight)
                .shortcut(NextTab),
            PickerItem::new("Acciones", "Pestaña anterior", PickerAction::Dispatch(Box::new(PreviousTab)))
                .icon(IconName::ArrowLeft)
                .shortcut(PreviousTab),
            PickerItem::new("Acciones", "Cerrar pestaña", PickerAction::Dispatch(Box::new(CloseTab)))
                .icon(IconName::Close)
                .shortcut(CloseTab),
            PickerItem::new("Acciones", "Salir de terminalapp", PickerAction::Dispatch(Box::new(Quit)))
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
            let tab_terminal = self.tabs[ix].view.read(cx).terminal().read(cx);
            let mut item = PickerItem::new(
                "Pestañas abiertas",
                format!("Ir a {}", self.tab_title(ix, cx)),
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
                PickerItem::new("Carpetas recientes", abbreviate_home(&dir), PickerAction::OpenDirInNewTab(dir))
                    .icon(IconName::Folder)
                    .detail("abrir en pestaña nueva")
                    .mono(),
            );
        }

        let config = PickerConfig {
            placeholder: "Buscar comandos, pestañas o carpetas…".into(),
            query: String::new(),
            empty_text: "Nada coincide con la búsqueda".into(),
            hints: vec![
                ("↑↓".into(), "navegar".into()),
                ("⏎".into(), "ejecutar".into()),
                ("esc".into(), "cerrar".into()),
            ],
            disabled_reason: None,
        };
        self.open_modal(ModalKind::CommandPalette, items, config, window, cx);
    }

    fn search_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = self.active_terminal(cx);
        let terminal = terminal.read(cx);
        // Arranca con lo que ya estaba escrito en el prompt, como ⌃R.
        let query = terminal
            .shell()
            .line
            .as_ref()
            .map(|line| line.buffer.clone())
            .unwrap_or_default();
        let items: Vec<PickerItem> = shell::history(cx)
            .map(|history| {
                history
                    .entries
                    .iter()
                    .rev()
                    .map(|command| {
                        PickerItem::new("", command.clone(), PickerAction::InsertCommand(command.clone()))
                            .mono()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let config = PickerConfig {
            placeholder: "Buscar en el historial…".into(),
            query,
            empty_text: if items.is_empty() {
                "El historial todavía no se cargó o está vacío".into()
            } else {
                "Ningún comando coincide".into()
            },
            hints: vec![
                ("↑↓".into(), "navegar".into()),
                ("⏎".into(), "llevar al prompt".into()),
                ("⌘⏎".into(), "ejecutar".into()),
                ("esc".into(), "cerrar".into()),
            ],
            disabled_reason: (!terminal.at_prompt())
                .then(|| "Hay un comando en ejecución: termina para usar el historial".into()),
        };
        self.open_modal(ModalKind::History, items, config, window, cx);
    }

    fn switch_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                let item = PickerItem::new("", branch.clone(), PickerAction::SwitchBranch(branch.clone()))
                    .icon(if current { IconName::Check } else { IconName::GitBranch });
                if current { item.detail("actual") } else { item }
            })
            .collect();
        let config = PickerConfig {
            placeholder: "Cambiar a la rama…".into(),
            query: String::new(),
            empty_text: "Ninguna rama coincide".into(),
            hints: vec![
                ("↑↓".into(), "navegar".into()),
                ("⏎".into(), "git switch".into()),
                ("esc".into(), "cerrar".into()),
            ],
            disabled_reason: (!terminal.at_prompt())
                .then(|| "Hay un comando en ejecución: termina para cambiar de rama".into()),
        };
        self.open_modal(ModalKind::Branches, items, config, window, cx);
    }

    fn render_modal(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let picker = self.modal.as_ref()?.picker.clone();
        Some(
            div()
                .absolute()
                .top(TAB_BAR_HEIGHT)
                .left_0()
                .right_0()
                .bottom_0()
                .bg(rgba(0x0506089e))
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

impl Focusable for Workspace {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.tabs[self.active].view.focus_handle(cx)
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.tabs.get(self.active).map(|tab| tab.view.clone());

        v_flex()
            .relative()
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
            .on_action(cx.listener(|this, _: &ToggleCommandPalette, w, cx| {
                this.toggle_command_palette(w, cx)
            }))
            .on_action(cx.listener(|this, _: &SearchHistory, w, cx| this.search_history(w, cx)))
            .on_action(cx.listener(|this, _: &SwitchBranch, w, cx| this.switch_branch(w, cx)))
            .on_action(cx.listener(|this, _: &ClearScreen, _, cx| {
                this.active_terminal(cx).update(cx, |t, cx| t.clear(cx))
            }))
            .child(self.render_tab_bar(cx))
            .child(div().flex_1().min_h_0().children(active))
            .when(!self.tabs.is_empty(), |this| this.child(self.render_status_bar(cx)))
            .children(self.render_modal(cx))
    }
}
