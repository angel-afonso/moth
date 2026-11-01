//! Vista de una sesión: pinta la grilla de alacritty directamente con el text system de gpui.

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
    blocks, completions,
    components::{Kbd, KindBadge, SuggestionKind},
    glyphs,
    highlight::{Token, TokenKind},
    keys, links, mouse, palette,
    terminal::{EditLine, Listener, TermSize, Terminal},
    theme::{MONO_FONT_FAMILY, TERMINAL_FONT_SIZE, TERMINAL_LINE_HEIGHT, font},
    tokens::{SyntaxColors, TokensExt, metrics, tint},
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
        ShowCompletions
    ]
);

struct FontSize(f32);
impl gpui::Global for FontSize {}

pub struct OptionAsMeta(pub bool);
impl gpui::Global for OptionAsMeta {}

struct OutputSearch {
    input: Entity<crate::text_input::TextInput>,
    current: Option<SearchMatch>,
    message: String,
    _changed: Subscription,
}

/// Coincidencia de ⌘F, con el tamaño del scrollback de cuando se encontró.
#[derive(Clone)]
struct SearchMatch {
    range: std::ops::RangeInclusive<AlacPoint>,
    history_size: usize,
}

impl SearchMatch {
    fn new(range: std::ops::RangeInclusive<AlacPoint>, term: &Term<Listener>) -> Self {
        Self {
            range,
            history_size: term.grid().history_size(),
        }
    }

    /// Dónde está ahora: cada línea que entra al scrollback corre la grilla una fila hacia
    /// arriba. Si salió del scrollback o se limpió la pantalla, ya no está.
    fn resolve(&self, term: &Term<Listener>) -> Option<std::ops::RangeInclusive<AlacPoint>> {
        let shift = term.grid().history_size().checked_sub(self.history_size)? as i32;
        let moved = |point: &AlacPoint| AlacPoint::new(point.line - shift, point.column);
        let (start, end) = (moved(self.range.start()), moved(self.range.end()));
        (start.line >= term.topmost_line()).then_some(start..=end)
    }
}

fn literal_pattern(query: &str) -> String {
    let mut pattern = String::new();
    for c in query.chars() {
        if "\\.^$|?*+()[]{}".contains(c) {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern
}

pub struct TerminalView {
    terminal: Entity<Terminal>,
    search: Option<OutputSearch>,
    focus_handle: FocusHandle,
    /// Texto en composición del IME (p. ej. una tilde muerta antes de la vocal).
    marked_text: Option<String>,
    scroll_remainder: f32,
    /// Texto de varias líneas esperando confirmación antes de pegarse en producción.
    pending_paste: Option<String>,
    /// Geometría de la grilla del último frame, para traducir clics a celdas.
    geometry: Rc<Cell<Option<GridGeometry>>>,
    /// Link bajo el mouse: se subraya y el cursor pasa a ser una mano.
    hovered_link: Option<HoveredLink>,
    /// Link donde se apretó el botón; se abre si se suelta sobre el mismo.
    link_press: Option<LinkTarget>,
    /// Botón izquierdo apretado fuera de un link: punto de partida de la selección.
    press: Option<Press>,
    /// El clic izquierdo actual se reportó al programa (vim, htop…).
    reported_press: bool,
    /// Última celda reportada en movimiento, para no repetir eventos dentro de la misma celda.
    last_motion_cell: Option<(usize, usize)>,
    /// Popup de completions abierto.
    completion: Option<CompletionMenu>,
    /// Después de completar una carpeta, reabrir el popup cuando la línea quede así.
    reopen_completion_on: Option<String>,
    /// Bloque elegido con ⌘↑/⌘↓ (su fila de entrada): se resalta y ⌘⇧C copia su salida.
    selected_block: Option<blocks::Anchor>,
    _observe: Subscription,
}

const COMPLETION_ROWS: usize = 8;

struct CompletionMenu {
    completion: completions::Completion,
    selected: usize,
    /// Primera fila visible.
    top: usize,
    /// La línea con la que se calcularon los candidatos.
    line: EditLine,
}

impl CompletionMenu {
    fn select(&mut self, delta: isize) {
        let len = self.completion.items.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + COMPLETION_ROWS {
            self.top = self.selected + 1 - COMPLETION_ROWS;
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Press {
    point: AlacPoint,
    side: Side,
    /// Ya hay selección (por arrastre, doble/triple clic o Shift+clic).
    dragged: bool,
}

/// Adónde lleva un link: una URL (navegador) o un archivo (editor).
#[derive(Clone, Debug, PartialEq)]
enum LinkTarget {
    Url(String),
    File(links::FileLink),
}

#[derive(Clone, Debug, PartialEq)]
struct HoveredLink {
    target: LinkTarget,
    /// Celdas (línea, columna) en coordenadas de la grilla.
    cells: Vec<(i32, usize)>,
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
            link_press: None,
            press: None,
            reported_press: false,
            last_motion_cell: None,
            completion: None,
            reopen_completion_on: None,
            selected_block: None,
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

    fn find_output(&mut self, _: &FindOutput, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_none() {
            self.completion = None;
            let input = cx.new(|cx| crate::text_input::TextInput::new("Buscar en la salida…", cx));
            let changed = cx.subscribe(&input, |this, _, _: &crate::text_input::Changed, cx| {
                this.find_match(false, true, cx);
            });
            self.search = Some(OutputSearch {
                input,
                current: None,
                message: String::new(),
                _changed: changed,
            });
        }
        self.search
            .as_ref()
            .unwrap()
            .input
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    fn find_match(&mut self, backwards: bool, reset: bool, cx: &mut Context<Self>) {
        let Some(search) = &mut self.search else {
            return;
        };
        let query = search.input.read(cx).text().to_string();
        if query.is_empty() {
            search.current = None;
            search.message.clear();
            cx.notify();
            return;
        }
        let Ok(mut regex) = RegexSearch::new(&literal_pattern(&query)) else {
            search.current = None;
            search.message = "Búsqueda demasiado larga".into();
            cx.notify();
            return;
        };
        let term = self.terminal.read(cx).term.clone();
        let mut term = term.lock();
        let direction = if backwards {
            Direction::Left
        } else {
            Direction::Right
        };
        let origin = if !reset {
            search
                .current
                .as_ref()
                .and_then(|found| found.resolve(&term))
                .map(|found| {
                    if backwards {
                        found.start().sub(&*term, Boundary::None, 1)
                    } else {
                        found.end().add(&*term, Boundary::None, 1)
                    }
                })
        } else {
            None
        }
        .unwrap_or_else(|| {
            AlacPoint::new(term.topmost_line(), alacritty_terminal::index::Column(0))
        });
        let found = term.search_next(&mut regex, origin, direction, Side::Left, None);
        search.current = found.map(|range| SearchMatch::new(range, &term));
        search.message = if let Some(found) = &search.current {
            term.scroll_to_point(*found.range.start());
            "⏎ siguiente · ⇧⏎ anterior · esc cerrar".into()
        } else {
            "Sin coincidencias".into()
        };
        cx.notify();
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search = None;
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    /// ⌘↑/⌘↓: elige el comando anterior o siguiente y lo lleva al borde de arriba. Pasado el
    /// último vuelve al final de la salida.
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
                // Con el prompt en la primera fila visible (o lo más arriba que se pueda).
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

    /// ⌘⇧C: copia la salida del comando elegido, o la del último.
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
        // Shell terminada: ⏎ abre otra; los atajos con ⌘ (⌘W) siguen funcionando.
        if self.terminal.read(cx).has_exited() {
            if keystroke.key == "enter" && !(m.control || m.alt || m.shift || m.platform) {
                window.dispatch_action(Box::new(RestartShell), cx);
            }
            if !m.platform {
                cx.stop_propagation();
            }
            return;
        }
        // Con la confirmación de pegado abierta, el teclado no llega al servidor.
        if self.pending_paste.is_some() {
            match keystroke.key.as_str() {
                "escape" => self.cancel_paste(cx),
                "enter" if m.platform => self.confirm_paste(cx),
                _ if m.platform => return,
                _ => {}
            }
            cx.stop_propagation();
            return;
        }
        let plain = !(m.control || m.alt || m.shift || m.platform);

        // Con el popup abierto: flechas para elegir, Tab completa, ⏎ completa y ejecuta.
        // El resto de las teclas sigue a la shell y el popup se refiltra con la línea nueva.
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

        if plain
            && matches!(keystroke.key.as_str(), "right" | "end")
            && let Some(rest) = self.terminal.read(cx).suggestion(cx)
        {
            self.terminal
                .update(cx, |t, cx| t.input(rest.into_bytes(), cx));
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

    /// Celda de la grilla bajo `position` y de qué lado de ella cayó. Con `clamp`, un punto
    /// fuera de la pantalla se lleva al borde (para arrastrar una selección más allá).
    fn grid_point_side(
        &self,
        position: Point<Pixels>,
        term: &Term<Listener>,
        clamp: bool,
    ) -> Option<(AlacPoint, Side)> {
        let geometry = self.geometry.get()?;
        let mut column = (position.x - geometry.origin.x) / geometry.cell_width;
        let mut row = (position.y - geometry.origin.y) / geometry.line_height;
        let (columns, lines) = (term.columns() as f32, term.screen_lines() as f32);
        if clamp {
            column = column.clamp(0., columns - 0.01);
            row = row.clamp(0., lines - 0.01);
        } else if column < 0. || row < 0. || column >= columns || row >= lines {
            return None;
        }
        let side = if column.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        Some((
            AlacPoint::new(
                alacritty_terminal::index::Line(row as i32 - geometry.display_offset),
                alacritty_terminal::index::Column(column as usize),
            ),
            side,
        ))
    }

    fn grid_point(&self, position: Point<Pixels>, term: &Term<Listener>) -> Option<AlacPoint> {
        self.grid_point_side(position, term, false)
            .map(|(point, _)| point)
    }

    /// Link bajo el mouse. En la línea que se está escribiendo no hay links: ahí el clic
    /// mueve el cursor.
    fn link_under(&self, position: Point<Pixels>, cx: &App) -> Option<HoveredLink> {
        let terminal = self.terminal.read(cx);
        let term = terminal.term.clone();
        let term = term.lock();
        let point = self.grid_point(position, &term)?;
        if let Some(edit) = terminal
            .shell()
            .line
            .as_ref()
            .filter(|_| terminal.at_prompt())
            && locate_input(&term, edit).is_some_and(|cells| cells.contains(&point))
        {
            return None;
        }
        // En SSH las rutas son del servidor: no hay archivo local que abrir.
        let cwd = terminal
            .shell()
            .cwd
            .as_deref()
            .filter(|_| terminal.shell().remote.is_none());
        link_at(&term, point, cwd)
    }

    /// El programa pide el mouse (vim, htop…) y no se forzó la selección local con Shift.
    fn reports_mouse(&self, shift: bool, cx: &App) -> bool {
        let term = self.terminal.read(cx).term.clone();
        let term = term.lock();
        !shift && mouse::wants_mouse(*term.mode()) && term.grid().display_offset() == 0
    }

    /// Manda el evento al programa si su modo de mouse lo pide.
    fn send_mouse(
        &mut self,
        button: Option<mouse::Button>,
        action: mouse::Action,
        position: Point<Pixels>,
        modifiers: &Modifiers,
        cx: &mut Context<Self>,
    ) -> bool {
        let bytes = {
            let term = self.terminal.read(cx).term.clone();
            let term = term.lock();
            let Some((point, _)) = self.grid_point_side(position, &term, true) else {
                return false;
            };
            let mods = mouse::Mods {
                shift: modifiers.shift,
                alt: modifiers.alt,
                control: modifiers.control,
            };
            let line = (point.line.0 + term.grid().display_offset() as i32).max(0) as usize;
            if action == mouse::Action::Motion
                && self.last_motion_cell == Some((point.column.0, line))
            {
                return true;
            }
            self.last_motion_cell = Some((point.column.0, line));
            mouse::report(button, action, point.column.0, line, mods, *term.mode())
        };
        match bytes {
            Some(bytes) => {
                self.terminal.update(cx, |t, cx| t.input(bytes, cx));
                true
            }
            None => false,
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        let shift = event.modifiers.shift;
        if self.reports_mouse(shift, cx) {
            self.reported_press = self.send_mouse(
                Some(mouse::Button::Left),
                mouse::Action::Press,
                event.position,
                &event.modifiers,
                cx,
            );
            return;
        }
        // El clic que solo activa la ventana no abre links.
        if !event.first_mouse
            && !shift
            && event.click_count == 1
            && let Some(link) = self.link_under(event.position, cx)
        {
            self.link_press = Some(link.target);
            return;
        }

        let term = self.terminal.read(cx).term.clone();
        let mut term = term.lock();
        let Some((point, side)) = self.grid_point_side(event.position, &term, false) else {
            return;
        };
        let selection_type = match event.click_count {
            2 => Some(SelectionType::Semantic),
            3.. => Some(SelectionType::Lines),
            _ => None,
        };
        let mut dragged = true;
        match selection_type {
            // Doble clic: palabra; triple: línea. Arrastrar después extiende de a palabras/líneas.
            Some(ty) => term.selection = Some(Selection::new(ty, point, side)),
            // Shift+clic extiende la selección existente.
            None if shift && term.selection.is_some() => {
                if let Some(selection) = term.selection.as_mut() {
                    selection.update(point, side);
                }
            }
            None => {
                term.selection = None;
                dragged = false;
            }
        }
        drop(term);
        self.press = Some(Press {
            point,
            side,
            dragged,
        });
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button == Some(MouseButton::Left) {
            if self.reported_press {
                self.send_mouse(
                    Some(mouse::Button::Left),
                    mouse::Action::Motion,
                    event.position,
                    &event.modifiers,
                    cx,
                );
                return;
            }
            if let Some(mut press) = self.press {
                let term = self.terminal.read(cx).term.clone();
                let mut term = term.lock();
                let Some((point, side)) = self.grid_point_side(event.position, &term, true) else {
                    return;
                };
                if !press.dragged {
                    if point == press.point && side == press.side {
                        return;
                    }
                    term.selection = Some(Selection::new(
                        SelectionType::Simple,
                        press.point,
                        press.side,
                    ));
                    press.dragged = true;
                }
                if let Some(selection) = term.selection.as_mut() {
                    selection.update(point, side);
                }
                drop(term);
                self.press = Some(press);
                cx.notify();
                return;
            }
        }
        if event.pressed_button.is_none() && self.reports_mouse(event.modifiers.shift, cx) {
            self.send_mouse(
                None,
                mouse::Action::Motion,
                event.position,
                &event.modifiers,
                cx,
            );
        }
        let link = self.link_under(event.position, cx);
        if link != self.hovered_link {
            self.hovered_link = link;
            cx.notify();
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.reported_press) {
            self.send_mouse(
                Some(mouse::Button::Left),
                mouse::Action::Release,
                event.position,
                &event.modifiers,
                cx,
            );
            return;
        }
        if let Some(target) = self.link_press.take() {
            if self
                .link_under(event.position, cx)
                .is_some_and(|link| link.target == target)
            {
                match target {
                    LinkTarget::Url(url) => cx.open_url(&url),
                    LinkTarget::File(file) => {
                        window.dispatch_action(Box::new(crate::workspace::OpenFile { file }), cx)
                    }
                }
            }
            return;
        }
        match self.press.take() {
            Some(press) if press.dragged => {
                let term = self.terminal.read(cx).term.clone();
                let mut term = term.lock();
                if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
                    term.selection = None;
                }
            }
            Some(_) => self.move_edit_cursor(event.position, cx),
            None => {}
        }
    }

    /// Clic derecho o central: solo interesan a los programas que piden el mouse.
    fn other_button(
        &mut self,
        button: mouse::Button,
        action: mouse::Action,
        position: Point<Pixels>,
        modifiers: &Modifiers,
        cx: &mut Context<Self>,
    ) {
        if self.reports_mouse(modifiers.shift, cx) {
            self.send_mouse(Some(button), action, position, modifiers, cx);
        }
    }

    /// Clic en la línea que se está escribiendo: mueve el cursor de zsh hasta ahí, como en un
    /// editor, mandando las flechas necesarias.
    fn move_edit_cursor(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(geometry) = self.geometry.get() else {
            return;
        };
        let terminal = self.terminal.read(cx);
        let Some(edit) = terminal
            .shell()
            .line
            .clone()
            .filter(|_| terminal.at_prompt())
        else {
            return;
        };
        let column = (position.x - geometry.origin.x) / geometry.cell_width;
        let row = ((position.y - geometry.origin.y) / geometry.line_height).floor();
        if column < 0. || row < 0. {
            return;
        }
        let grid_line = row as i32 - geometry.display_offset;

        let term = terminal.term.clone();
        let term = term.lock();
        let Some(points) = locate_input(&term, &edit) else {
            return;
        };
        let cells: Vec<CharCell> = points
            .iter()
            .map(|p| CharCell {
                line: p.line.0,
                column: p.column.0,
                width: if term.grid()[*p].flags.contains(Flags::WIDE_CHAR) {
                    2
                } else {
                    1
                },
            })
            .collect();
        let app_cursor = term.mode().contains(TermMode::APP_CURSOR);
        drop(term);

        let Some(target) = click_target(&cells, grid_line, column) else {
            return;
        };
        let delta = target as isize - edit.cursor.min(cells.len()) as isize;
        if delta == 0 {
            return;
        }
        let arrow = match (delta > 0, app_cursor) {
            (true, true) => "\x1bOC",
            (true, false) => "\x1b[C",
            (false, true) => "\x1bOD",
            (false, false) => "\x1b[D",
        };
        let bytes = arrow.repeat(delta.unsigned_abs()).into_bytes();
        self.terminal.update(cx, |t, cx| t.input(bytes, cx));
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = match event.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => {
                let line_height = self
                    .geometry
                    .get()
                    .map_or(TERMINAL_LINE_HEIGHT, |g| g.line_height);
                p.y / line_height
            }
        };
        self.scroll_remainder += delta;
        let lines = self.scroll_remainder.trunc();
        self.scroll_remainder -= lines;
        if lines == 0. {
            return;
        }

        // Si el programa pide el mouse, la rueda va como eventos de rueda.
        if self.reports_mouse(event.modifiers.shift, cx) {
            let button = if lines > 0. {
                mouse::Button::WheelUp
            } else {
                mouse::Button::WheelDown
            };
            for _ in 0..lines.abs() as usize {
                self.send_mouse(
                    Some(button),
                    mouse::Action::Press,
                    event.position,
                    &event.modifiers,
                    cx,
                );
            }
            return;
        }

        let mode = self.terminal.read(cx).mode();
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            // En pantallas completas (less, vim) la rueda se traduce a flechas.
            let app = mode.contains(TermMode::APP_CURSOR);
            let key = match (lines > 0., app) {
                (true, true) => "\x1bOA",
                (true, false) => "\x1b[A",
                (false, true) => "\x1bOB",
                (false, false) => "\x1b[B",
            };
            let bytes = key.repeat(lines.abs() as usize).into_bytes();
            self.terminal.update(cx, |t, cx| t.input(bytes, cx));
        } else {
            self.terminal.update(cx, |t, cx| t.scroll(lines as i32, cx));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let production = self
            .terminal
            .read(cx)
            .shell()
            .remote
            .as_ref()
            .is_some_and(|remote| remote.identity.production);
        // Varias líneas en producción se ejecutarían una tras otra: se pide confirmación.
        if production && text.trim_end().contains('\n') {
            self.pending_paste = Some(text);
            cx.notify();
            return;
        }
        self.terminal.update(cx, |t, cx| t.paste(&text, cx));
    }

    fn confirm_paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.pending_paste.take() {
            self.terminal.update(cx, |t, cx| t.paste(&text, cx));
            cx.notify();
        }
    }

    fn cancel_paste(&mut self, cx: &mut Context<Self>) {
        self.pending_paste = None;
        cx.notify();
    }

    /// Tab: abre el popup. Con un solo candidato lo completa directo; sin candidatos devuelve
    /// `false` y el Tab sigue a la shell (su completion de siempre).
    fn open_completions(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(line) = self.terminal.read(cx).shell().line.clone() else {
            return false;
        };
        let Some(completion) = self.terminal.update(cx, |t, cx| t.complete(cx)) else {
            return false;
        };
        if completion.items.len() == 1 {
            let candidate = completion.items[0].clone();
            self.accept_completion(Some(candidate), false, cx);
            return true;
        }
        self.completion = Some(CompletionMenu {
            completion,
            selected: 0,
            top: 0,
            line,
        });
        cx.notify();
        true
    }

    /// Escribe el candidato (el elegido si no se pasa otro) en la línea; con `run`, ejecuta.
    fn accept_completion(
        &mut self,
        candidate: Option<completions::Candidate>,
        run: bool,
        cx: &mut Context<Self>,
    ) {
        let menu = self.completion.take();
        let Some(candidate) =
            candidate.or_else(|| menu.and_then(|m| m.completion.items.get(m.selected).cloned()))
        else {
            return;
        };
        let Some(line) = self.terminal.read(cx).shell().line.clone() else {
            return;
        };
        let cursor = line
            .buffer
            .char_indices()
            .nth(line.cursor)
            .map_or(line.buffer.len(), |(i, _)| i);
        let (buffer, new_cursor) = completions::apply(&line.buffer, cursor, &candidate);
        let back = buffer.chars().count().saturating_sub(new_cursor);
        let app_cursor = self.terminal.read(cx).mode().contains(TermMode::APP_CURSOR);
        self.terminal.update(cx, |t, cx| {
            t.insert_command(&buffer, run, cx);
            if !run && back > 0 {
                let left = if app_cursor { "\x1bOD" } else { "\x1b[D" };
                t.input(left.repeat(back).into_bytes(), cx);
            }
        });
        // Una carpeta sigue: al quedar escrita, se vuelve a abrir el popup con su contenido.
        if !run && candidate.text.ends_with('/') {
            self.reopen_completion_on = Some(buffer);
        }
        cx.notify();
    }

    /// Mantiene el popup al día con la línea que reporta la shell.
    fn sync_completion(&mut self, cx: &mut Context<Self>) {
        let terminal = self.terminal.read(cx);
        let line = terminal
            .shell()
            .line
            .clone()
            .filter(|_| terminal.at_prompt());
        let Some(line) = line else {
            self.completion = None;
            return;
        };
        if self.reopen_completion_on.as_ref() == Some(&line.buffer) {
            self.reopen_completion_on = None;
            self.open_completions(cx);
            return;
        }
        let Some(menu) = &self.completion else {
            return;
        };
        if menu.line == line {
            return;
        }
        let previous = menu
            .completion
            .items
            .get(menu.selected)
            .map(|c| c.label.clone());
        match self.terminal.update(cx, |t, cx| t.complete(cx)) {
            Some(completion) => {
                let selected = previous
                    .and_then(|label| completion.items.iter().position(|c| c.label == label))
                    .unwrap_or(0);
                let mut menu = CompletionMenu {
                    completion,
                    selected: 0,
                    top: 0,
                    line,
                };
                menu.select(selected as isize);
                self.completion = Some(menu);
            }
            None => self.completion = None,
        }
    }

    /// Lo que falta del candidato elegido para mostrarlo en gris en la línea.
    fn completion_ghost(&self) -> Option<String> {
        let menu = self.completion.as_ref()?;
        let candidate = menu.completion.items.get(menu.selected)?;
        let typed = &menu.completion.token;
        if candidate.kind == SuggestionKind::History {
            let line = &menu.line.buffer;
            return candidate
                .text
                .strip_prefix(line.as_str())
                .map(str::to_string);
        }
        candidate
            .text
            .strip_prefix(typed.as_str())
            .filter(|rest| !rest.is_empty())
            .map(str::to_string)
    }

    fn render_completion_menu(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let menu = self.completion.as_ref()?;
        let geometry = self.geometry.get()?;
        let ui = cx.tokens().ui.clone();
        let warning = cx.tokens().status.warning;

        // Ancla: la celda donde empieza la palabra que se completa.
        let anchor_cell = {
            let term = self.terminal.read(cx).term.clone();
            let term = term.lock();
            let replace_from = menu
                .completion
                .items
                .get(menu.selected)
                .map_or(0, |c| c.replace_from);
            let char_ix = menu
                .line
                .buffer
                .get(..replace_from)
                .map_or(0, |s| s.chars().count());
            locate_input(&term, &menu.line)
                .and_then(|cells| cells.get(char_ix).copied())
                .unwrap_or(term.grid().cursor.point)
        };
        let x = geometry.origin.x + geometry.cell_width * anchor_cell.column.0 as f32 - px(10.);
        let row_top = geometry.origin.y
            + geometry.line_height * (anchor_cell.line.0 + geometry.display_offset) as f32;
        let above = row_top > window.viewport_size().height / 2.;
        let (position, corner) = if above {
            (point(x, row_top - px(6.)), gpui::Anchor::BottomLeft)
        } else {
            (
                point(x, row_top + geometry.line_height + px(6.)),
                gpui::Anchor::TopLeft,
            )
        };

        let items = &menu.completion.items;
        let selected = items.get(menu.selected);
        let first_history = items.iter().position(|c| c.kind == SuggestionKind::History);
        let rows = items
            .iter()
            .enumerate()
            .skip(menu.top)
            .take(COMPLETION_ROWS)
            .map(|(ix, item)| {
                let is_selected = ix == menu.selected;
                v_flex()
                    .when(Some(ix) == first_history && ix > menu.top, |el| {
                        el.child(div().h(px(1.)).mx(px(8.)).my(px(3.)).bg(ui.divider))
                    })
                    .child(
                        h_flex()
                            .id(("completion", ix))
                            .h(px(30.))
                            .px(px(10.))
                            .gap(px(10.))
                            .rounded(metrics::RADIUS_ROW)
                            .cursor_pointer()
                            .when(is_selected, |row| row.bg(ui.element_selected))
                            .when(!is_selected, |row| {
                                row.hover(|style| style.bg(ui.element_hover))
                            })
                            .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                if let Some(menu) = &mut this.completion {
                                    menu.selected = ix;
                                }
                                this.accept_completion(None, false, cx);
                            }))
                            .child(KindBadge::new(item.kind))
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(px(260.))
                                    .overflow_hidden()
                                    .font_family(MONO_FONT_FAMILY)
                                    .child(
                                        HighlightedLabel::new(
                                            item.label.clone(),
                                            item.positions.clone(),
                                        )
                                        .color(if is_selected {
                                            Color::Default
                                        } else {
                                            Color::Muted
                                        })
                                        .truncate(),
                                    ),
                            )
                            .child(div().flex_1())
                            .when_some(item.description.clone(), |row, description| {
                                row.child(
                                    div().min_w_0().child(
                                        Label::new(description)
                                            .size(LabelSize::Small)
                                            .color(if is_selected {
                                                Color::Muted
                                            } else {
                                                Color::Placeholder
                                            })
                                            .truncate(),
                                    ),
                                )
                            }),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let hint = |key: &'static str, label: &'static str| {
            h_flex().gap(px(5.)).child(Kbd::new(key)).child(
                Label::new(label)
                    .size(LabelSize::XSmall)
                    .color(Color::Placeholder),
            )
        };
        let list = v_flex()
            .w(px(440.))
            .child(v_flex().p(px(6.)).gap(px(1.)).children(rows))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(px(12.))
                    .px(px(14.))
                    .py(px(7.))
                    .border_t_1()
                    .border_color(ui.divider)
                    .child(hint("↑↓", "navegar"))
                    .child(hint("Tab", "completar"))
                    .child(hint("⏎", "ejecutar"))
                    .child(hint("esc", "cerrar"))
                    .child(div().flex_1())
                    .child(
                        Label::new(format!("{} resultados", items.len()))
                            .size(LabelSize::XSmall)
                            .color(Color::Placeholder),
                    ),
            );

        let doc = selected.and_then(|c| c.doc.clone()).map(|doc| {
            v_flex()
                .w(px(300.))
                .gap(px(10.))
                .p(px(14.))
                .border_l_1()
                .border_color(ui.divider)
                .bg(ui.chrome)
                .child(
                    div()
                        .font_family(MONO_FONT_FAMILY)
                        .child(Label::new(doc.title).weight(gpui::FontWeight::SEMIBOLD)),
                )
                .when_some(doc.description, |el, d| {
                    el.child(Label::new(d).size(LabelSize::Small).color(Color::Muted))
                })
                .when_some(doc.usage, |el, usage| {
                    el.child(
                        div()
                            .px(px(10.))
                            .py(px(7.))
                            .rounded(metrics::RADIUS_ROW)
                            .bg(ui.background)
                            .border_1()
                            .border_color(ui.border)
                            .font_family(MONO_FONT_FAMILY)
                            .child(Label::new(usage).size(LabelSize::Small).color(Color::Muted)),
                    )
                })
                .children(doc.flags.into_iter().map(|(flag, description)| {
                    h_flex()
                        .gap(px(10.))
                        .child(
                            div()
                                .w(px(72.))
                                .flex_none()
                                .font_family(MONO_FONT_FAMILY)
                                .child(
                                    Label::new(flag)
                                        .size(LabelSize::Small)
                                        .color(Color::Custom(warning)),
                                ),
                        )
                        .child(
                            Label::new(description)
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .truncate(),
                        )
                }))
        });

        let popup = h_flex()
            .items_start()
            .bg(ui.surface)
            .border_1()
            .border_color(ui.border_strong)
            .rounded(px(10.))
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(list)
            .children(doc);

        Some(
            deferred(
                anchored()
                    .position(position)
                    .anchor(corner)
                    .snap_to_window_with_margin(px(8.))
                    .child(popup),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    fn render_paste_guard(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let text = self.pending_paste.as_ref()?;
        let terminal = self.terminal.read(cx);
        let remote = terminal.shell().remote.as_ref()?;
        let host_color = remote.identity.color(cx);
        let host = remote.target.alias.clone();
        let lines = text.trim_end().lines().count();
        let preview: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let ui = cx.tokens().ui.clone();

        Some(
            h_flex()
                .absolute()
                .left(metrics::SPACE_4)
                .right(metrics::SPACE_4)
                .bottom(px(12.))
                .gap(px(14.))
                .py(px(10.))
                .pl(px(14.))
                .pr(px(12.))
                .rounded(px(10.))
                .bg(ui.surface)
                .border_1()
                .border_color(tint(host_color, 0.45))
                .shadow_lg()
                .child(
                    Icon::new(IconName::Lock)
                        .size(IconSize::Small)
                        .color(Color::Custom(host_color)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(
                            h_flex()
                                .gap_1()
                                .child(Label::new(format!("¿Pegar {lines} líneas en")))
                                .child(
                                    h_flex()
                                        .child(
                                            Label::new(host)
                                                .color(Color::Custom(host_color))
                                                .weight(FontWeight::SEMIBOLD),
                                        )
                                        .child(Label::new("?")),
                                ),
                        )
                        .child(
                            div()
                                .font_family(MONO_FONT_FAMILY)
                                .text_size(px(12.))
                                .text_color(ui.text_placeholder)
                                .child(
                                    Label::new(preview)
                                        .size(LabelSize::Small)
                                        .color(Color::Placeholder)
                                        .truncate(),
                                ),
                        ),
                )
                .child(
                    Button::new("paste-cancel", "Cancelar")
                        .style(ButtonStyle::Outlined)
                        .size(ButtonSize::Large)
                        .label_size(LabelSize::Small)
                        .key_binding(ui::KeyBinding::for_action(&menu::Cancel, cx))
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_paste(cx))),
                )
                .child(
                    Button::new("paste-confirm", "Pegar igual")
                        .style(ButtonStyle::Tinted(TintColor::Error))
                        .size(ButtonSize::Large)
                        .label_size(LabelSize::Small)
                        .on_click(cx.listener(|this, _, _, cx| this.confirm_paste(cx))),
                )
                .into_any_element(),
        )
    }

    /// Aviso de que la shell terminó con error; la salida queda arriba para ver por qué.
    fn render_exit_banner(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let message = self.terminal.read(cx).exit_failure()?;
        let ui = cx.tokens().ui.clone();
        let error = cx.tokens().terminal.ansi.red;
        Some(
            h_flex()
                .absolute()
                .left(metrics::SPACE_4)
                .right(metrics::SPACE_4)
                .bottom(px(12.))
                .gap(px(14.))
                .py(px(10.))
                .pl(px(14.))
                .pr(px(12.))
                .rounded(px(10.))
                .bg(ui.surface)
                .border_1()
                .border_color(tint(error, 0.45))
                .shadow_lg()
                .child(
                    Icon::new(IconName::Warning)
                        .size(IconSize::Small)
                        .color(Color::Custom(error)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(Label::new(message))
                        .child(
                            Label::new("La salida de arriba puede explicar por qué.")
                                .size(LabelSize::Small)
                                .color(Color::Placeholder),
                        ),
                )
                .child(
                    Button::new("exited-close", "Cerrar")
                        .style(ButtonStyle::Outlined)
                        .size(ButtonSize::Large)
                        .label_size(LabelSize::Small)
                        .key_binding(ui::KeyBinding::for_action(&crate::workspace::CloseTab, cx))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(crate::workspace::CloseTab), cx)
                        }),
                )
                .child(
                    Button::new("exited-restart", "Abrir shell nueva ⏎")
                        .style(ButtonStyle::Filled)
                        .size(ButtonSize::Large)
                        .label_size(LabelSize::Small)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(RestartShell), cx)
                        }),
                )
                .into_any_element(),
        )
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
        // Sin selección todavía: copia la pantalla visible.
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

        div()
            .id("terminal-view")
            .relative()
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .size_full()
            .px(metrics::SPACE_7)
            .pt(metrics::SPACE_5)
            .pb(px(10.))
            .bg(cx.tokens().terminal.background)
            // En producción, un borde del color del host enmarca la terminal.
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
                // Sin nada que sugerir (o fuera del prompt), ⌃Space sigue a la shell.
                if !this.open_completions(cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(Some(1.), cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(Some(-1.), cx)))
            .on_action(cx.listener(|this, _: &ResetZoom, _, cx| this.zoom(None, cx)))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    this.other_button(
                        mouse::Button::Right,
                        mouse::Action::Press,
                        e.position,
                        &e.modifiers,
                        cx,
                    )
                }),
            )
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
                if !*hovered && this.hovered_link.take().is_some() {
                    cx.notify();
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
            .children(self.render_exit_banner(cx))
            .children(paste_guard)
            .children(completion_menu)
    }
}

#[derive(Clone, Copy, PartialEq)]
struct CellStyle {
    fg: Hsla,
    bold: bool,
    italic: bool,
    /// Color y si es ondulado.
    underline: Option<(Hsla, bool)>,
    strikethrough: bool,
}

struct Segment {
    line: usize,
    start_column: usize,
    next_column: usize,
    style: CellStyle,
    text: String,
}

struct GridLayout {
    backgrounds: Vec<PaintQuad>,
    glyph_quads: Vec<PaintQuad>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<PaintQuad>,
    cursor_text: Option<(Point<Pixels>, ShapedLine)>,
    cursor_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    geometry: GridGeometry,
}

impl GridLayout {
    fn paint(&self, window: &mut Window, cx: &mut App) {
        for quad in self.backgrounds.iter().chain(&self.glyph_quads) {
            window.paint_quad(quad.clone());
        }
        for (origin, line) in &self.lines {
            line.paint(*origin, self.line_height, TextAlign::Left, None, window, cx)
                .ok();
        }
        if let Some(cursor) = &self.cursor {
            window.paint_quad(cursor.clone());
        }
        if let Some((origin, line)) = &self.cursor_text {
            line.paint(*origin, self.line_height, TextAlign::Left, None, window, cx)
                .ok();
        }
    }
}

/// Barra en el margen izquierdo de los comandos que fallaron, y fondo y barra de acento en el
/// bloque elegido con ⌘↑/⌘↓. Solo mira las filas visibles y las de debajo.
fn block_decorations(
    term: &Term<Listener>,
    selected: Option<blocks::Anchor>,
    tokens: &crate::tokens::Tokens,
    origin: Point<Pixels>,
    width: Pixels,
    line_height: Pixels,
    display_offset: i32,
) -> Vec<PaintQuad> {
    let top = Line(-display_offset);
    let bottom = top + (term.screen_lines() as i32 - 1);
    let selected = selected.and_then(|anchor| anchor.resolve(term));
    let row_y = |line: Line| origin.y + line_height * (line.0 + display_offset) as f32;
    let mut quads = Vec::new();
    for block in blocks::visible_blocks(term, top, bottom) {
        let is_selected = !block.partial && Some(block.input) == selected;
        if !block.failed && !is_selected {
            continue;
        }
        let Some((start, end)) = blocks::trimmed_rows(term, &block) else {
            continue;
        };
        let (start, end) = (start.max(top), end.min(bottom));
        if start > end {
            continue;
        }
        let y = row_y(start);
        let height = row_y(end) + line_height - y;
        if is_selected {
            quads.push(fill(
                Bounds::new(point(origin.x - px(16.), y), size(width + px(24.), height)),
                tint(tokens.ui.accent, 0.08),
            ));
        }
        let bar = if block.failed {
            tokens.status.error
        } else {
            tokens.ui.accent
        };
        quads.push(
            fill(
                Bounds::new(
                    point(origin.x - px(14.), y + px(2.)),
                    size(px(3.), height - px(4.)),
                ),
                bar,
            )
            .corner_radii(px(1.5)),
        );
    }
    quads
}

/// La fuente de config.json o Geist Mono. Los íconos de Nerd Fonts que no estén en la fuente
/// salen de "Symbols Nerd Font" si está instalada.
fn terminal_font(cx: &App) -> Font {
    let family = crate::config::get(cx).font_family.clone();
    let mut fallbacks = vec![
        "Symbols Nerd Font Mono".to_string(),
        "Symbols Nerd Font".to_string(),
    ];
    if family.is_some() {
        fallbacks.insert(0, MONO_FONT_FAMILY.to_string());
    }
    Font {
        family: family
            .unwrap_or_else(|| MONO_FONT_FAMILY.to_string())
            .into(),
        fallbacks: Some(gpui::FontFallbacks::from_fonts(fallbacks)),
        ..font(MONO_FONT_FAMILY)
    }
}

fn styled_font(base: &Font, style: &CellStyle) -> Font {
    let mut font = base.clone();
    if style.bold {
        font.weight = FontWeight::BOLD;
    }
    if style.italic {
        font.style = FontStyle::Italic;
    }
    font
}

/// Lo que la vista superpone a la grilla en este frame.
struct Overlays {
    focused: bool,
    /// Texto en composición del IME.
    marked_text: Option<String>,
    /// Celdas del link bajo el mouse (se subrayan).
    link_cells: std::collections::HashSet<(i32, usize)>,
    /// Resto del candidato elegido en el popup de completions.
    completion_ghost: Option<String>,
    /// Bloque elegido con ⌘↑/⌘↓.
    selected_block: Option<blocks::Anchor>,
}

fn layout_grid(
    terminal: &Entity<Terminal>,
    bounds: Bounds<Pixels>,
    overlays: Overlays,
    font_size: Pixels,
    search_match: Option<SearchMatch>,
    window: &mut Window,
    cx: &mut App,
) -> GridLayout {
    let Overlays {
        focused,
        marked_text,
        link_cells,
        completion_ghost,
        selected_block,
    } = overlays;
    let base_font = terminal_font(cx);
    let line_height = TERMINAL_LINE_HEIGHT * (font_size / TERMINAL_FONT_SIZE);
    let text_system = window.text_system().clone();
    let font_id = text_system.resolve_font(&base_font);
    let cell_width = text_system
        .advance(font_id, font_size, 'm')
        .map(|s| s.width)
        .unwrap_or(px(8.));

    let columns = ((bounds.size.width / cell_width).floor() as usize).max(2);
    let lines = ((bounds.size.height / line_height).floor() as usize).max(1);
    let (input, suggestion) = terminal.update(cx, |t, cx| {
        t.resize(TermSize {
            columns,
            lines,
            cell_width,
            line_height,
        });
        // El candidato del popup de completions tiene prioridad sobre la sugerencia del historial.
        (
            t.highlighted_line(),
            completion_ghost.or_else(|| t.suggestion(cx)),
        )
    });

    let origin = bounds.origin;
    let cell_origin = |line: usize, column: usize| {
        point(
            origin.x + cell_width * column as f32,
            origin.y + line_height * line as f32,
        )
    };

    let shape = |text: String, style: &CellStyle| -> ShapedLine {
        let run = TextRun {
            len: text.len(),
            font: styled_font(&base_font, style),
            color: style.fg,
            background_color: None,
            underline: style.underline.map(|(color, wavy)| UnderlineStyle {
                thickness: px(1.),
                color: Some(color),
                wavy,
            }),
            strikethrough: style.strikethrough.then(|| StrikethroughStyle {
                thickness: px(1.),
                color: Some(style.fg),
            }),
        };
        text_system.shape_line(text.into(), font_size, &[run], Some(cell_width))
    };

    let tokens = cx.tokens().clone();
    let term = terminal.read(cx).term.clone();
    let term = term.lock();
    let content = term.renderable_content();
    let colors = content.colors;
    let overrides = |i: usize| colors[i];
    let display_offset = content.display_offset as i32;
    let background = tokens.terminal.background;
    let selection = content.selection;
    let search_match = search_match.and_then(|found| found.resolve(&term));

    // Celdas de la línea en edición → tipo de token. Solo si la grilla coincide con lo que
    // reportó zsh; si no (por ejemplo, durante un frame intermedio) no se colorea nada.
    let located = input
        .as_ref()
        .and_then(|(line, tokens)| locate_input(&term, line).map(|cells| (line, tokens, cells)));
    let input_cells: HashMap<(i32, usize), TokenKind> = located
        .as_ref()
        .map(|(line, tokens, cells)| token_cells(&line.buffer, tokens, cells))
        .unwrap_or_default();

    let mut backgrounds = block_decorations(
        &term,
        selected_block,
        &tokens,
        origin,
        bounds.size.width,
        line_height,
        display_offset,
    );
    // Bloques y líneas de caja dibujados como rectángulos, ajustados a píxeles del dispositivo
    // para que celdas vecinas compartan bordes exactos (sin costuras).
    let mut glyph_quads = Vec::new();
    let scale = window.scale_factor();
    let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
    let stroke = (f32::from(cell_width) * 0.12 * scale).round().max(1.) / scale;
    let mut shaped = Vec::new();
    let mut current: Option<Segment> = None;
    let flush = |segment: Option<Segment>, shaped: &mut Vec<(Point<Pixels>, ShapedLine)>| {
        if let Some(s) = segment {
            let at = cell_origin(s.line, s.start_column);
            shaped.push((at, shape(s.text, &s.style)));
        }
    };

    for indexed in content.display_iter {
        let cell = indexed.cell;
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        let line = (indexed.point.line.0 + display_offset).max(0) as usize;
        let column = indexed.point.column.0;
        let wide = cell.flags.contains(Flags::WIDE_CHAR);
        let width = if wide { 2 } else { 1 };

        let mut fg = palette::resolve(cell.fg, &overrides, &tokens.terminal);
        let mut bg = palette::resolve(cell.bg, &overrides, &tokens.terminal);
        let mut default_bg = matches!(cell.bg, AnsiColor::Named(NamedColor::Background));
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
            default_bg = false;
        }
        if cell.flags.contains(Flags::DIM) {
            fg = fg.opacity(0.7);
        }
        if cell.flags.contains(Flags::HIDDEN) {
            fg = transparent_black();
        }
        if !default_bg {
            backgrounds.push(fill(
                Bounds::new(
                    cell_origin(line, column),
                    size(cell_width * width as f32, line_height),
                ),
                bg,
            ));
        }
        // La coincidencia de ⌘F en amarillo, para no confundirla con la selección.
        let highlight = if search_match
            .as_ref()
            .is_some_and(|range| range.contains(&indexed.point))
        {
            Some(tint(tokens.terminal.ansi.yellow, 0.45))
        } else if selection.is_some_and(|range| range.contains(indexed.point)) {
            Some(tokens.terminal.selection)
        } else {
            None
        };
        if let Some(color) = highlight {
            backgrounds.push(fill(
                Bounds::new(
                    cell_origin(line, column),
                    size(cell_width * width as f32, line_height),
                ),
                color,
            ));
        }

        let style = CellStyle {
            fg,
            bold: cell.flags.contains(Flags::BOLD),
            italic: cell.flags.contains(Flags::ITALIC),
            underline: cell
                .flags
                .intersects(Flags::ALL_UNDERLINES)
                .then_some((fg, false)),
            strikethrough: cell.flags.contains(Flags::STRIKEOUT),
        };

        let mut style = style;
        if let Some(&kind) = input_cells.get(&(indexed.point.line.0, indexed.point.column.0)) {
            apply_token_style(&mut style, kind, &tokens.syntax, tokens.terminal.foreground);
        }
        if link_cells.contains(&(indexed.point.line.0, indexed.point.column.0)) {
            style.underline = Some((style.fg, false));
        }

        if let Some(rects) = glyphs::custom_glyph(
            cell.c,
            f32::from(cell_width) * width as f32,
            f32::from(line_height),
            stroke,
        ) {
            flush(current.take(), &mut shaped);
            let at = cell_origin(line, column);
            for r in rects {
                let top_left = point(snap(at.x + px(r.x0)), snap(at.y + px(r.y0)));
                let bottom_right = point(snap(at.x + px(r.x1)), snap(at.y + px(r.y1)));
                glyph_quads.push(fill(
                    Bounds::from_corners(top_left, bottom_right),
                    style.fg.opacity(r.alpha),
                ));
            }
            continue;
        }

        if cell.c == ' ' && style.underline.is_none() && !style.strikethrough {
            flush(current.take(), &mut shaped);
            continue;
        }

        let continues = current
            .as_ref()
            .is_some_and(|s| s.line == line && s.next_column == column && s.style == style);
        if !continues {
            flush(current.take(), &mut shaped);
            current = Some(Segment {
                line,
                start_column: column,
                next_column: column,
                style,
                text: String::new(),
            });
        }
        let segment = current.as_mut().unwrap();
        segment.text.push(cell.c);
        if let Some(zerowidth) = cell.zerowidth() {
            segment
                .text
                .extend(zerowidth.iter().filter(|&&c| !blocks::is_mark_char(c)));
        }
        segment.next_column = column + width;
        // Un carácter ancho cierra el segmento para que el siguiente arranque alineado a la grilla.
        if wide {
            flush(current.take(), &mut shaped);
        }
    }
    flush(current.take(), &mut shaped);

    // Sugerencia del historial, en gris a partir del cursor.
    let mut ghost_first = None;
    if let (Some(rest), Some((line, _, _))) = (suggestion, located.as_ref()) {
        let cursor_point = term.grid().cursor.point;
        let viewport_line = cursor_point.line.0 + display_offset;
        let free: usize = (cursor_point.column.0..columns)
            .take_while(|&col| {
                term.grid()
                    [AlacPoint::new(cursor_point.line, alacritty_terminal::index::Column(col))]
                .c == ' '
            })
            .count();
        let text: String = rest.chars().take(free).collect();
        if line.cursor == line.buffer.chars().count()
            && !text.is_empty()
            && (0..lines as i32).contains(&viewport_line)
        {
            ghost_first = text.chars().next();
            let style = CellStyle {
                fg: tokens.syntax.ghost,
                bold: false,
                italic: false,
                underline: None,
                strikethrough: false,
            };
            shaped.push((
                cell_origin(viewport_line as usize, cursor_point.column.0),
                shape(text, &style),
            ));
        }
    }

    // Cursor
    let accent = tokens.terminal.cursor;
    let mut cursor = None;
    let mut cursor_text = None;
    let mut cursor_bounds = None;
    let cursor_line = content.cursor.point.line.0 + display_offset;
    if (0..lines as i32).contains(&cursor_line) {
        let line = cursor_line as usize;
        let column = content.cursor.point.column.0;
        let cell =
            &term.grid()[AlacPoint::new(content.cursor.point.line, content.cursor.point.column)];
        let width = if cell.flags.contains(Flags::WIDE_CHAR) {
            2.
        } else {
            1.
        };
        let at = cell_origin(line, column);
        let block = Bounds::new(at, size(cell_width * width, line_height));
        cursor_bounds = Some(block);

        if let Some(marked) = marked_text.filter(|t| !t.is_empty()) {
            let style = CellStyle {
                fg: accent,
                bold: false,
                italic: false,
                underline: Some((accent, false)),
                strikethrough: false,
            };
            cursor_text = Some((at, shape(marked, &style)));
        } else {
            match (content.cursor.shape, focused) {
                (CursorShape::Hidden, _) => {}
                (_, false) | (CursorShape::HollowBlock, _) => {
                    cursor = Some(outline(block, accent, BorderStyle::Solid));
                }
                (CursorShape::Block, true) => {
                    cursor = Some(fill(block, accent));
                    let under_cursor = if cell.c != ' ' {
                        Some(cell.c)
                    } else {
                        ghost_first
                    };
                    if let Some(c) = under_cursor {
                        let style = CellStyle {
                            fg: background,
                            bold: cell.flags.contains(Flags::BOLD),
                            italic: cell.flags.contains(Flags::ITALIC),
                            underline: None,
                            strikethrough: false,
                        };
                        cursor_text = Some((at, shape(c.to_string(), &style)));
                    }
                }
                (CursorShape::Beam, true) => {
                    cursor = Some(fill(Bounds::new(at, size(px(2.), line_height)), accent));
                }
                (CursorShape::Underline, true) => {
                    let y = at.y + line_height - px(2.);
                    cursor = Some(fill(
                        Bounds::new(point(at.x, y), size(cell_width * width, px(2.))),
                        accent,
                    ));
                }
            }
        }
    }

    GridLayout {
        backgrounds,
        glyph_quads,
        lines: shaped,
        cursor,
        cursor_text,
        cursor_bounds,
        line_height,
        geometry: GridGeometry {
            origin,
            cell_width,
            line_height,
            display_offset,
        },
    }
}

/// Link en `point`: un hipervínculo OSC 8 de la celda, o una URL escrita en la línea lógica
/// (las filas partidas por el ancho de la terminal se unen). Solo esquemas seguros.
/// El link bajo `point`: un hipervínculo OSC 8, una URL escrita o, con `cwd`, una ruta a un
/// archivo que existe.
fn link_at<T>(term: &Term<T>, point: AlacPoint, cwd: Option<&Path>) -> Option<HoveredLink> {
    let grid = term.grid();
    let last_column = alacritty_terminal::index::Column(term.columns() - 1);

    if let Some(hyperlink) = grid[point].hyperlink() {
        let url = hyperlink.uri().to_string();
        if !links::is_openable(&url) {
            return None;
        }
        let same = |column: usize| {
            grid[AlacPoint::new(point.line, alacritty_terminal::index::Column(column))]
                .hyperlink()
                .is_some_and(|h| h.id() == hyperlink.id())
        };
        let start = (0..=point.column.0)
            .rev()
            .take_while(|&c| same(c))
            .last()
            .unwrap_or(point.column.0);
        let end = (point.column.0..term.columns())
            .take_while(|&c| same(c))
            .last()
            .unwrap_or(point.column.0);
        let cells = (start..=end).map(|c| (point.line.0, c)).collect();
        // `file://` local (`ls --hyperlink`, compiladores): al editor si es un archivo.
        let local_file = url
            .strip_prefix("file://")
            .and_then(|rest| rest.find('/').map(|slash| PathBuf::from(&rest[slash..])))
            .filter(|path| cwd.is_some() && path.is_file());
        let target = match local_file {
            Some(path) => LinkTarget::File(links::FileLink {
                path,
                line: None,
                column: None,
            }),
            None => LinkTarget::Url(url),
        };
        return Some(HoveredLink { target, cells });
    }

    let wraps = |line: alacritty_terminal::index::Line| {
        grid[AlacPoint::new(line, last_column)]
            .flags
            .contains(Flags::WRAPLINE)
    };
    let mut first = point.line;
    while first > term.topmost_line() && wraps(first - 1) {
        first -= 1;
    }
    let mut last = point.line;
    while last < term.bottommost_line() && wraps(last) {
        last += 1;
    }

    let mut text = String::new();
    let mut offsets: Vec<(usize, (i32, usize))> = Vec::new();
    let mut line = first;
    while line <= last {
        for column in 0..term.columns() {
            let cell = &grid[AlacPoint::new(line, alacritty_terminal::index::Column(column))];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            offsets.push((text.len(), (line.0, column)));
            text.push(cell.c);
        }
        line += 1;
    }

    // Una celda de relleno de un carácter ancho cuenta como el carácter.
    let hovered = offsets
        .iter()
        .rev()
        .find(|(_, (l, c))| *l == point.line.0 && *c <= point.column.0)
        .map(|(offset, _)| *offset)?;
    let url = links::find_urls(&text)
        .into_iter()
        .find(|r| r.contains(&hovered))
        .filter(|range| links::is_openable(&text[range.clone()]))
        .map(|range| (range.clone(), LinkTarget::Url(text[range].to_string())));
    let file = || {
        let cwd = cwd?;
        let found = links::find_paths(&text)
            .into_iter()
            .find(|m| m.range.contains(&hovered))?;
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let path = links::resolve_path(&found.path, cwd, home.as_deref())?;
        let file = links::FileLink {
            path,
            line: found.line,
            column: found.column,
        };
        Some((found.range, LinkTarget::File(file)))
    };
    let (range, target) = url.or_else(file)?;
    let cells = offsets
        .iter()
        .filter(|(offset, _)| range.contains(offset))
        .map(|(_, cell)| *cell)
        .collect();
    Some(HoveredLink { target, cells })
}

/// Celda de un carácter de la línea en edición: línea de la grilla, columna inicial y ancho.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CharCell {
    line: i32,
    column: usize,
    width: usize,
}

/// Dónde debería quedar el cursor (índice en caracteres) al hacer clic en `line`, `column`
/// (columna fraccional). Como en un editor: la mitad derecha de un carácter deja el cursor
/// después de él, y un clic más allá del final de una fila va al final de esa fila.
/// `None` si el clic no cae sobre la línea en edición (por ejemplo, sobre el prompt).
fn click_target(cells: &[CharCell], line: i32, column: f32) -> Option<usize> {
    let row: Vec<(usize, &CharCell)> = cells
        .iter()
        .enumerate()
        .filter(|(_, c)| c.line == line)
        .collect();
    let (first_ix, first) = *row.first()?;
    if column < first.column as f32 {
        // A la izquierda del texto: en una fila continuada, al inicio de esa fila; en la
        // primera fila es el prompt, y no se toca.
        return (first_ix > 0).then_some(first_ix);
    }
    for (ix, cell) in &row {
        let start = cell.column as f32;
        let end = start + cell.width as f32;
        if column >= start && column < end {
            let right_half = column - start >= cell.width as f32 / 2.;
            return Some(if right_half { ix + 1 } else { *ix });
        }
    }
    let (last_ix, _) = *row.last()?;
    Some(last_ix + 1)
}

/// Posición en la grilla de cada carácter de la línea en edición, retrocediendo desde el
/// cursor. `None` si las celdas no coinciden con el texto que reportó la shell.
fn locate_input(term: &Term<Listener>, line: &EditLine) -> Option<Vec<AlacPoint>> {
    let grid = term.grid();
    let chars: Vec<char> = line.buffer.chars().collect();
    let cursor = line.cursor.min(chars.len());
    let mut points = vec![AlacPoint::default(); chars.len()];

    let step_back = |point: AlacPoint| -> Option<AlacPoint> {
        let mut prev = point.sub(grid, Boundary::Grid, 1);
        if prev == point {
            return None;
        }
        if grid[prev]
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            let before = prev.sub(grid, Boundary::Grid, 1);
            if before == prev {
                return None;
            }
            prev = before;
        }
        Some(prev)
    };

    let mut point = grid.cursor.point;
    for i in (0..cursor).rev() {
        point = step_back(point)?;
        if grid[point].c != chars[i] {
            return None;
        }
        points[i] = point;
    }

    let mut point = grid.cursor.point;
    for i in cursor..chars.len() {
        if i > cursor {
            let width = if grid[point].flags.contains(Flags::WIDE_CHAR) {
                2
            } else {
                1
            };
            let next = point.add(grid, Boundary::Grid, width);
            if next == point {
                return None;
            }
            point = next;
            if grid[point].flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
                point = point.add(grid, Boundary::Grid, 1);
            }
        }
        if point.column.0 >= grid.columns() || grid[point].c != chars[i] {
            return None;
        }
        points[i] = point;
    }
    Some(points)
}

fn token_cells(
    buffer: &str,
    tokens: &[Token],
    cells: &[AlacPoint],
) -> HashMap<(i32, usize), TokenKind> {
    let mut map = HashMap::new();
    for (char_ix, (byte_ix, _)) in buffer.char_indices().enumerate() {
        if let Some(token) = tokens.iter().find(|t| t.range.contains(&byte_ix))
            && let Some(&point) = cells.get(char_ix)
        {
            map.insert((point.line.0, point.column.0), token.kind);
        }
    }
    map
}

fn apply_token_style(style: &mut CellStyle, kind: TokenKind, syntax: &SyntaxColors, text: Hsla) {
    style.fg = match kind {
        TokenKind::Command => syntax.command,
        TokenKind::UnknownCommand => syntax.error,
        TokenKind::Subcommand | TokenKind::Keyword => syntax.subcommand,
        TokenKind::Flag => syntax.flag,
        TokenKind::String => syntax.string,
        TokenKind::Path | TokenKind::ExistingPath => syntax.path,
        TokenKind::Operator => syntax.operator,
        TokenKind::Variable | TokenKind::Number => syntax.number,
        TokenKind::Comment => syntax.ghost,
        TokenKind::Plain => text,
    };
    style.underline = match kind {
        TokenKind::UnknownCommand => Some((syntax.error.opacity(0.7), true)),
        TokenKind::ExistingPath => Some((syntax.path.opacity(0.45), false)),
        _ => style.underline,
    };
}

struct TerminalInputHandler {
    view: Entity<TerminalView>,
    cursor_bounds: Option<Bounds<Pixels>>,
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut App,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&mut self, _: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.view
            .read(cx)
            .marked_text
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }

    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut App,
    ) {
        let bytes = text.as_bytes().to_vec();
        self.view.update(cx, |view, cx| {
            view.marked_text = None;
            view.selected_block = None;
            view.terminal.update(cx, |t, cx| t.input(bytes, cx));
            cx.notify();
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut App,
    ) {
        let text = (!new_text.is_empty()).then(|| new_text.to_string());
        self.view.update(cx, |view, cx| {
            view.marked_text = text;
            cx.notify();
        });
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut App) {
        self.view.update(cx, |view, cx| {
            view.marked_text = None;
            cx.notify();
        });
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.cursor_bounds
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<usize> {
        None
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        // Mantener una tecla debe repetirla, no abrir el selector de acentos de macOS.
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{CharCell, click_target};

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

    #[test]
    fn compiler_paths_link_to_the_file_and_urls_win() {
        use alacritty_terminal::index::{Column, Line, Point};
        let cwd = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let term = search_term(
            "  --> src/blocks.rs:12:5\r\nver https://a.dev/src/x.rs",
            40,
            4,
        );
        let link = super::link_at(&term, Point::new(Line(0), Column(10)), Some(cwd)).unwrap();
        assert_eq!(
            link.target,
            super::LinkTarget::File(crate::links::FileLink {
                path: cwd.join("src/blocks.rs"),
                line: Some(12),
                column: Some(5),
            })
        );
        assert_eq!(link.cells.len(), "src/blocks.rs:12:5".len());
        // Sin carpeta (SSH) no hay link de archivo; las URLs siguen.
        assert!(super::link_at(&term, Point::new(Line(0), Column(10)), None).is_none());
        let url = super::link_at(&term, Point::new(Line(1), Column(20)), Some(cwd)).unwrap();
        assert!(matches!(url.target, super::LinkTarget::Url(_)));
    }

    #[test]
    fn output_search_is_literal_and_finds_unicode_across_wrapped_rows() {
        use alacritty_terminal::{
            grid::Dimensions,
            index::{Column, Direction, Point, Side},
            term::search::RegexSearch,
        };
        let term = search_term("prefix ñ.[x] suffix", 10, 4);
        let mut regex = RegexSearch::new(&super::literal_pattern("ñ.[x]")).unwrap();
        let found = term
            .search_next(
                &mut regex,
                Point::new(term.topmost_line(), Column(0)),
                Direction::Right,
                Side::Left,
                None,
            )
            .unwrap();
        assert_eq!(found.start().column, Column(7));
        assert!(found.end().line > found.start().line);
        let mut regex = RegexSearch::new(&super::literal_pattern("ñ.[y]")).unwrap();
        assert!(
            term.search_next(
                &mut regex,
                Point::new(term.topmost_line(), Column(0)),
                Direction::Right,
                Side::Left,
                None
            )
            .is_none()
        );
    }

    #[test]
    fn output_search_finds_scrollback_and_wraps_both_directions() {
        use alacritty_terminal::{
            grid::Dimensions,
            index::{Boundary, Column, Direction, Point, Side},
            term::search::RegexSearch,
        };
        let term = search_term("match first\r\nother\r\nother\r\nmatch last", 20, 2);
        let mut regex = RegexSearch::new("match").unwrap();
        let first = term
            .search_next(
                &mut regex,
                Point::new(term.topmost_line(), Column(0)),
                Direction::Right,
                Side::Left,
                None,
            )
            .unwrap();
        assert!(first.start().line.0 < 0);
        let second = term
            .search_next(
                &mut regex,
                first.end().add(&term, Boundary::None, 1),
                Direction::Right,
                Side::Left,
                None,
            )
            .unwrap();
        assert!(second.start() > first.start());
        let wrapped = term
            .search_next(
                &mut regex,
                second.end().add(&term, Boundary::None, 1),
                Direction::Right,
                Side::Left,
                None,
            )
            .unwrap();
        assert_eq!(wrapped, first);
        let previous = term
            .search_next(
                &mut regex,
                first.start().sub(&term, Boundary::None, 1),
                Direction::Left,
                Side::Left,
                None,
            )
            .unwrap();
        assert_eq!(previous, second);
    }

    fn cells(line: i32, start: usize, widths: &[usize]) -> Vec<CharCell> {
        let mut column = start;
        widths
            .iter()
            .map(|&width| {
                let cell = CharCell {
                    line,
                    column,
                    width,
                };
                column += width;
                cell
            })
            .collect()
    }

    #[test]
    fn clicks_land_on_nearest_boundary() {
        // `git push` empezando en la columna 2 (después de "❯ ").
        let line = cells(5, 2, &[1; 8]);
        assert_eq!(click_target(&line, 5, 2.2), Some(0));
        assert_eq!(click_target(&line, 5, 2.7), Some(1));
        assert_eq!(click_target(&line, 5, 6.1), Some(4));
        // Más allá del final, al final.
        assert_eq!(click_target(&line, 5, 40.), Some(8));
        // Sobre el prompt o en otra fila: nada.
        assert_eq!(click_target(&line, 5, 0.5), None);
        assert_eq!(click_target(&line, 4, 3.), None);
    }

    #[test]
    fn handles_wide_chars_and_wrapped_rows() {
        // "a界b": el carácter ancho ocupa dos celdas.
        let line = cells(0, 2, &[1, 2, 1]);
        assert_eq!(click_target(&line, 0, 3.4), Some(1));
        assert_eq!(click_target(&line, 0, 4.2), Some(2));
        assert_eq!(click_target(&line, 0, 5.1), Some(2));
        assert_eq!(click_target(&line, 0, 5.7), Some(3));

        // Línea partida en dos filas: 4 caracteres en la fila 3 y 2 en la 4.
        let mut wrapped = cells(3, 76, &[1; 4]);
        wrapped.extend(cells(4, 0, &[1; 2]));
        assert_eq!(click_target(&wrapped, 4, 0.2), Some(4));
        assert_eq!(click_target(&wrapped, 4, 10.), Some(6));
    }

    #[test]
    fn empty_line_has_no_target() {
        assert_eq!(click_target(&[], 0, 3.), None);
    }
}
