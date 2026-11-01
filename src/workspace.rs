//! Window: tab bar, active terminal and status bar, built with components from Zed's `ui`
//! crate (Label, HighlightedLabel, Icon, IconButton, Button, KeyBinding, Tooltip), plus the
//! search overlays: command palette, history and branches.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::{
    Action, App, Bounds, ClickEvent, Context, CursorStyle, DragMoveEvent, Entity, EntityId,
    FocusHandle, Focusable, Global, KeyBinding, MouseButton, MouseDownEvent, Pixels, SharedString,
    Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowDecorations,
    WindowOptions, actions, deferred, div, point, prelude::*, px, relative, size,
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
    session, shell, shortcuts, ssh, state,
    terminal::{Terminal, TerminalBuilder, TerminalEvent},
    terminal_view::{self, TerminalView},
    theme::{self, Themes},
    theme::{MONO_FONT_FAMILY, UI_FONT_FAMILY, UI_FONT_SIZE},
    tokens::{Appearance, Tokens, TokensExt, metrics, tint},
    updates,
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

/// Open a file from the output in the editor (clicking a path).
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = workspace, no_json)]
pub struct OpenFile {
    pub file: crate::links::FileLink,
}

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

/// Tab bar height; the macOS traffic lights are centered within it.
pub const TAB_BAR_HEIGHT: Pixels = px(40.);

/// What a window opens with.
pub enum Start {
    /// One tab in that folder; with no folder, in the last one used.
    Cwd(Option<PathBuf>),
    /// The tabs and panes from the previous session.
    Restore(session::WindowSession),
}

/// The windows from the previous session (unless `restore_session` is off), or a new one.
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
        // If the display it was on is no longer connected, open it centered.
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

/// The window icon X11 shows in the taskbar and the window switcher (Wayland and the other
/// systems take it from the app's desktop entry or bundle).
#[cfg(target_os = "linux")]
fn window_icon() -> Option<std::sync::Arc<image::RgbaImage>> {
    static ICON: std::sync::OnceLock<Option<std::sync::Arc<image::RgbaImage>>> =
        std::sync::OnceLock::new();
    ICON.get_or_init(|| {
        let png = include_bytes!("../assets/branding/moth-256.png");
        let icon = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?;
        Some(std::sync::Arc::new(icon.to_rgba8()))
    })
    .clone()
}

#[cfg(not(target_os = "linux"))]
fn window_icon() -> Option<std::sync::Arc<image::RgbaImage>> {
    None
}

/// Opens a window with its workspace; centered when `bounds` is `None`.
pub fn open_window(start: Start, bounds: Option<Bounds<Pixels>>, cx: &mut App) {
    let bounds = bounds.unwrap_or_else(|| Bounds::centered(None, size(px(1100.), px(720.)), cx));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // On macOS the tab bar is the title bar, with the traffic lights centered in it; Linux
        // and Windows keep the system's.
        titlebar: Some(TitlebarOptions {
            title: Some("Moth".into()),
            appears_transparent: cfg!(target_os = "macos"),
            traffic_light_position: cfg!(target_os = "macos").then(|| point(px(14.), px(14.))),
        }),
        window_decorations: Some(WindowDecorations::Server),
        // Matches the window with the desktop entry `install.sh` adds on Linux.
        app_id: Some("moth".into()),
        icon: window_icon(),
        window_min_size: Some(size(px(480.), px(280.))),
        ..Default::default()
    };
    if let Err(err) = cx.open_window(options, |window, cx| {
        cx.new(|cx| Workspace::new(start, window, cx))
    }) {
        crate::log::error(format!("failed to open window: {err:#}"));
    }
}

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

/// The workspaces of every window, used to confirm on quit (and to save the session).
#[derive(Default)]
struct Workspaces(Vec<WeakEntity<Workspace>>);

impl Global for Workspaces {}

/// A terminal inside a tab.
struct Pane {
    view: Entity<TerminalView>,
    initial_dir: Option<PathBuf>,
    _subscriptions: [Subscription; 2],
}

struct TabEntry {
    /// Identifies the tab for close confirmation even if its panes change.
    id: usize,
    panes: Vec<Pane>,
    /// How the panes are laid out, keyed by their view's id.
    layout: PaneTree<EntityId>,
    /// The focused pane: the one the title and status bar reflect.
    active_pane: EntityId,
    /// The active pane fills the whole tab (⌘⇧⏎).
    zoomed: bool,
    /// The session rang the bell while it wasn't visible.
    needs_attention: bool,
}

impl TabEntry {
    fn pane(&self, id: EntityId) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.view.entity_id() == id)
    }

    fn active(&self) -> &Pane {
        self.pane(self.active_pane).unwrap_or(&self.panes[0])
    }

    /// The active pane's view.
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

/// What is being closed; if processes are running, confirmation is asked first.
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
    /// Theme to restore if the picker closes without a choice (undoes the preview).
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
    /// A close awaiting confirmation because it would kill processes.
    pending_close: Option<CloseTarget>,
    confirm_focus: FocusHandle,
    next_tab_id: usize,
    entity_id: EntityId,
}

impl Workspace {
    pub fn new(start: Start, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (program, _) = shell::shell_command(config::get(cx).shell.as_deref());
        let shell_name = shell::shell_name(&program);
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
        // Re-read config.json when the app regains focus, in case it was edited.
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() && config::reload_if_changed(cx) {
                if let Some(error) = config::error(cx) {
                    this.command_notice = Some(error.to_string());
                }
                cx.refresh_windows();
            }
        })
        .detach();
        // The window's red close button also goes through confirmation.
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
        // Moving or resizing the window is saved to the session (see `render`).
        cx.observe_window_bounds(window, |_, _, cx| cx.notify())
            .detach();
        this
    }

    /// A tab from the previous session: a new pane for each folder, with the same
    /// layout.
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
            .map(&mut |_| ids.next().expect("one pane per leaf"));
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

    /// What this window contributes to the session: tabs, panes with their folders, and position.
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

    /// ⌘N: another window, offset slightly from this one and in the active pane's folder.
    fn new_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self
            .tabs
            .get(self.active)
            .and_then(|tab| tab.view().read(cx).terminal().read(cx).shell().cwd.clone());
        let current = window.bounds();
        let bounds = Bounds::new(current.origin + point(px(24.), px(24.)), current.size);
        cx.defer(move |cx| open_window(Start::Cwd(cwd), Some(bounds), cx));
    }

    /// A new tab opens in the active tab's folder; the first one, in the last folder used
    /// in the previous run.
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
            .or_else(state::home_dir);
        let builder = match TerminalBuilder::new(cwd, config::get(cx)) {
            Ok(builder) => builder,
            Err(err) => {
                crate::log::error(format!("failed to open terminal: {err:#}"));
                self.command_notice = Some(format!("Couldn't open the terminal: {err}"));
                cx.notify();
                return None;
            }
        };
        let terminal = cx.new(|cx| builder.build(cx));
        let events = cx.subscribe_in(&terminal, window, Self::on_terminal_event);
        let view = cx.new(|cx| TerminalView::new(terminal, cx));
        // Clicking a pane (or focusing its search field) makes it the active one.
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

    /// The tab and pane that hold `terminal`.
    fn locate(&self, terminal: &Entity<Terminal>, cx: &App) -> Option<(usize, EntityId)> {
        self.tabs.iter().enumerate().find_map(|(ix, tab)| {
            tab.panes
                .iter()
                .find(|pane| pane.view.read(cx).terminal() == terminal)
                .map(|pane| (ix, pane.view.entity_id()))
        })
    }

    /// Splits the active pane; the new one opens in the same folder and takes focus.
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

    /// ⌘] / ⌘[: the next or previous pane in reading order, wrapping around.
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

    /// Closes a pane; if it's the tab's last one, closes the tab.
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
            // Focus moves to the previous pane in reading order (or the first one).
            let leaves = tab.layout.leaves();
            tab.active_pane = leaves[position.saturating_sub(1).min(leaves.len() - 1)];
        }
        if ix == self.active {
            let active = tab.active_pane;
            self.focus_pane(active, window, cx);
        }
        cx.notify();
    }

    /// Replaces the active pane's shell, which exited with an error, with a new one in the
    /// same folder.
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
                        Some(0) => "finished successfully".to_string(),
                        Some(code) => format!("exited with code {code}"),
                        None => "finished (exit status unknown)".to_string(),
                    };
                    let elapsed = crate::components::format_duration(*duration);
                    self.command_notice = Some(format!(
                        "{} · {} · {}",
                        short_command(command),
                        result,
                        elapsed
                    ));
                    // With the app in the background, also send a macOS notification.
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

    /// Opens the file in the editor from config.json (or a detected one); a terminal editor
    /// runs in a new tab in the current folder.
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
                .map(|err| format!("Couldn't open {}: {err}", program.display())),
            Launch::Terminal(command) => {
                self.add_tab_in(cwd, window, cx);
                let terminal = self.active_terminal(cx);
                terminal.update(cx, |t, cx| t.run_when_ready(command, cx));
                None
            }
            Launch::System => {
                cx.open_with_system(&file.path);
                None
            }
            Launch::Missing(program) => Some(format!(
                "Couldn't find \"{program}\" (the editor set in config.json) in the shell's PATH"
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
                self.command_notice = Some(format!("Couldn't create config.json: {err}"));
                cx.notify();
            }
        }
    }

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

    /// ⌘W: closes the active pane, or the tab if it has only one.
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

    /// The running command, else the current folder, else the title set by the shell.
    fn tab_title(&self, ix: usize, cx: &App) -> SharedString {
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
    if state::home_dir().is_some_and(|home| home == path) {
        return "~".into();
    }
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// `~` in place of home and, for long paths, only the last three folders (like the prompt).
fn abbreviate_home(path: &Path) -> String {
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

    fn toggle_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        };
        self.open_modal(ModalKind::CommandPalette, items, config, window, cx);
    }

    fn search_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = self.active_terminal(cx);
        let terminal = terminal.read(cx);
        // Start with whatever was already typed at the prompt, like ⌃R.
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
            placeholder: "Search history…".into(),
            query,
            empty_text: if items.is_empty() {
                "History hasn't loaded yet or is empty".into()
            } else {
                "No matching commands".into()
            },
            hints: vec![
                ("↑↓".into(), "navigate".into()),
                ("⏎".into(), "insert at prompt".into()),
                (shortcuts::label("cmd-enter").into(), "run".into()),
                ("esc".into(), "close".into()),
            ],
            disabled_reason: (!terminal.at_prompt())
                .then(|| "A command is running; wait for it to finish to use history".into()),
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
        };
        self.open_modal(ModalKind::Branches, items, config, window, cx);
    }

    fn select_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Banner below the tabs when the active session is a production one.
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

    fn connect_to_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    /// The active tab's panes: a single one (or the maximized one) fills everything; otherwise
    /// the tree with one-pixel dividers and unfocused panes dimmed.
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

    /// One-pixel line between panes, with a wider hit area for grabbing and dragging it.
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
        // `deferred` paints it above the panes, so the pane to the right (or below) doesn't
        // cover half of the hit area.
        line.child(deferred(handle)).into_any_element()
    }
}

/// Payload carried while dragging a divider; also serves as the (empty) drag view.
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

/// No pane shrinks below this when dragging.
const MIN_PANE_SIZE: Pixels = px(80.);

impl Focusable for Workspace {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.tabs[self.active].view().focus_handle(cx)
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Every visible change (tabs, panes, folders, the window) goes through here.
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
                        .child(Button::new("dismiss-command-notice", "Close").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.command_notice = None;
                                cx.notify();
                            }),
                        )),
                )
            })
            .children(updates::render_notice(cx))
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
