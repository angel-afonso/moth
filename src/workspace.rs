//! Ventana: barra de pestañas, terminal activa y barra de estado, con componentes del crate
//! `ui` de Zed (Label, HighlightedLabel, Icon, IconButton, Button, KeyBinding, Tooltip), más
//! los overlays de búsqueda: paleta de comandos, historial y ramas.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::{
    Action, App, ClickEvent, Context, Entity, FocusHandle, Focusable, KeyBinding,
    MouseButton, MouseDownEvent, Pixels, Subscription, Window, actions, div, prelude::*, px,
};
use ui::{
    Button, ButtonSize, ButtonStyle, Color, Icon, IconButton, IconName, IconSize, Label,
    LabelCommon, LabelSize, Tooltip, h_flex, prelude::*, v_flex,
};

use crate::{
    components::{BlockStatus, DotState, StatusDot, SuggestionKind, TabItem, TabState},
    hosts::{self, RulesSource},
    ssh,
    picker::{self, Picker, PickerAction, PickerConfig, PickerEvent, PickerItem},
    shell, state,
    terminal::{Terminal, TerminalBuilder, TerminalEvent},
    terminal_view::{self, TerminalView},
    theme::{self, Themes},
    tokens::{Appearance, Tokens, TokensExt, metrics, tint},
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
        SelectTheme,
        ConnectToHost,
        Reconnect,
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
        KeyBinding::new("cmd-shift-r", Reconnect, None),
    ]);
    hosts::reload(cx);
    crate::completions::init(cx);
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
    Themes,
    Hosts,
}

struct Modal {
    kind: ModalKind,
    picker: Entity<Picker>,
    /// Tema a restaurar si se cierra el selector sin elegir (por la vista previa).
    restore_theme: Option<Arc<Tokens>>,
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
        // En SSH, el host; si se cortó, el host también (tachado en la pestaña).
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
        let state = if self.tabs[ix].needs_attention {
            TabState::Attention
        } else if self.is_running(ix, cx) {
            TabState::Running
        } else {
            TabState::Idle
        };
        let shell = self.tabs[ix].view.read(cx).terminal().read(cx).shell();
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
        let disconnected = shell.remote.is_none() && shell.running.is_none() && shell.disconnected.is_some();
        TabItem::new(("tab", ix), self.tab_title(ix, cx))
            .selected(ix == self.active)
            .state(state)
            .when_some(remote, |tab, (color, label)| tab.remote(color, label.map(Into::into)))
            .when(disconnected, |tab| {
                tab.disconnected(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.reconnect_tab(ix, window, cx)
                }))
            })
            .when(ix < 9, |tab| tab.shortcut(format!("⌘{}", ix + 1)))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.activate(ix, window, cx)
            }))
            .on_close(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.close(ix, window, cx)
            }))
            .on_middle_click(cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.close(ix, window, cx)
            }))
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
        let ui = cx.tokens().ui.clone();
        let border = ui.border;

        h_flex()
            .h(TAB_BAR_HEIGHT)
            .w_full()
            .flex_none()
            .bg(ui.chrome)
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

    fn render_status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(remote) = self.render_remote_status_bar(cx) {
            return remote;
        }
        let git = self.render_git(cx);
        let ui = cx.tokens().ui.clone();
        let terminal = self.tabs[self.active].view.read(cx).terminal().read(cx);
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
                let dot = if exit_code == 0 { DotState::Success } else { DotState::Error };
                (dot, Some(BlockStatus::Finished { exit_code, duration: last.duration }))
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
            .into_any_element()
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

/// `~` en lugar del home y, si la ruta es larga, solo las últimas tres carpetas (como el prompt).
fn abbreviate_home(path: &Path) -> String {
    let (prefix, rest) = match std::env::var_os("HOME")
        .and_then(|home| path.strip_prefix(&home).ok().map(Path::to_path_buf))
    {
        Some(rest) if rest.as_os_str().is_empty() => return "~".into(),
        Some(rest) => ("~/", rest),
        None => ("/", path.strip_prefix("/").unwrap_or(path).to_path_buf()),
    };
    let parts: Vec<_> = rest.components().map(|c| c.as_os_str().to_string_lossy()).collect();
    if parts.len() > 3 {
        format!("…/{}", parts[parts.len() - 3..].join("/"))
    } else {
        format!("{prefix}{}", parts.join("/"))
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
        let (action, secondary) = match event {
            PickerEvent::Selected(PickerAction::SetTheme(name)) => {
                // Vista previa: se aplica sin guardar.
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
        if self.active_terminal(cx).read(cx).shell().disconnected.is_some() {
            items.push(
                PickerItem::new("Acciones", "Reconectar", PickerAction::Dispatch(Box::new(Reconnect)))
                    .icon(IconName::RotateCw)
                    .shortcut(Reconnect),
            );
        }
        if has_git {
            items.push(
                PickerItem::new("Acciones", "Cambiar de rama…", PickerAction::Dispatch(Box::new(SwitchBranch)))
                    .icon(IconName::GitBranch),
            );
        }
        items.extend([
            PickerItem::new("Acciones", "Conectar a…", PickerAction::Dispatch(Box::new(ConnectToHost)))
                .icon(IconName::Server),
            PickerItem::new("Acciones", "Cambiar tema…", PickerAction::Dispatch(Box::new(SelectTheme)))
                .icon(IconName::Sparkle),
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
                    .kind(SuggestionKind::Path)
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
            selected_item: None,
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
                            .kind(SuggestionKind::History)
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
            selected_item: None,
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
                    .kind(SuggestionKind::Branch);
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
            selected_item: None,
        };
        self.open_modal(ModalKind::Branches, items, config, window, cx);
    }

    fn select_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Se releen por si el usuario agregó o editó un tema desde la última vez.
        let themes = theme::reload_themes(cx);
        let current = cx.tokens().name.clone();
        let items = themes
            .iter()
            .map(|t| {
                let appearance = match t.appearance {
                    Appearance::Dark => "oscuro",
                    Appearance::Light => "claro",
                };
                let mut item = PickerItem::new("", t.name.clone(), PickerAction::SetTheme(t.name.clone()))
                    .icon(if t.name == current { IconName::Check } else { IconName::Sparkle })
                    .detail(appearance);
                if t.name == current {
                    item = item.detail(format!("{appearance} · actual"));
                }
                item
            })
            .collect();
        let config = PickerConfig {
            placeholder: "Elegir tema…".into(),
            query: String::new(),
            empty_text: "Ningún tema coincide".into(),
            hints: vec![
                ("↑↓".into(), "vista previa".into()),
                ("⏎".into(), "usar".into()),
                ("esc".into(), "volver al anterior".into()),
            ],
            disabled_reason: None,
            selected_item: themes.iter().position(|t| t.name == current),
        };
        self.open_modal(ModalKind::Themes, items, config, window, cx);
    }

    fn active_is_production(&self, cx: &App) -> bool {
        self.tabs
            .get(self.active)
            .and_then(|tab| tab.view.read(cx).terminal().read(cx).shell().remote.as_ref().map(|r| r.identity.production))
            .unwrap_or(false)
    }

    fn reconnect_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        let terminal = tab.view.read(cx).terminal().clone();
        if terminal.update(cx, |t, cx| t.reconnect(cx)) {
            self.activate(ix, window, cx);
        }
    }

    /// Franja bajo las pestañas cuando la sesión activa es de producción.
    fn render_production_strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let terminal = self.active_terminal(cx);
        let terminal = terminal.read(cx);
        let remote = terminal.shell().remote.as_ref().filter(|r| r.identity.production)?;
        let color = remote.identity.color(cx);
        let label = remote
            .identity
            .label
            .clone()
            .map(|l| if l.eq_ignore_ascii_case("prod") { "PRODUCCIÓN".to_string() } else { l.to_uppercase() })
            .unwrap_or_else(|| "PRODUCCIÓN".into());
        let reason = match (&remote.identity.matched, &hosts::rules(cx).source) {
            (Some(pattern), RulesSource::File(_)) => format!("· coincide con {pattern} en hosts.json"),
            (Some(pattern), RulesSource::Defaults) => format!("· coincide con {pattern} (regla por defecto)"),
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
                        .child(Icon::new(IconName::Warning).size(IconSize::Small).color(Color::Custom(color)))
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
                        .child(Label::new(reason).size(LabelSize::Small).color(Color::Muted).truncate()),
                )
                .child(
                    h_flex()
                        .gap(px(6.))
                        .flex_none()
                        .child(Icon::new(IconName::Lock).size(IconSize::XSmall).color(Color::Muted))
                        .child(Label::new("Pegado protegido").size(LabelSize::Small).color(Color::Muted)),
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
            "conectado · recién".to_string()
        } else if minutes < 60 {
            format!("conectado · {minutes} min")
        } else {
            format!("conectado · {}h {}min", minutes / 60, minutes % 60)
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
                                .child(Label::new("ssh").size(LabelSize::Small).color(Color::Muted)),
                        )
                        .child(
                            h_flex()
                                .gap(px(6.))
                                .flex_none()
                                .child(Icon::new(IconName::ArrowRightLeft).size(IconSize::XSmall).color(Color::Custom(color)))
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
                                    Label::new(address).size(LabelSize::XSmall).color(Color::Placeholder),
                                ),
                            )
                        })
                        .when_some(remote.remote_cwd.clone(), |this, cwd| {
                            this.child(
                                div().min_w_0().child(
                                    Label::new(cwd).size(LabelSize::Small).color(Color::Placeholder).truncate(),
                                ),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .gap(metrics::SPACE_4)
                        .flex_none()
                        .font_family(MONO_FONT_FAMILY)
                        .child(Label::new(connected).size(LabelSize::XSmall).color(Color::Placeholder))
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
                .into_any_element(),
        )
    }

    fn connect_to_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        hosts::reload(cx);
        let rules = hosts::rules(cx);
        let terminal_colors = cx.tokens().terminal.clone();
        let identity_parts = |alias: &str, hostname: Option<&str>| {
            let identity = rules.identify(alias, hostname);
            (identity.color.resolve(&terminal_colors), identity.label)
        };
        let mut items = Vec::new();

        // Sesiones cortadas, de cualquier pestaña.
        for (ix, tab) in self.tabs.iter().enumerate() {
            let shell = tab.view.read(cx).terminal().read(cx).shell();
            let Some(d) = shell.disconnected.as_ref().filter(|_| shell.running.is_none()) else {
                continue;
            };
            let minutes = d.at.elapsed().as_secs() / 60;
            let ago = if minutes == 0 { "recién".to_string() } else { format!("hace {minutes} min") };
            let mut item = PickerItem::new("Reconectar", d.target.alias.clone(), PickerAction::ReconnectTab(ix))
                .icon(IconName::RotateCw)
                .detail(format!("se cortó {ago} · exit {}", d.exit_code.unwrap_or(255)))
                .mono();
            if let Some(label) = d.identity.label.clone() {
                item = item.chip(label, d.identity.color(cx));
            }
            if ix == self.active {
                item.shortcut = Some(Box::new(Reconnect));
            }
            items.push(item);
        }

        // Pestañas con sesión abierta, para avisarlo en cada host.
        let open_tabs: Vec<(usize, String)> = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(ix, tab)| {
                let shell = tab.view.read(cx).terminal().read(cx).shell();
                shell.remote.as_ref().map(|r| (ix, r.target.alias.clone()))
            })
            .collect();

        let config_hosts = ssh::config_hosts();
        for host in &config_hosts {
            let (color, label) = identity_parts(&host.alias, host.hostname.as_deref());
            let open = open_tabs
                .iter()
                .find(|(_, alias)| alias == &host.alias)
                .map(|(ix, _)| format!("abierta en ⌘{} · ", ix + 1))
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

        // `ssh …` del historial a destinos que no están en el config.
        let mut seen: Vec<String> = config_hosts.iter().map(|h| h.alias.clone()).collect();
        if let Some(history) = shell::history(cx) {
            for command in history.entries.iter().rev() {
                let Some(target) = ssh::parse_ssh_command(command).filter(|t| !t.remote_command) else {
                    continue;
                };
                let key = command.trim().to_string();
                if seen.contains(&key) || seen.contains(&target.alias) && command.trim() == format!("ssh {}", target.alias) {
                    continue;
                }
                seen.push(key);
                let (color, label) = identity_parts(&target.alias, None);
                let mut item = PickerItem::new("Del historial", command.clone(), PickerAction::RunCommand(command.clone()))
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
            placeholder: "Conectar a… alias, usuario@host o IP".into(),
            query: String::new(),
            empty_text: if items.is_empty() {
                "No hay hosts en ~/.ssh/config ni en el historial".into()
            } else {
                "Ningún host coincide".into()
            },
            hints: vec![
                ("↑↓".into(), "navegar".into()),
                ("⏎".into(), "abrir en pestaña nueva".into()),
                ("⌘⏎".into(), if at_prompt { "en esta pestaña".into() } else { "en esta pestaña (no disponible)".into() }),
                ("esc".into(), "cerrar".into()),
            ],
            disabled_reason: None,
            selected_item: None,
        };
        self.open_modal(ModalKind::Hosts, items, config, window, cx);
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
            .bg(cx.tokens().ui.background)
            .text_color(cx.tokens().ui.text)
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
            .on_action(cx.listener(|this, _: &SelectTheme, w, cx| this.select_theme(w, cx)))
            .on_action(cx.listener(|this, _: &ClearScreen, _, cx| {
                this.active_terminal(cx).update(cx, |t, cx| t.clear(cx))
            }))
            .on_action(cx.listener(|this, _: &ConnectToHost, w, cx| this.connect_to_host(w, cx)))
            .on_action(cx.listener(|this, _: &Reconnect, w, cx| {
                let active = this.active;
                this.reconnect_tab(active, w, cx)
            }))
            .child(self.render_tab_bar(cx))
            .children(self.render_production_strip(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(self.active_is_production(cx), |el| {
                        el.p(px(8.)).bg(cx.tokens().terminal.background)
                    })
                    .children(active),
            )
            .when(!self.tabs.is_empty(), |this| this.child(self.render_status_bar(cx)))
            .children(self.render_modal(cx))
    }
}
