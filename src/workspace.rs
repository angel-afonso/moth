//! Ventana: barra de pestañas, terminal activa y barra de estado, con componentes del crate
//! `ui` de Zed (Label, HighlightedLabel, Icon, IconButton, Button, KeyBinding, Tooltip), más
//! los overlays de búsqueda: paleta de comandos, historial y ramas.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::{
    Action, App, Bounds, ClickEvent, Context, CursorStyle, DragMoveEvent, Entity, EntityId,
    FocusHandle, Focusable, Global, KeyBinding, MouseButton, MouseDownEvent, Pixels, SharedString,
    Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions, actions,
    deferred, div, point, prelude::*, px, relative, size,
};
use ui::{
    Button, ButtonSize, ButtonStyle, Color, Icon, IconButton, IconName, IconSize, Label,
    LabelCommon, LabelSize, Tooltip, h_flex, prelude::*, v_flex,
};

use crate::{
    components::{BlockStatus, DotState, StatusDot, SuggestionKind, TabItem, TabState},
    config,
    editor::{self, Launch},
    hosts::{self, RulesSource},
    panes::{Axis, Direction, PaneTree},
    picker::{self, Picker, PickerAction, PickerConfig, PickerEvent, PickerItem},
    session, shell, ssh, state,
    terminal::{Terminal, TerminalBuilder, TerminalEvent},
    terminal_view::{self, TerminalView},
    theme::{self, Themes},
    theme::{MONO_FONT_FAMILY, UI_FONT_FAMILY, UI_FONT_SIZE},
    tokens::{Appearance, Tokens, TokensExt, metrics, tint},
};

actions!(
    workspace,
    [
        NewTab,
        CloseTab,
        ReopenTab,
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
        OpenConfig,
        NewWindow,
        Minimize,
        ZoomWindow,
        SplitRight,
        SplitDown,
        FocusPaneLeft,
        FocusPaneRight,
        FocusPaneUp,
        FocusPaneDown,
        NextPane,
        PreviousPane,
        TogglePaneZoom,
        Quit,
    ]
);

/// Abrir un archivo de la salida en el editor (clic en una ruta).
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = workspace, no_json)]
pub struct OpenFile {
    pub file: crate::links::FileLink,
}

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-n", NewWindow, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("cmd-shift-t", ReopenTab, None),
        KeyBinding::new("cmd-d", SplitRight, None),
        KeyBinding::new("cmd-shift-d", SplitDown, None),
        KeyBinding::new("cmd-alt-left", FocusPaneLeft, None),
        KeyBinding::new("cmd-alt-right", FocusPaneRight, None),
        KeyBinding::new("cmd-alt-up", FocusPaneUp, None),
        KeyBinding::new("cmd-alt-down", FocusPaneDown, None),
        KeyBinding::new("cmd-]", NextPane, None),
        KeyBinding::new("cmd-[", PreviousPane, None),
        KeyBinding::new("cmd-shift-enter", TogglePaneZoom, None),
        KeyBinding::new("cmd-f", terminal_view::FindOutput, Some("Terminal")),
        KeyBinding::new("cmd-=", terminal_view::ZoomIn, Some("Terminal")),
        KeyBinding::new("cmd-+", terminal_view::ZoomIn, Some("Terminal")),
        KeyBinding::new("cmd--", terminal_view::ZoomOut, Some("Terminal")),
        KeyBinding::new("cmd-0", terminal_view::ResetZoom, Some("Terminal")),
        KeyBinding::new("cmd-up", terminal_view::PreviousCommand, Some("Terminal")),
        KeyBinding::new("cmd-down", terminal_view::NextCommand, Some("Terminal")),
        KeyBinding::new(
            "cmd-shift-c",
            terminal_view::CopyCommandOutput,
            Some("Terminal"),
        ),
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
        KeyBinding::new("enter", menu::Confirm, Some("ConfirmDialog")),
        KeyBinding::new("escape", menu::Cancel, Some("ConfirmDialog")),
    ]);
    hosts::reload(cx);
    crate::completions::init(cx);
    cx.on_action(|_: &Quit, cx| cx.quit());
    picker::init(cx);
}

/// Alto de la barra de pestañas; los semáforos de macOS se centran en ella.
pub const TAB_BAR_HEIGHT: Pixels = px(40.);

/// Con qué abre una ventana.
pub enum Start {
    /// Una pestaña en esa carpeta; sin carpeta, en la última que se usó.
    Cwd(Option<PathBuf>),
    /// Las pestañas y paneles de la sesión anterior.
    Restore(session::WindowSession),
}

/// Las ventanas de la sesión anterior (si `restore_session` no está apagado), o una nueva.
pub fn open_initial_windows(cx: &mut App) {
    let restore = config::get(cx).restore_session != Some(false);
    let windows = restore
        .then(session::load)
        .flatten()
        .map(|s| s.windows)
        .unwrap_or_default();
    if windows.is_empty() {
        open_window(Start::Cwd(None), None, cx);
    }
    for window in windows {
        // Si la pantalla donde estaba ya no está conectada, se abre centrada.
        let bounds = window
            .bounds
            .map(|[x, y, w, h]| Bounds::new(point(px(x), px(y)), size(px(w), px(h))))
            .filter(|bounds| {
                cx.displays()
                    .iter()
                    .any(|display| display.bounds().intersects(bounds))
            });
        open_window(Start::Restore(window), bounds, cx);
    }
}

/// Abre una ventana con su workspace; sin `bounds`, centrada.
pub fn open_window(start: Start, bounds: Option<Bounds<Pixels>>, cx: &mut App) {
    let bounds = bounds.unwrap_or_else(|| Bounds::centered(None, size(px(1100.), px(720.)), cx));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("terminalapp".into()),
            appears_transparent: true,
            // Centrados en la barra de pestañas.
            traffic_light_position: Some(point(px(14.), px(14.))),
        }),
        window_min_size: Some(size(px(480.), px(280.))),
        ..Default::default()
    };
    if let Err(err) = cx.open_window(options, |window, cx| {
        cx.new(|cx| Workspace::new(start, window, cx))
    }) {
        crate::log::error(format!("no se pudo abrir la ventana: {err:#}"));
    }
}

/// Los workspaces de todas las ventanas, para confirmar al salir (y guardar la sesión).
#[derive(Default)]
struct Workspaces(Vec<WeakEntity<Workspace>>);

impl Global for Workspaces {}

/// Una terminal dentro de una pestaña.
struct Pane {
    view: Entity<TerminalView>,
    initial_dir: Option<PathBuf>,
    _subscriptions: [Subscription; 2],
}

struct TabEntry {
    /// Identifica la pestaña para la confirmación de cierre aunque cambien sus paneles.
    id: usize,
    panes: Vec<Pane>,
    /// Cómo se reparten los paneles, por el id de su vista.
    layout: PaneTree<EntityId>,
    /// El panel con el foco: el que muestran el título y la barra de estado.
    active_pane: EntityId,
    /// El panel activo ocupa toda la pestaña (⌘⇧⏎).
    zoomed: bool,
    /// La sesión sonó la campana mientras no estaba visible.
    needs_attention: bool,
}

impl TabEntry {
    fn pane(&self, id: EntityId) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.view.entity_id() == id)
    }

    fn active(&self) -> &Pane {
        self.pane(self.active_pane).unwrap_or(&self.panes[0])
    }

    /// La vista del panel activo.
    fn view(&self) -> &Entity<TerminalView> {
        &self.active().view
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ModalKind {
    CommandPalette,
    History,
    Branches,
    Themes,
    Hosts,
}

/// Qué se quiere cerrar; si hay procesos en curso se pide confirmación antes.
#[derive(Clone, Copy, PartialEq)]
enum CloseTarget {
    Tab(usize),
    Pane(EntityId),
    Window,
    Quit,
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
    closed_dirs: Vec<PathBuf>,
    command_notice: Option<String>,
    active: usize,
    should_move: bool,
    shell_name: String,
    modal: Option<Modal>,
    /// Cierre esperando confirmación porque cortaría procesos.
    pending_close: Option<CloseTarget>,
    confirm_focus: FocusHandle,
    next_tab_id: usize,
    entity_id: EntityId,
}

impl Workspace {
    pub fn new(start: Start, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shell_name = std::env::var("SHELL")
            .ok()
            .and_then(|s| s.rsplit('/').next().map(str::to_string))
            .unwrap_or_else(|| "shell".into());
        let mut this = Self {
            tabs: Vec::new(),
            closed_dirs: state::closed_dirs(),
            command_notice: None,
            active: 0,
            should_move: false,
            shell_name,
            modal: None,
            pending_close: None,
            confirm_focus: cx.focus_handle(),
            next_tab_id: 0,
            entity_id: cx.entity_id(),
        };
        let me = cx.entity().downgrade();
        let registry = &mut cx.default_global::<Workspaces>().0;
        registry.retain(|workspace| workspace.upgrade().is_some());
        registry.push(me);
        // Al volver a la app se relee config.json, por si se editó.
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() && config::reload_if_changed(cx) {
                if let Some(error) = config::error(cx) {
                    this.command_notice = Some(error.to_string());
                }
                cx.refresh_windows();
            }
        })
        .detach();
        // El botón rojo de la ventana también pasa por la confirmación.
        let workspace = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            workspace
                .update(cx, |this, cx| {
                    let close = this.should_close(CloseTarget::Window, window, cx);
                    if close {
                        session::remove(this.entity_id, cx);
                    }
                    close
                })
                .unwrap_or(true)
        });
        match start {
            Start::Cwd(cwd) => this.add_tab_in(cwd.or_else(state::last_cwd), window, cx),
            Start::Restore(saved) => {
                for tab in saved.tabs {
                    if let Some(tab) = this.restore_tab(tab, window, cx) {
                        this.tabs.push(tab);
                    }
                }
                if this.tabs.is_empty() {
                    this.add_tab_in(state::last_cwd(), window, cx);
                } else {
                    this.activate(saved.active.min(this.tabs.len() - 1), window, cx);
                }
            }
        }
        // Mover o redimensionar la ventana se guarda en la sesión (ver `render`).
        cx.observe_window_bounds(window, |_, _, cx| cx.notify())
            .detach();
        this
    }

    /// Una pestaña de la sesión anterior: un panel nuevo por cada carpeta, con la misma
    /// distribución.
    fn restore_tab(
        &mut self,
        saved: session::TabSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<TabEntry> {
        let mut cwds = Vec::new();
        saved.layout.map(&mut |cwd| cwds.push(cwd.clone()));
        let mut panes = Vec::new();
        for cwd in cwds {
            panes.push(self.new_pane(cwd, window, cx)?);
        }
        let mut ids = panes.iter().map(|pane| pane.view.entity_id());
        let layout = saved
            .layout
            .map(&mut |_| ids.next().expect("un panel por hoja"));
        let leaves = layout.leaves();
        self.next_tab_id += 1;
        Some(TabEntry {
            id: self.next_tab_id,
            active_pane: leaves.get(saved.active_pane).copied().unwrap_or(leaves[0]),
            panes,
            layout,
            zoomed: false,
            needs_attention: false,
        })
    }

    /// Lo que esta ventana aporta a la sesión: pestañas, paneles con su carpeta y posición.
    fn snapshot(&self, window: &Window, cx: &App) -> session::WindowSession {
        let tabs = self
            .tabs
            .iter()
            .map(|tab| {
                let layout = tab.layout.map(&mut |id| {
                    let pane = tab.pane(*id)?;
                    pane.view
                        .read(cx)
                        .terminal()
                        .read(cx)
                        .shell()
                        .cwd
                        .clone()
                        .or_else(|| pane.initial_dir.clone())
                });
                let active_pane = tab
                    .layout
                    .leaves()
                    .iter()
                    .position(|id| *id == tab.active_pane)
                    .unwrap_or(0);
                session::TabSession {
                    layout,
                    active_pane,
                }
            })
            .collect();
        let bounds = window.window_bounds().get_bounds();
        session::WindowSession {
            tabs,
            active: self.active,
            bounds: Some([
                f32::from(bounds.origin.x),
                f32::from(bounds.origin.y),
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
            ]),
        }
    }

    /// ⌘N: otra ventana, corrida un poco respecto de esta y en la carpeta del panel activo.
    fn new_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self
            .tabs
            .get(self.active)
            .and_then(|tab| tab.view().read(cx).terminal().read(cx).shell().cwd.clone());
        let current = window.bounds();
        let bounds = Bounds::new(current.origin + point(px(24.), px(24.)), current.size);
        cx.defer(move |cx| open_window(Start::Cwd(cwd), Some(bounds), cx));
    }

    /// Una pestaña nueva abre en la carpeta de la pestaña activa; la primera, en la última
    /// carpeta usada en la ejecución anterior.
    fn add_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self
            .tabs
            .get(self.active)
            .and_then(|tab| tab.view().read(cx).terminal().read(cx).shell().cwd.clone())
            .or_else(state::last_cwd);
        self.add_tab_in(cwd, window, cx);
    }

    fn add_tab_in(&mut self, cwd: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.new_tab_entry(cwd, window, cx) {
            self.tabs.push(tab);
            self.activate(self.tabs.len() - 1, window, cx);
        }
    }

    fn new_tab_entry(
        &mut self,
        cwd: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<TabEntry> {
        let pane = self.new_pane(cwd, window, cx)?;
        let id = pane.view.entity_id();
        self.next_tab_id += 1;
        Some(TabEntry {
            id: self.next_tab_id,
            panes: vec![pane],
            layout: PaneTree::Leaf(id),
            active_pane: id,
            zoomed: false,
            needs_attention: false,
        })
    }

    fn new_pane(
        &mut self,
        cwd: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Pane> {
        let initial_dir = cwd
            .clone()
            .filter(|dir| dir.is_dir())
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from));
        let builder = match TerminalBuilder::new(cwd, config::get(cx).scrollback) {
            Ok(builder) => builder,
            Err(err) => {
                crate::log::error(format!("no se pudo abrir la terminal: {err:#}"));
                self.command_notice = Some(format!("No se pudo abrir la terminal: {err}"));
                cx.notify();
                return None;
            }
        };
        let terminal = cx.new(|cx| builder.build(cx));
        let events = cx.subscribe_in(&terminal, window, Self::on_terminal_event);
        let view = cx.new(|cx| TerminalView::new(terminal, cx));
        // Un clic en un panel (o el foco en su buscador) lo vuelve el activo.
        let id = view.entity_id();
        let focus = cx.on_focus_in(&view.focus_handle(cx), window, move |this, _, cx| {
            this.pane_focused(id, cx)
        });
        Some(Pane {
            view,
            initial_dir,
            _subscriptions: [events, focus],
        })
    }

    fn pane_focused(&mut self, id: EntityId, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.pane(id).is_some())
            && tab.active_pane != id
        {
            tab.active_pane = id;
            cx.notify();
        }
    }

    /// La pestaña y el panel de `terminal`.
    fn locate(&self, terminal: &Entity<Terminal>, cx: &App) -> Option<(usize, EntityId)> {
        self.tabs.iter().enumerate().find_map(|(ix, tab)| {
            tab.panes
                .iter()
                .find(|pane| pane.view.read(cx).terminal() == terminal)
                .map(|pane| (ix, pane.view.entity_id()))
        })
    }

    /// Divide el panel activo; el nuevo abre en la misma carpeta y queda con el foco.
    fn split(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let current = tab.active_pane;
        let cwd = tab.view().read(cx).terminal().read(cx).shell().cwd.clone();
        let Some(pane) = self.new_pane(cwd, window, cx) else {
            return;
        };
        let id = pane.view.entity_id();
        let tab = &mut self.tabs[self.active];
        tab.layout.split(current, id, axis);
        tab.panes.push(pane);
        tab.zoomed = false;
        self.focus_pane(id, window, cx);
    }

    fn focus_pane(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        if let Some(pane) = tab.pane(id) {
            let view = pane.view.clone();
            tab.active_pane = id;
            view.focus_handle(cx).focus(window, cx);
            cx.notify();
        }
    }

    fn focus_neighbor(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        if let Some(id) = tab.layout.neighbor(tab.active_pane, direction) {
            self.tabs[self.active].zoomed = false;
            self.focus_pane(id, window, cx);
        }
    }

    /// ⌘] / ⌘[: el panel siguiente o anterior en orden de lectura, dando la vuelta.
    fn cycle_pane(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let leaves = tab.layout.leaves();
        let Some(current) = leaves.iter().position(|id| *id == tab.active_pane) else {
            return;
        };
        let next = leaves[(current as isize + delta).rem_euclid(leaves.len() as isize) as usize];
        self.focus_pane(next, window, cx);
    }

    fn toggle_zoom(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get_mut(self.active)
            && tab.panes.len() > 1
        {
            tab.zoomed = !tab.zoomed;
            cx.notify();
        }
    }

    /// Cierra un panel; si es el último de la pestaña, la pestaña.
    fn close_pane(&mut self, ix: usize, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(ix) else {
            return;
        };
        if tab.panes.len() == 1 {
            self.close(ix, window, cx);
            return;
        }
        let order = tab.layout.leaves();
        let position = order.iter().position(|leaf| *leaf == id).unwrap_or(0);
        tab.layout.remove(id);
        tab.panes.retain(|pane| pane.view.entity_id() != id);
        tab.zoomed = false;
        if self.pending_close == Some(CloseTarget::Pane(id)) {
            self.pending_close = None;
        }
        if tab.active_pane == id {
            // El foco pasa al panel que estaba antes en orden de lectura (o al primero).
            let leaves = tab.layout.leaves();
            tab.active_pane = leaves[position.saturating_sub(1).min(leaves.len() - 1)];
        }
        if ix == self.active {
            let active = tab.active_pane;
            self.focus_pane(active, window, cx);
        }
        cx.notify();
    }

    /// Reemplaza la shell del panel activo, que terminó con error, por una nueva en la misma
    /// carpeta.
    fn restart_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self.active;
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        let old = tab.active_pane;
        let cwd = tab
            .view()
            .read(cx)
            .terminal()
            .read(cx)
            .shell()
            .cwd
            .clone()
            .or_else(|| tab.active().initial_dir.clone());
        if let Some(pane) = self.new_pane(cwd, window, cx) {
            let id = pane.view.entity_id();
            let tab = &mut self.tabs[ix];
            tab.layout.replace(old, id);
            if let Some(slot) = tab
                .panes
                .iter_mut()
                .find(|pane| pane.view.entity_id() == old)
            {
                *slot = pane;
            }
            self.focus_pane(id, window, cx);
        }
    }

    fn on_terminal_event(
        &mut self,
        terminal: &Entity<Terminal>,
        event: &TerminalEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((ix, pane)) = self.locate(terminal, cx) else {
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
            TerminalEvent::CommandFinished {
                command,
                exit_code,
                duration,
            } => {
                if duration.as_secs() >= 10 && (ix != self.active || !window.is_window_active()) {
                    self.tabs[ix].needs_attention = true;
                    let result = match exit_code {
                        Some(0) => "terminó correctamente".to_string(),
                        Some(code) => format!("terminó con código {code}"),
                        None => "terminó (resultado desconocido)".to_string(),
                    };
                    let elapsed = crate::components::format_duration(*duration);
                    self.command_notice = Some(format!(
                        "{} · {} · {}",
                        short_command(command),
                        result,
                        elapsed
                    ));
                    // Con la app en segundo plano, además una notificación de macOS.
                    if !window.is_window_active() && config::get(cx).notifications != Some(false) {
                        let mark = if *exit_code == Some(0) { "✓" } else { "✕" };
                        crate::notify::send(
                            &format!("{mark} {}", short_command(command)),
                            &format!("{result} · {elapsed}"),
                        );
                    }
                    cx.notify();
                }
            }
            TerminalEvent::Exited => {
                if terminal.read(cx).exit_failure().is_some() {
                    if ix != self.active {
                        self.tabs[ix].needs_attention = true;
                    }
                    cx.notify();
                } else {
                    self.close_pane(ix, pane, window, cx);
                }
            }
        }
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(ix) else {
            return;
        };
        tab.needs_attention = false;
        self.active = ix;
        tab.view().focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn close(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        if let Some(dir) = self.tabs[ix]
            .view()
            .read(cx)
            .terminal()
            .read(cx)
            .shell()
            .cwd
            .clone()
            .or_else(|| self.tabs[ix].active().initial_dir.clone())
        {
            self.closed_dirs.push(dir);
            if self.closed_dirs.len() > 20 {
                self.closed_dirs.remove(0);
            }
            state::save_closed_dirs(&self.closed_dirs);
        }
        let removed = self.tabs.remove(ix);
        let removed_pane = |id| removed.pane(id).is_some();
        match self.pending_close {
            Some(CloseTarget::Tab(id)) if id == removed.id => self.pending_close = None,
            Some(CloseTarget::Pane(id)) if removed_pane(id) => self.pending_close = None,
            _ => {}
        }
        if self.tabs.is_empty() {
            session::remove(self.entity_id, cx);
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

    /// Abre el archivo en el editor de config.json (o el que se detecte): uno de terminal va en
    /// una pestaña nueva en la carpeta actual.
    fn open_file(
        &mut self,
        file: &crate::links::FileLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shell = self.active_terminal(cx).read(cx).shell();
        let cwd = shell.cwd.clone();
        let search_path = shell.path_env.clone().unwrap_or_else(editor::fallback_path);
        let error = match editor::launch(config::get(cx).editor.as_deref(), file, &search_path) {
            Launch::App { program, args } => editor::spawn(&program, &args, &search_path)
                .err()
                .map(|err| format!("No se pudo abrir {}: {err}", program.display())),
            Launch::Terminal(command) => {
                self.add_tab_in(cwd, window, cx);
                let terminal = self.active_terminal(cx);
                terminal.update(cx, |t, cx| t.run_when_ready(command, cx));
                None
            }
            Launch::System => editor::spawn(
                Path::new("/usr/bin/open"),
                &[file.path.display().to_string()],
                &search_path,
            )
            .err()
            .map(|err| format!("No se pudo abrir el archivo: {err}")),
            Launch::Missing(program) => Some(format!(
                "No se encontró «{program}», el editor de config.json, en el PATH de la shell"
            )),
        };
        if let Some(error) = error {
            crate::log::error(&error);
            self.command_notice = Some(error);
            cx.notify();
        }
    }

    fn open_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match config::ensure_file() {
            Ok(path) => {
                let file = crate::links::FileLink {
                    path,
                    line: None,
                    column: None,
                };
                self.open_file(&file, window, cx);
            }
            Err(err) => {
                self.command_notice = Some(format!("No se pudo crear config.json: {err}"));
                cx.notify();
            }
        }
    }

    /// Lo que se cortaría al cerrar el panel: la sesión SSH o el comando en curso.
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

    /// Al salir también cuentan los procesos de las otras ventanas.
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

    /// Si se puede cerrar ya. Si se cortarían procesos, abre la confirmación y devuelve
    /// `false`; pedir lo mismo con la confirmación abierta (⌘W ⌘W, ⌘Q ⌘Q) confirma.
    fn should_close(
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

    fn request_close(&mut self, target: CloseTarget, window: &mut Window, cx: &mut Context<Self>) {
        if self.should_close(target, window, cx) {
            self.perform_close(target, window, cx);
        }
    }

    fn request_close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(ix) {
            let target = CloseTarget::Tab(tab.id);
            self.request_close(target, window, cx);
        }
    }

    /// ⌘W: cierra el panel activo, o la pestaña si tiene uno solo.
    fn request_close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn render_close_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let target = self.pending_close?;
        let busy = self.busy(target, cx);
        let (title, confirm) = match target {
            CloseTarget::Tab(_) => ("¿Cerrar la pestaña?", "Cerrar"),
            CloseTarget::Pane(_) => ("¿Cerrar el panel?", "Cerrar"),
            CloseTarget::Window => ("¿Cerrar la ventana?", "Cerrar"),
            CloseTarget::Quit => ("¿Salir de terminalapp?", "Salir"),
        };
        let detail = match busy.as_slice() {
            [] => "Ya no hay procesos en curso.".to_string(),
            [only] => format!("Se terminará {only}."),
            many => format!(
                "Se terminarán {} procesos: {}.",
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
                                    Button::new("close-cancel", "Cancelar")
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

    fn reopen_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(dir) = self.closed_dirs.pop() {
            if dir.is_dir() {
                let count = self.tabs.len();
                self.add_tab_in(Some(dir.clone()), window, cx);
                if self.tabs.len() == count {
                    self.closed_dirs.push(dir);
                }
                state::save_closed_dirs(&self.closed_dirs);
                break;
            }
            state::save_closed_dirs(&self.closed_dirs);
        }
    }

    /// El comando en ejecución, si no la carpeta actual, si no el título que puso la shell.
    fn tab_title(&self, ix: usize, cx: &App) -> SharedString {
        let terminal = self.tabs[ix].view().read(cx).terminal().read(cx);
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

    /// Algún panel de la pestaña tiene un comando en curso.
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
        let shell = self.tabs[ix].view().read(cx).terminal().read(cx).shell();
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
            .when(ix < 9, |tab| tab.shortcut(format!("⌘{}", ix + 1)))
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
        let terminal = self.tabs[self.active].view().read(cx).terminal().clone();
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
        let terminal = self.tabs[self.active].view().read(cx).terminal().read(cx);
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
    fn active_terminal(&self, cx: &App) -> Entity<Terminal> {
        self.tabs[self.active].view().read(cx).terminal().clone()
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
            self.tabs[self.active]
                .view()
                .focus_handle(cx)
                .focus(window, cx);
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
            PickerItem::new(
                "Acciones",
                "Nueva pestaña",
                PickerAction::Dispatch(Box::new(NewTab)),
            )
            .icon(IconName::Plus)
            .shortcut(NewTab),
            PickerItem::new(
                "Acciones",
                "Buscar en el historial",
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
                    "Acciones",
                    "Reconectar",
                    PickerAction::Dispatch(Box::new(Reconnect)),
                )
                .icon(IconName::RotateCw)
                .shortcut(Reconnect),
            );
        }
        if has_git {
            items.push(
                PickerItem::new(
                    "Acciones",
                    "Cambiar de rama…",
                    PickerAction::Dispatch(Box::new(SwitchBranch)),
                )
                .icon(IconName::GitBranch),
            );
        }
        items.extend([
            PickerItem::new(
                "Acciones",
                "Dividir a la derecha",
                PickerAction::Dispatch(Box::new(SplitRight)),
            )
            .shortcut(SplitRight),
            PickerItem::new(
                "Acciones",
                "Dividir hacia abajo",
                PickerAction::Dispatch(Box::new(SplitDown)),
            )
            .shortcut(SplitDown),
            PickerItem::new(
                "Acciones",
                "Maximizar o restaurar el panel",
                PickerAction::Dispatch(Box::new(TogglePaneZoom)),
            )
            .shortcut(TogglePaneZoom),
            PickerItem::new(
                "Acciones",
                "Reabrir pestaña cerrada",
                PickerAction::Dispatch(Box::new(ReopenTab)),
            )
            .shortcut(ReopenTab),
            PickerItem::new(
                "Acciones",
                "Buscar en la salida",
                PickerAction::Dispatch(Box::new(terminal_view::FindOutput)),
            )
            .shortcut(terminal_view::FindOutput),
            PickerItem::new(
                "Acciones",
                "Copiar la salida del último comando",
                PickerAction::Dispatch(Box::new(terminal_view::CopyCommandOutput)),
            )
            .shortcut(terminal_view::CopyCommandOutput),
            PickerItem::new(
                "Acciones",
                "Ir al comando anterior",
                PickerAction::Dispatch(Box::new(terminal_view::PreviousCommand)),
            )
            .shortcut(terminal_view::PreviousCommand),
            PickerItem::new(
                "Acciones",
                "Aumentar texto",
                PickerAction::Dispatch(Box::new(terminal_view::ZoomIn)),
            ),
            PickerItem::new(
                "Acciones",
                "Reducir texto",
                PickerAction::Dispatch(Box::new(terminal_view::ZoomOut)),
            ),
            PickerItem::new(
                "Acciones",
                "Restablecer zoom",
                PickerAction::Dispatch(Box::new(terminal_view::ResetZoom)),
            ),
            PickerItem::new(
                "Acciones",
                if cx.global::<terminal_view::OptionAsMeta>().0 {
                    "Dejar de usar ⌥ como Meta"
                } else {
                    "Usar ⌥ como Meta (⌥B, ⌥F, ⌥.)"
                },
                PickerAction::Dispatch(Box::new(terminal_view::ToggleOptionAsMeta)),
            ),
            PickerItem::new(
                "Acciones",
                "Conectar a…",
                PickerAction::Dispatch(Box::new(ConnectToHost)),
            )
            .icon(IconName::Server),
            PickerItem::new(
                "Acciones",
                "Cambiar tema…",
                PickerAction::Dispatch(Box::new(SelectTheme)),
            )
            .icon(IconName::Sparkle),
            PickerItem::new(
                "Acciones",
                "Abrir configuración (fuente, editor, scrollback)",
                PickerAction::Dispatch(Box::new(OpenConfig)),
            )
            .icon(IconName::Settings),
            PickerItem::new(
                "Acciones",
                "Limpiar pantalla",
                PickerAction::Dispatch(Box::new(ClearScreen)),
            )
            .icon(IconName::Eraser)
            .shortcut(ClearScreen),
            PickerItem::new(
                "Acciones",
                "Copiar pantalla",
                PickerAction::Dispatch(Box::new(terminal_view::Copy)),
            )
            .icon(IconName::Copy),
            PickerItem::new(
                "Acciones",
                "Pegar",
                PickerAction::Dispatch(Box::new(terminal_view::Paste)),
            )
            .icon(IconName::Download),
            PickerItem::new(
                "Acciones",
                "Pestaña siguiente",
                PickerAction::Dispatch(Box::new(NextTab)),
            )
            .icon(IconName::ArrowRight)
            .shortcut(NextTab),
            PickerItem::new(
                "Acciones",
                "Pestaña anterior",
                PickerAction::Dispatch(Box::new(PreviousTab)),
            )
            .icon(IconName::ArrowLeft)
            .shortcut(PreviousTab),
            PickerItem::new(
                "Acciones",
                "Cerrar pestaña",
                PickerAction::Dispatch(Box::new(CloseTab)),
            )
            .icon(IconName::Close)
            .shortcut(CloseTab),
            PickerItem::new(
                "Acciones",
                "Salir de terminalapp",
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
                PickerItem::new(
                    "Carpetas recientes",
                    abbreviate_home(&dir),
                    PickerAction::OpenDirInNewTab(dir),
                )
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
                        PickerItem::new(
                            "",
                            command.clone(),
                            PickerAction::InsertCommand(command.clone()),
                        )
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
                let item = PickerItem::new(
                    "",
                    branch.clone(),
                    PickerAction::SwitchBranch(branch.clone()),
                )
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
                let mut item =
                    PickerItem::new("", t.name.clone(), PickerAction::SetTheme(t.name.clone()))
                        .icon(if t.name == current {
                            IconName::Check
                        } else {
                            IconName::Sparkle
                        })
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

    fn reconnect_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        let terminal = tab.view().read(cx).terminal().clone();
        if terminal.update(cx, |t, cx| t.reconnect(cx)) {
            self.activate(ix, window, cx);
        }
    }

    /// Franja bajo las pestañas cuando la sesión activa es de producción.
    fn render_production_strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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
                    "PRODUCCIÓN".to_string()
                } else {
                    l.to_uppercase()
                }
            })
            .unwrap_or_else(|| "PRODUCCIÓN".into());
        let reason = match (&remote.identity.matched, &hosts::rules(cx).source) {
            (Some(pattern), RulesSource::File(_)) => {
                format!("· coincide con {pattern} en hosts.json")
            }
            (Some(pattern), RulesSource::Defaults) => {
                format!("· coincide con {pattern} (regla por defecto)")
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
                            Label::new("Pegado protegido")
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
            let shell = tab.view().read(cx).terminal().read(cx).shell();
            let Some(d) = shell
                .disconnected
                .as_ref()
                .filter(|_| shell.running.is_none())
            else {
                continue;
            };
            let minutes = d.at.elapsed().as_secs() / 60;
            let ago = if minutes == 0 {
                "recién".to_string()
            } else {
                format!("hace {minutes} min")
            };
            let mut item = PickerItem::new(
                "Reconectar",
                d.target.alias.clone(),
                PickerAction::ReconnectTab(ix),
            )
            .icon(IconName::RotateCw)
            .detail(format!(
                "se cortó {ago} · exit {}",
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

        // Pestañas con sesión abierta, para avisarlo en cada host.
        let open_tabs: Vec<(usize, String)> = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(ix, tab)| {
                let shell = tab.view().read(cx).terminal().read(cx).shell();
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
                    "Del historial",
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
                (
                    "⌘⏎".into(),
                    if at_prompt {
                        "en esta pestaña".into()
                    } else {
                        "en esta pestaña (no disponible)".into()
                    },
                ),
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

impl Workspace {
    /// Los paneles de la pestaña activa: uno solo (o el maximizado) ocupa todo; si no, el árbol
    /// con divisores de un píxel y los paneles sin foco atenuados.
    fn render_panes(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tab = self.tabs.get(self.active)?;
        if tab.zoomed || tab.panes.len() == 1 {
            return Some(tab.view().clone().into_any_element());
        }
        Some(self.render_pane_node(tab, &tab.layout, Vec::new(), cx))
    }

    fn render_pane_node(
        &self,
        tab: &TabEntry,
        node: &PaneTree<EntityId>,
        path: Vec<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = cx.tokens().clone();
        match node {
            PaneTree::Leaf(id) => {
                let Some(pane) = tab.pane(*id) else {
                    return div().into_any_element();
                };
                div()
                    .relative()
                    .size_full()
                    .child(pane.view.clone())
                    .when(*id != tab.active_pane, |el| {
                        el.child(
                            div()
                                .absolute()
                                .inset_0()
                                .bg(tint(tokens.terminal.background, 0.35)),
                        )
                    })
                    .into_any_element()
            }
            PaneTree::Split {
                axis,
                children,
                sizes,
            } => {
                let axis = *axis;
                let tab_id = tab.id;
                let mut container = div().flex().size_full().on_drag_move(cx.listener({
                    let path = path.clone();
                    move |this, event: &DragMoveEvent<DividerDrag>, _, cx| {
                        let drag = event.drag(cx);
                        if drag.tab != tab_id || drag.path != path {
                            return;
                        }
                        let divider = drag.divider;
                        let (offset, length) = match axis {
                            Axis::Row => (
                                event.event.position.x - event.bounds.origin.x,
                                event.bounds.size.width,
                            ),
                            Axis::Column => (
                                event.event.position.y - event.bounds.origin.y,
                                event.bounds.size.height,
                            ),
                        };
                        if length <= px(0.) {
                            return;
                        }
                        let min = MIN_PANE_SIZE / length;
                        if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                            tab.layout.resize(&path, divider, offset / length, min);
                            cx.notify();
                        }
                    }
                }));
                container = match axis {
                    Axis::Row => container.flex_row(),
                    Axis::Column => container.flex_col(),
                };
                for (ix, (child, size)) in children.iter().zip(sizes).enumerate() {
                    if ix > 0 {
                        container =
                            container.child(self.render_divider(tab_id, &path, ix, axis, cx));
                    }
                    let mut child_path = path.clone();
                    child_path.push(ix);
                    container = container.child(
                        div()
                            .flex_basis(relative(*size))
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .child(self.render_pane_node(tab, child, child_path, cx)),
                    );
                }
                container.into_any_element()
            }
        }
    }

    /// Línea de un píxel entre paneles, con una zona más ancha para agarrarla y arrastrarla.
    fn render_divider(
        &self,
        tab: usize,
        path: &[usize],
        divider: usize,
        axis: Axis,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let drag = DividerDrag {
            tab,
            path: path.to_vec(),
            divider,
        };
        let id = SharedString::from(format!("divider-{tab}-{path:?}-{divider}"));
        let handle = div()
            .id(id)
            .absolute()
            .occlude()
            .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()));
        let handle = match axis {
            Axis::Row => handle
                .top_0()
                .bottom_0()
                .left(px(-3.))
                .w(px(7.))
                .cursor(CursorStyle::ResizeLeftRight),
            Axis::Column => handle
                .left_0()
                .right_0()
                .top(px(-3.))
                .h(px(7.))
                .cursor(CursorStyle::ResizeUpDown),
        };
        let line = div()
            .relative()
            .flex_none()
            .bg(cx.tokens().ui.border_strong);
        let line = match axis {
            Axis::Row => line.w(px(1.)).h_full(),
            Axis::Column => line.h(px(1.)).w_full(),
        };
        // `deferred` la pinta encima de los paneles, para que el panel de la derecha (o de
        // abajo) no le tape la mitad de la zona.
        line.child(deferred(handle)).into_any_element()
    }
}

/// Lo que viaja mientras se arrastra un divisor; también es la vista (vacía) del arrastre.
#[derive(Clone)]
struct DividerDrag {
    tab: usize,
    path: Vec<usize>,
    divider: usize,
}

impl Render for DividerDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// Ningún panel se achica por debajo de esto al arrastrar.
const MIN_PANE_SIZE: Pixels = px(80.);

impl Focusable for Workspace {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.tabs[self.active].view().focus_handle(cx)
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Cualquier cambio que se ve (pestañas, paneles, carpetas, la ventana) pasa por acá.
        if !self.tabs.is_empty() {
            let snapshot = self.snapshot(window, cx);
            session::update(self.entity_id, snapshot, cx);
        }
        v_flex()
            .relative()
            .size_full()
            .key_context("Workspace")
            .bg(cx.tokens().ui.background)
            .text_color(cx.tokens().ui.text)
            .font_family(UI_FONT_FAMILY)
            .text_size(UI_FONT_SIZE)
            .on_action(cx.listener(|this, _: &NewTab, window, cx| this.add_tab(window, cx)))
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
            .child(self.render_tab_bar(cx))
            .when_some(self.command_notice.clone(), |el, notice| {
                el.child(
                    h_flex()
                        .px(px(16.))
                        .py(px(6.))
                        .gap(px(12.))
                        .child(Label::new(notice))
                        .child(Button::new("dismiss-command-notice", "Cerrar").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.command_notice = None;
                                cx.notify();
                            }),
                        )),
                )
            })
            .children(self.render_production_strip(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(self.active_is_production(cx), |el| {
                        el.p(px(8.)).bg(cx.tokens().terminal.background)
                    })
                    .children(self.render_panes(cx)),
            )
            .when(!self.tabs.is_empty(), |this| {
                this.child(self.render_status_bar(cx))
            })
            .children(self.render_modal(cx))
            .children(self.render_close_confirm(cx))
    }
}
