//! View for a single session: paints the alacritty grid directly with gpui's text system.

use std::{
    cell::Cell,
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
};

use alacritty_terminal::{
    Term,
    grid::{Dimensions, Scroll},
    index::{Boundary, Direction, Line, Point as AlacPoint, Side},
    selection::{Selection, SelectionType},
    term::{TermMode, cell::Flags, search::RegexSearch},
    vte::ansi::{Color as AnsiColor, CursorShape, NamedColor},
};
use gpui::{
    App, BorderStyle, Bounds, ClipboardItem, Context, CursorStyle, Entity, FocusHandle, Focusable,
    Font, FontStyle, FontWeight, Hsla, InputHandler, KeyDownEvent, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ScrollDelta,
    ScrollWheelEvent, ShapedLine, StrikethroughStyle, Subscription, TextAlign, TextRun,
    UTF16Selection, UnderlineStyle, Window, actions, anchored, canvas, deferred, div, fill,
    outline, point, prelude::*, px, size, transparent_black,
};
use ui::{
    Button, ButtonCommon, ButtonSize, ButtonStyle, Clickable, Color, HighlightedLabel, Icon,
    IconName, IconSize, Label, LabelCommon, LabelSize, TintColor, h_flex, v_flex,
};

use crate::{
    agent,
    agent_mode::panel::{AgentModeEvent, AgentModePanel},
    agent_panel::{AgentPanel, AgentPanelEvent, AgentTask},
    blocks, completions,
    components::{Kbd, KindBadge},
    glyphs,
    highlight::{Token, TokenKind},
    keys, links, mouse, palette, shortcuts,
    terminal::{EditLine, Listener, TermSize, Terminal},
    theme::{MONO_FONT_FAMILY, TERMINAL_FONT_SIZE, TERMINAL_LINE_HEIGHT, font},
    tokens::{SyntaxColors, TokensExt, metrics, tint},
};

// `TerminalView`'s implementation is split by concern into these submodules; they share the
// imports above through `use super::*`.
mod completion_menu;
mod edit_line;
mod grid_layout;
mod input_handler;
mod link_hover;
mod link_menu;
mod output_search;
mod overlays;
mod pointer;

use self::{
    completion_menu::CompletionMenu,
    edit_line::{CharCell, click_target, locate_input, token_cells},
    grid_layout::{Overlays, layout_grid},
    input_handler::TerminalInputHandler,
    link_hover::{HoveredLink, LinkTarget, link_at},
    output_search::{OutputSearch, SearchMatch},
    pointer::Press,
};

actions!(
    terminal,
    [
        Paste,
        Copy,
        FindOutput,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        ToggleOptionAsMeta,
        RestartShell,
        PreviousCommand,
        NextCommand,
        CopyCommandOutput,
        ShowCompletions,
        AskAgent,
        FixWithAgent,
        ToggleAgentMode
    ]
);

struct FontSize(f32);
impl gpui::Global for FontSize {}

pub struct OptionAsMeta(pub bool);
impl gpui::Global for OptionAsMeta {}

pub struct TerminalView {
    terminal: Entity<Terminal>,
    search: Option<OutputSearch>,
    focus_handle: FocusHandle,
    /// IME composition text (e.g. a dead-key accent before the vowel).
    marked_text: Option<String>,
    scroll_remainder: f32,
    /// Multi-line text awaiting confirmation before being pasted into production.
    pending_paste: Option<String>,
    /// Grid geometry from the last frame, used to map clicks to cells.
    geometry: Rc<Cell<Option<GridGeometry>>>,
    /// Link under the mouse: underlined, and the cursor becomes a pointing hand.
    hovered_link: Option<HoveredLink>,
    /// The cell `hovered_link` was looked up for.
    hover_point: Option<AlacPoint>,
    /// Link where the button was pressed; opens if released over the same link.
    link_press: Option<LinkTarget>,
    /// Left button pressed outside a link: the selection's starting point.
    press: Option<Press>,
    /// Last pointer position while a text selection is being dragged. The timer uses it to
    /// extend the selection after scrolling past the top or bottom edge.
    selection_drag_position: Option<Point<Pixels>>,
    /// Keeps edge-scrolling alive while the mouse button is down; dropping it stops the task.
    selection_autoscroll: Option<gpui::Task<()>>,
    /// The current left click was reported to the program (vim, htop…).
    reported_press: bool,
    /// Last cell reported on motion, so we don't repeat events within the same cell.
    last_motion_cell: Option<(usize, usize)>,
    /// Open completions popup.
    completion: Option<CompletionMenu>,
    /// After completing a folder, reopen the popup once the line reads like this.
    reopen_completion_on: Option<String>,
    /// Command block selected with ⌘↑/⌘↓ (its input row): highlighted; ⌘⇧C copies its output.
    selected_block: Option<blocks::Anchor>,
    /// The agent's panel (⌘I, ⌘⇧I), at the prompt.
    agent_panel: Option<(Entity<AgentPanel>, Subscription)>,
    /// A link's right-click menu, and where it opened.
    link_menu: Option<(Entity<ui::ContextMenu>, Point<Pixels>, Subscription)>,
    /// Agent mode's conversation (⌘⇧A), beside the terminal.
    agent_mode: Option<(Entity<AgentModePanel>, Subscription)>,
    _observe: Subscription,
}

#[derive(Clone, Copy, Debug)]
struct GridGeometry {
    origin: Point<Pixels>,
    cell_width: Pixels,
    line_height: Pixels,
    display_offset: i32,
}

impl TerminalView {
    pub fn new(terminal: Entity<Terminal>, cx: &mut Context<Self>) -> Self {
        if cx.try_global::<FontSize>().is_none() {
            cx.set_global(FontSize(crate::state::saved_font_size()));
        }
        if cx.try_global::<OptionAsMeta>().is_none() {
            cx.set_global(OptionAsMeta(crate::state::option_as_meta()));
        }
        let observe = cx.observe(&terminal, |_, _, cx| cx.notify());
        Self {
            terminal,
            search: None,
            focus_handle: cx.focus_handle(),
            marked_text: None,
            scroll_remainder: 0.,
            pending_paste: None,
            geometry: Rc::new(Cell::new(None)),
            hovered_link: None,
            hover_point: None,
            link_press: None,
            press: None,
            selection_drag_position: None,
            selection_autoscroll: None,
            reported_press: false,
            last_motion_cell: None,
            completion: None,
            reopen_completion_on: None,
            selected_block: None,
            agent_panel: None,
            link_menu: None,
            agent_mode: None,
            _observe: observe,
        }
    }

    pub fn terminal(&self) -> &Entity<Terminal> {
        &self.terminal
    }

    fn zoom(&mut self, delta: Option<f32>, cx: &mut Context<Self>) {
        let current = cx.global::<FontSize>().0;
        let size = delta.map_or(13.5, |delta| (current + delta).clamp(8., 32.));
        cx.set_global(FontSize(size));
        crate::state::save_font_size(size);
        cx.refresh_windows();
    }

    /// ⌘↑/⌘↓: selects the previous or next command and scrolls it to the top edge. Past the
    /// last one, returns to the end of the output.
    fn select_command(&mut self, previous: bool, cx: &mut Context<Self>) {
        let term = self.terminal.read(cx).term.clone();
        let mut term = term.lock();
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        let commands = blocks::commands(&term);
        let current = self
            .selected_block
            .and_then(|anchor| anchor.resolve(&term))
            .and_then(|line| commands.iter().position(|block| block.input == line));
        let next = match (current, previous) {
            (None, true) => commands.len().checked_sub(1),
            (None, false) => None,
            (Some(ix), true) => Some(ix.saturating_sub(1)),
            (Some(ix), false) => Some(ix + 1).filter(|&next| next < commands.len()),
        };
        match next {
            Some(ix) => {
                let block = &commands[ix];
                self.selected_block = Some(blocks::Anchor::new(block.input, &term));
                let start =
                    blocks::trimmed_rows(&term, block).map_or(block.start, |(start, _)| start);
                // With the prompt on the first visible row (or as high up as possible).
                let offset = (-start.0).clamp(0, term.grid().history_size() as i32);
                let delta = offset - term.grid().display_offset() as i32;
                term.scroll_display(Scroll::Delta(delta));
            }
            None => {
                self.selected_block = None;
                term.scroll_display(Scroll::Bottom);
            }
        }
        cx.notify();
    }

    /// ⌘⇧C: copies the output of the selected command, or of the last one.
    fn copy_command_output(
        &mut self,
        _: &CopyCommandOutput,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = {
            let term = self.terminal.read(cx).term.clone();
            let term = term.lock();
            let commands = blocks::commands(&term);
            let selected = self.selected_block.and_then(|anchor| anchor.resolve(&term));
            commands
                .iter()
                .find(|block| Some(block.input) == selected)
                .or(commands.last())
                .map(|block| blocks::output_text(&term, block))
        };
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn toggle_option_as_meta(
        &mut self,
        _: &ToggleOptionAsMeta,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let enabled = !cx.global::<OptionAsMeta>().0;
        cx.set_global(OptionAsMeta(enabled));
        crate::state::save_option_as_meta(enabled);
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_handle.is_focused(window) {
            return;
        }
        let keystroke = &event.keystroke;
        let m = &keystroke.modifiers;
        // Shell exited: ⏎ opens another; app shortcuts (⌘W, Ctrl+Shift+W) keep working.
        if self.terminal.read(cx).has_exited() {
            if keystroke.key == "enter" && !(m.control || m.alt || m.shift || m.platform) {
                window.dispatch_action(Box::new(RestartShell), cx);
            }
            if !m.secondary() {
                cx.stop_propagation();
            }
            return;
        }
        // The confirmation only shows during the SSH session: once that's over, nothing is
        // waiting to be pasted, and keys must not be swallowed by a banner no one sees.
        if self.pending_paste.is_some() && self.terminal.read(cx).shell().remote.is_none() {
            self.pending_paste = None;
            cx.notify();
        }
        // While the paste confirmation is open, keystrokes don't reach the server.
        if self.pending_paste.is_some() {
            match keystroke.key.as_str() {
                "escape" => self.cancel_paste(cx),
                "enter" if m.secondary() => self.confirm_paste(cx),
                _ if m.secondary() => return,
                _ => {}
            }
            cx.stop_propagation();
            return;
        }
        let plain = !(m.control || m.alt || m.shift || m.platform);

        // With the popup open: arrows to pick, Tab completes, ⏎ completes and runs.
        // Other keys go to the shell and the popup refilters against the new line.
        if self.completion.is_some() {
            let handled = match (keystroke.key.as_str(), plain) {
                ("up", true) => {
                    if let Some(menu) = self.completion.as_mut() {
                        menu.select(-1);
                    }
                    true
                }
                ("down", true) => {
                    if let Some(menu) = self.completion.as_mut() {
                        menu.select(1);
                    }
                    true
                }
                ("tab", true) => {
                    self.accept_completion(None, false, cx);
                    true
                }
                ("tab", false) if m.shift && !m.control && !m.alt && !m.platform => {
                    if let Some(menu) = self.completion.as_mut() {
                        menu.select(-1);
                    }
                    true
                }
                ("enter", true) => {
                    self.accept_completion(None, true, cx);
                    true
                }
                ("escape", _) => {
                    self.completion = None;
                    true
                }
                _ => false,
            };
            if handled {
                cx.notify();
                cx.stop_propagation();
                return;
            }
        } else if plain
            && keystroke.key == "tab"
            && crate::config::get(cx).tab_completion == Some(crate::config::TabCompletion::Popup)
            && self.open_completions(cx)
        {
            cx.stop_propagation();
            return;
        }

        // The gray suggestion: → or End take all of it, ⌥→ its next word.
        let only_alt = m.alt && !(m.control || m.shift || m.platform);
        let whole = plain && matches!(keystroke.key.as_str(), "right" | "end");
        let word = only_alt && keystroke.key == "right";
        if (whole || word)
            && let Some(rest) = self.terminal.read(cx).suggestion(cx)
        {
            let accepted = if whole {
                rest
            } else {
                crate::shell::next_word(&rest).to_string()
            };
            self.terminal
                .update(cx, |t, cx| t.input(accepted.into_bytes(), cx));
            cx.stop_propagation();
            return;
        }

        let mode = self.terminal.read(cx).mode();
        let option_as_meta = cx.global::<OptionAsMeta>().0;
        if let Some(esc) = keys::to_esc_str(&event.keystroke, mode, option_as_meta) {
            self.selected_block = None;
            self.terminal
                .update(cx, |t, cx| t.input(esc.into_owned().into_bytes(), cx));
            cx.stop_propagation();
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let selected = self
            .terminal
            .read(cx)
            .term
            .lock()
            .selection_to_string()
            .map(|text| blocks::strip_marks(&text))
            .filter(|text| !text.is_empty());
        if let Some(text) = selected {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            return;
        }
        // No selection yet: copy the visible screen.
        let text = {
            let term = self.terminal.read(cx).term.clone();
            let term = term.lock();
            let content = term.renderable_content();
            let mut text = String::new();
            let mut last_line = None;
            for cell in content.display_iter {
                if last_line.is_some_and(|l| l != cell.point.line) {
                    text.truncate(text.trim_end_matches(' ').len());
                    text.push('\n');
                }
                last_line = Some(cell.point.line);
                if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    text.push(cell.c);
                }
            }
            text.trim_end().to_string()
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let terminal = self.terminal.clone();
        let focused = self.focus_handle.is_focused(window);
        let marked_text = self.marked_text.clone();
        let link_cells: std::collections::HashSet<(i32, usize)> = self
            .hovered_link
            .as_ref()
            .map(|link| link.cells.iter().copied().collect())
            .unwrap_or_default();
        let focus_handle = self.focus_handle.clone();
        let view = cx.entity();
        let production_color = self
            .terminal
            .read(cx)
            .shell()
            .remote
            .as_ref()
            .filter(|remote| remote.identity.production)
            .map(|remote| remote.identity.color(cx));
        self.sync_completion(cx);
        let completion_menu = self.render_completion_menu(window, cx);
        let completion_ghost = self.completion_ghost();
        let agent_panel = self.render_agent_panel(window, cx);
        let fix_hint = self.render_fix_hint(window, cx);
        let selected_block = self.selected_block;
        let paste_guard = self.render_paste_guard(cx);
        let geometry = self.geometry.clone();
        let font_size = px(cx.global::<FontSize>().0);
        let search_match = self
            .search
            .as_ref()
            .and_then(|search| search.current.clone());
        let search_bar = self.search.as_ref().map(|search| {
            v_flex()
                .absolute()
                .top(px(4.))
                .right(px(12.))
                .w(px(360.))
                .p(px(8.))
                .gap(px(8.))
                .bg(cx.tokens().ui.surface)
                .border_1()
                .border_color(cx.tokens().ui.border)
                .child(
                    h_flex()
                        .w_full()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .child(search.input.clone()),
                        )
                        .child(Button::new("previous-output-match", "↑").on_click(
                            cx.listener(|this, _, _, cx| this.find_match(true, false, cx)),
                        ))
                        .child(Button::new("next-output-match", "↓").on_click(
                            cx.listener(|this, _, _, cx| this.find_match(false, false, cx)),
                        ))
                        .child(
                            Button::new("close-output-search", "×")
                                .on_click(cx.listener(|this, _, w, cx| this.close_search(w, cx))),
                        ),
                )
                .child(Label::new(search.message.clone()).size(LabelSize::Small))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        if let Some(search) = &this.search {
                            search.input.focus_handle(cx).focus(window, cx);
                        }
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    match event.keystroke.key.as_str() {
                        "escape" => this.close_search(window, cx),
                        "enter" => this.find_match(event.keystroke.modifiers.shift, false, cx),
                        _ => return,
                    }
                    cx.stop_propagation();
                }))
        });

        let terminal_area = div()
            .id("terminal-view")
            .relative()
            .overflow_hidden()
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .size_full()
            .px(metrics::SPACE_7)
            .pt(metrics::SPACE_5)
            .pb(px(10.))
            .bg(cx.tokens().terminal.background)
            // In production, a border in the host's color frames the terminal.
            .when_some(production_color, |el, color| {
                el.border_1()
                    .border_color(tint(color, 0.22))
                    .rounded(px(8.))
                    .px(px(20.))
                    .pt(px(12.))
            })
            .cursor(if self.hovered_link.is_some() {
                CursorStyle::PointingHand
            } else {
                CursorStyle::IBeam
            })
            .on_key_down(cx.listener(Self::key_down))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .on_action(cx.listener(Self::find_output))
            .on_action(cx.listener(Self::toggle_option_as_meta))
            .on_action(
                cx.listener(|this, _: &PreviousCommand, _, cx| this.select_command(true, cx)),
            )
            .on_action(cx.listener(|this, _: &NextCommand, _, cx| this.select_command(false, cx)))
            .on_action(cx.listener(Self::copy_command_output))
            .on_action(cx.listener(|this, _: &ShowCompletions, _, cx| {
                // With nothing to suggest (or outside the prompt), ⌃Space goes to the shell.
                if !this.open_completions(cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &AskAgent, window, cx| {
                this.open_agent(AgentTask::Ask, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FixWithAgent, window, cx| {
                this.open_agent(AgentTask::Fix, window, cx)
            }))
            .on_action(cx.listener(Self::toggle_agent_mode))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(Some(1.), cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(Some(-1.), cx)))
            .on_action(cx.listener(|this, _: &ResetZoom, _, cx| this.zoom(None, cx)))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::right_mouse_down))
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, e: &MouseUpEvent, _, cx| {
                    this.other_button(
                        mouse::Button::Right,
                        mouse::Action::Release,
                        e.position,
                        &e.modifiers,
                        cx,
                    )
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    this.other_button(
                        mouse::Button::Middle,
                        mouse::Action::Press,
                        e.position,
                        &e.modifiers,
                        cx,
                    )
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|this, e: &MouseUpEvent, _, cx| {
                    this.other_button(
                        mouse::Button::Middle,
                        mouse::Action::Release,
                        e.position,
                        &e.modifiers,
                        cx,
                    )
                }),
            )
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered {
                    this.hover_point = None;
                    if this.hovered_link.take().is_some() {
                        cx.notify();
                    }
                }
            }))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let overlays = Overlays {
                            focused,
                            marked_text,
                            link_cells,
                            completion_ghost,
                            selected_block,
                        };
                        let layout = layout_grid(
                            &terminal,
                            bounds,
                            overlays,
                            font_size,
                            search_match,
                            window,
                            cx,
                        );
                        geometry.set(Some(layout.geometry));
                        layout
                    },
                    move |_, layout, window, cx| {
                        layout.paint(window, cx);
                        window.handle_input(
                            &focus_handle,
                            TerminalInputHandler {
                                view,
                                cursor_bounds: layout.cursor_bounds,
                            },
                            cx,
                        );
                    },
                )
                .size_full(),
            )
            .children(search_bar)
            .children(self.render_missing_cwd_banner(cx))
            .children(self.render_exit_banner(cx))
            .children(paste_guard)
            .children(completion_menu)
            .children(fix_hint)
            .children(agent_panel)
            .children(self.render_link_menu());
        match &self.agent_mode {
            Some((panel, _)) => h_flex()
                .size_full()
                .child(terminal_area.flex_1().min_w_0())
                .child(panel.clone())
                .into_any_element(),
            None => terminal_area.into_any_element(),
        }
    }
}

#[cfg(test)]
fn search_term(
    text: &str,
    columns: usize,
    lines: usize,
) -> alacritty_terminal::Term<alacritty_terminal::event::VoidListener> {
    let size = crate::terminal::TermSize {
        columns,
        lines,
        cell_width: gpui::px(8.),
        line_height: gpui::px(22.),
    };
    let mut term = alacritty_terminal::Term::new(
        Default::default(),
        &size,
        alacritty_terminal::event::VoidListener,
    );
    let mut parser: alacritty_terminal::vte::ansi::Processor =
        alacritty_terminal::vte::ansi::Processor::new();
    parser.advance(&mut term, text.as_bytes());
    term
}
