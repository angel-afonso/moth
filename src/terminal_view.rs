//! Vista de una sesión: pinta la grilla de alacritty directamente con el text system de gpui.

use std::{cell::Cell, collections::HashMap, ops::Range, rc::Rc};

use alacritty_terminal::{
    Term,
    grid::Dimensions,
    index::{Boundary, Point as AlacPoint},
    term::{TermMode, cell::Flags},
    vte::ansi::{Color as AnsiColor, CursorShape, NamedColor},
};
use gpui::{
    App, BorderStyle, Bounds, ClipboardItem, Context, CursorStyle, Entity, FocusHandle, Focusable,
    Font, FontStyle, FontWeight, Hsla, InputHandler, KeyDownEvent, MouseButton, MouseDownEvent,
    PaintQuad, Pixels,
    Point, ScrollDelta, ScrollWheelEvent, ShapedLine, StrikethroughStyle, Subscription, TextAlign,
    TextRun, UTF16Selection, UnderlineStyle, Window, actions, canvas, div, fill, outline,
    prelude::*, point, px, size, transparent_black,
};
use ui::{
    Button, ButtonCommon, ButtonSize, ButtonStyle, Clickable, Color, Icon, IconName, IconSize,
    Label, LabelCommon, LabelSize, TintColor, h_flex, v_flex,
};

use crate::{
    highlight::{Token, TokenKind},
    keys, palette,
    tokens::{SyntaxColors, TokensExt, metrics, tint},
    terminal::{EditLine, Listener, TermSize, Terminal},
    theme::{MONO_FONT_FAMILY, TERMINAL_FONT_SIZE, TERMINAL_LINE_HEIGHT, font},
};

actions!(terminal, [Paste, Copy]);

pub struct TerminalView {
    terminal: Entity<Terminal>,
    focus_handle: FocusHandle,
    /// Texto en composición del IME (p. ej. una tilde muerta antes de la vocal).
    marked_text: Option<String>,
    scroll_remainder: f32,
    /// Texto de varias líneas esperando confirmación antes de pegarse en producción.
    pending_paste: Option<String>,
    /// Geometría de la grilla del último frame, para traducir clics a celdas.
    geometry: Rc<Cell<Option<GridGeometry>>>,
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
        let observe = cx.observe(&terminal, |_, _, cx| cx.notify());
        Self {
            terminal,
            focus_handle: cx.focus_handle(),
            marked_text: None,
            scroll_remainder: 0.,
            pending_paste: None,
            geometry: Rc::new(Cell::new(None)),
            _observe: observe,
        }
    }

    pub fn terminal(&self) -> &Entity<Terminal> {
        &self.terminal
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let m = &keystroke.modifiers;
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
        if plain && matches!(keystroke.key.as_str(), "right" | "end")
            && let Some(rest) = self.terminal.read(cx).suggestion(cx) {
                self.terminal
                    .update(cx, |t, cx| t.input(rest.into_bytes(), cx));
                cx.stop_propagation();
                return;
            }

        let mode = self.terminal.read(cx).mode();
        if let Some(esc) = keys::to_esc_str(&event.keystroke, mode) {
            self.terminal
                .update(cx, |t, cx| t.input(esc.into_owned().into_bytes(), cx));
            cx.stop_propagation();
        }
    }

    /// Clic en la línea que se está escribiendo: mueve el cursor de zsh hasta ahí, como en un
    /// editor, mandando las flechas necesarias.
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        let Some(geometry) = self.geometry.get() else {
            return;
        };
        let terminal = self.terminal.read(cx);
        let Some(edit) = terminal.shell().line.clone().filter(|_| terminal.at_prompt()) else {
            return;
        };
        let column = (event.position.x - geometry.origin.x) / geometry.cell_width;
        let row = ((event.position.y - geometry.origin.y) / geometry.line_height).floor();
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
                width: if term.grid()[*p].flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 },
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
            ScrollDelta::Pixels(p) => p.y / TERMINAL_LINE_HEIGHT,
        };
        self.scroll_remainder += delta;
        let lines = self.scroll_remainder.trunc();
        self.scroll_remainder -= lines;
        if lines == 0. {
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
                .child(Icon::new(IconName::Lock).size(IconSize::Small).color(Color::Custom(host_color)))
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
                                        .child(Label::new(host).color(Color::Custom(host_color)).weight(FontWeight::SEMIBOLD))
                                        .child(Label::new("?")),
                                ),
                        )
                        .child(
                            div()
                                .font_family(MONO_FONT_FAMILY)
                                .text_size(px(12.))
                                .text_color(ui.text_placeholder)
                                .child(Label::new(preview).size(LabelSize::Small).color(Color::Placeholder).truncate()),
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

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
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
        let paste_guard = self.render_paste_guard(cx);
        let geometry = self.geometry.clone();

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
                el.border_1().border_color(tint(color, 0.22)).rounded(px(8.)).px(px(20.)).pt(px(12.))
            })
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::key_down))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let layout = layout_grid(&terminal, bounds, focused, marked_text, window, cx);
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
            .children(paste_guard)
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
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<PaintQuad>,
    cursor_text: Option<(Point<Pixels>, ShapedLine)>,
    cursor_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    geometry: GridGeometry,
}

impl GridLayout {
    fn paint(&self, window: &mut Window, cx: &mut App) {
        for quad in &self.backgrounds {
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

fn layout_grid(
    terminal: &Entity<Terminal>,
    bounds: Bounds<Pixels>,
    focused: bool,
    marked_text: Option<String>,
    window: &mut Window,
    cx: &mut App,
) -> GridLayout {
    let base_font = font(MONO_FONT_FAMILY);
    let font_size = TERMINAL_FONT_SIZE;
    let line_height = TERMINAL_LINE_HEIGHT;
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
        (t.highlighted_line(), t.suggestion(cx))
    });

    let origin = bounds.origin;
    let cell_origin =
        |line: usize, column: usize| point(origin.x + cell_width * column as f32, origin.y + line_height * line as f32);

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

    // Celdas de la línea en edición → tipo de token. Solo si la grilla coincide con lo que
    // reportó zsh; si no (por ejemplo, durante un frame intermedio) no se colorea nada.
    let located = input
        .as_ref()
        .and_then(|(line, tokens)| locate_input(&term, line).map(|cells| (line, tokens, cells)));
    let input_cells: HashMap<(i32, usize), TokenKind> = located
        .as_ref()
        .map(|(line, tokens, cells)| token_cells(&line.buffer, tokens, cells))
        .unwrap_or_default();

    let mut backgrounds = Vec::new();
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
                Bounds::new(cell_origin(line, column), size(cell_width * width as f32, line_height)),
                bg,
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

        if cell.c == ' ' && style.underline.is_none() && !style.strikethrough {
            flush(current.take(), &mut shaped);
            continue;
        }

        let continues = current.as_ref().is_some_and(|s| {
            s.line == line && s.next_column == column && s.style == style
        });
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
            segment.text.extend(zerowidth);
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
                term.grid()[AlacPoint::new(cursor_point.line, alacritty_terminal::index::Column(col))].c == ' '
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
        let cell = &term.grid()[AlacPoint::new(content.cursor.point.line, content.cursor.point.column)];
        let width = if cell.flags.contains(Flags::WIDE_CHAR) { 2. } else { 1. };
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
                    let under_cursor = if cell.c != ' ' { Some(cell.c) } else { ghost_first };
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
    let row: Vec<(usize, &CharCell)> = cells.iter().enumerate().filter(|(_, c)| c.line == line).collect();
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
            let width = if grid[point].flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
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

    fn cells(line: i32, start: usize, widths: &[usize]) -> Vec<CharCell> {
        let mut column = start;
        widths
            .iter()
            .map(|&width| {
                let cell = CharCell { line, column, width };
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
