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
    terminal::{ShellState, Terminal, TerminalBuilder, TerminalEvent},
    terminal_view::{self, TerminalView},
    theme::{self, Themes},
    theme::{MONO_FONT_FAMILY, UI_FONT_FAMILY, UI_FONT_SIZE},
    tokens::{Appearance, Tokens, TokensExt, metrics, tint},
    updates,
};

// `Workspace`'s implementation is split by concern into these submodules; they share the
// imports above through `use super::*`.
mod actions;
mod chrome;
mod close;
mod palette;
mod pane_view;

pub use self::{actions::init, close::busy_everywhere};
use self::{
    chrome::{abbreviate_home, short_command},
    close::CloseTarget,
    palette::Modal,
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

/// Open a new tab in a folder (from a link's menu in the output).
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = workspace, no_json)]
pub struct OpenFolderInNewTab {
    pub dir: PathBuf,
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

    /// The active pane's terminal.
    fn terminal(&self, cx: &App) -> Entity<Terminal> {
        self.view().read(cx).terminal().clone()
    }

    /// What the active pane's shell reported: folder, running command, SSH session…
    fn shell<'a>(&self, cx: &'a App) -> &'a ShellState {
        self.view().read(cx).terminal().read(cx).shell()
    }
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
            // A broken config.json is reported as soon as there's a window to show it in.
            command_notice: config::error(cx).map(str::to_string),
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
            .and_then(|tab| tab.shell(cx).cwd.clone());
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
            .and_then(|tab| tab.shell(cx).cwd.clone())
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
        let cwd = tab.shell(cx).cwd.clone();
        let Some(pane) = self.new_pane(cwd, window, cx) else {
            return;
        };
        let id = pane.view.entity_id();
        let tab = self.active_tab_mut();
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
            self.active_tab_mut().zoomed = false;
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
                            cx,
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
            // With no editor, the system's default app; but a link in the output must not
            // run a program or a script, so those are only revealed.
            Launch::System if crate::links::may_run(&file.path) => {
                cx.reveal_path(&file.path);
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

    /// The active tab. There's always one while the window shows terminals: `render` shows
    /// `render_without_tabs` (with no actions) otherwise, and closing the last tab closes the
    /// window.
    fn active_tab(&self) -> &TabEntry {
        &self.tabs[self.active]
    }

    fn active_tab_mut(&mut self) -> &mut TabEntry {
        &mut self.tabs[self.active]
    }

    fn active_terminal(&self, cx: &App) -> Entity<Terminal> {
        self.active_tab().terminal(cx)
    }

    fn reconnect_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        let terminal = tab.terminal(cx);
        if terminal.update(cx, |t, cx| t.reconnect(cx)) {
            self.activate(ix, window, cx);
        }
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.tabs.get(self.active) {
            Some(tab) => tab.view().focus_handle(cx),
            None => self.confirm_focus.clone(),
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.tabs.is_empty() {
            return self.render_without_tabs(cx).into_any_element();
        }
        // Every visible change (tabs, panes, folders, the window) goes through here.
        let snapshot = self.snapshot(window, cx);
        session::update(self.entity_id, snapshot, cx);
        let root = v_flex()
            .relative()
            .size_full()
            .key_context("Workspace")
            .bg(cx.tokens().ui.background)
            .text_color(cx.tokens().ui.text)
            .font_family(UI_FONT_FAMILY)
            .text_size(UI_FONT_SIZE);
        Self::register_actions(root, cx)
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
            .into_any_element()
    }
}
