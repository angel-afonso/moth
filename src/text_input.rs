//! Campo de texto de una línea para los buscadores. El crate `ui` de Zed no trae uno
//! (Zed usa su editor), así que este cubre lo justo: cursor, IME, borrar, mover y pegar.

use std::ops::Range;

use gpui::{
    App, Bounds, Context, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    KeyBinding, Pixels, Point, ShapedLine, SharedString, TextRun, UTF16Selection, Window, actions,
    canvas, div, fill, point, prelude::*, px, size,
};

use crate::{
    theme::{UI_FONT_FAMILY, font},
    tokens::TokensExt,
};

actions!(
    text_input,
    [Backspace, Delete, Left, Right, Home, End, Paste, DeleteToStart, DeleteWordLeft]
);

pub fn init(cx: &mut App) {
    let ctx = Some("TextInput");
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("cmd-left", Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new("cmd-right", End, ctx),
        KeyBinding::new("cmd-v", Paste, ctx),
        KeyBinding::new("cmd-backspace", DeleteToStart, ctx),
        KeyBinding::new("ctrl-u", DeleteToStart, ctx),
        KeyBinding::new("alt-backspace", DeleteWordLeft, ctx),
        KeyBinding::new("ctrl-w", DeleteWordLeft, ctx),
    ]);
}

/// Se emite cada vez que cambia el texto.
pub struct Changed;

pub struct TextInput {
    focus_handle: FocusHandle,
    text: String,
    /// Posición del cursor en bytes.
    cursor: usize,
    marked: Option<Range<usize>>,
    placeholder: SharedString,
    font_size: Pixels,
}

impl EventEmitter<Changed> for TextInput {}

impl TextInput {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            text: String::new(),
            cursor: 0,
            marked: None,
            placeholder: placeholder.into(),
            font_size: px(14.),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.text = text.into();
        self.cursor = self.text.len();
        self.marked = None;
        cx.emit(Changed);
        cx.notify();
    }

    fn replace(&mut self, range: Range<usize>, new_text: &str, cx: &mut Context<Self>) {
        self.text.replace_range(range.clone(), new_text);
        self.cursor = range.start + new_text.len();
        cx.emit(Changed);
        cx.notify();
    }

    fn prev_boundary(&self, offset: usize) -> usize {
        self.text[..offset]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.text[offset..]
            .chars()
            .next()
            .map_or(offset, |c| offset + c.len_utf8())
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.cursor > 0 {
            let start = self.prev_boundary(self.cursor);
            self.replace(start..self.cursor, "", cx);
        }
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.cursor < self.text.len() {
            let end = self.next_boundary(self.cursor);
            self.replace(self.cursor..end, "", cx);
        }
    }

    fn delete_to_start(&mut self, _: &DeleteToStart, _: &mut Window, cx: &mut Context<Self>) {
        self.replace(0..self.cursor, "", cx);
    }

    fn delete_word_left(&mut self, _: &DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end();
        let start = trimmed
            .rfind(|c: char| c.is_whitespace() || c == '/')
            .map_or(0, |i| i + 1);
        self.replace(start..self.cursor, "", cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.prev_boundary(self.cursor);
        cx.notify();
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.next_boundary(self.cursor);
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = 0;
        cx.notify();
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.text.len();
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let line = text.lines().next().unwrap_or_default().to_string();
            self.replace(self.cursor..self.cursor, &line, cx);
        }
    }

    fn utf16_to_offset(&self, utf16: usize) -> usize {
        let mut count = 0;
        for (i, c) in self.text.char_indices() {
            if count >= utf16 {
                return i;
            }
            count += c.len_utf16();
        }
        self.text.len()
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.text[..offset.min(self.text.len())].encode_utf16().count()
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.utf16_to_offset(range.start)..self.utf16_to_offset(range.end)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.text[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let cursor = self.offset_to_utf16(self.cursor);
        Some(UTF16Selection {
            range: cursor..cursor,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|r| self.range_from_utf16(&r))
            .or(self.marked.take())
            .unwrap_or(self.cursor..self.cursor);
        // Una sola línea: los saltos se descartan.
        let new_text = new_text.replace(['\n', '\r'], "");
        self.replace(range, &new_text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|r| self.range_from_utf16(&r))
            .or(self.marked.clone())
            .unwrap_or(self.cursor..self.cursor);
        self.text.replace_range(range.clone(), new_text);
        self.marked = (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        self.cursor = range.start + new_text.len();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.text.is_empty();
        let text: SharedString = if empty {
            self.placeholder.clone()
        } else {
            self.text.clone().into()
        };
        let ui = &cx.tokens().ui;
        let color = if empty { ui.text_placeholder } else { ui.text };
        let accent = ui.accent;
        let cursor = if empty { 0 } else { self.cursor };
        let marked = self.marked.clone();
        let focused = self.focus_handle.is_focused(window);
        let font_size = self.font_size;
        let entity = cx.entity();
        let focus_handle = self.focus_handle.clone();

        div()
            .key_context("TextInput")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_to_start))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .flex_1()
            .h(px(22.))
            .child(
                canvas(
                    move |_, window, _| {
                        let len = text.len();
                        let underline = marked.map(|range| (range, color));
                        let mut runs = Vec::new();
                        let base = |len: usize| TextRun {
                            len,
                            font: font(UI_FONT_FAMILY),
                            color,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        match &underline {
                            Some((range, color)) if !empty && range.end <= len => {
                                if range.start > 0 {
                                    runs.push(base(range.start));
                                }
                                runs.push(TextRun {
                                    underline: Some(gpui::UnderlineStyle {
                                        thickness: px(1.),
                                        color: Some(*color),
                                        wavy: false,
                                    }),
                                    ..base(range.len())
                                });
                                if range.end < len {
                                    runs.push(base(len - range.end));
                                }
                            }
                            _ => runs.push(base(len)),
                        }
                        let line: ShapedLine =
                            window.text_system().shape_line(text, font_size, &runs, None);
                        let cursor_x = line.x_for_index(cursor);
                        (line, cursor_x)
                    },
                    move |bounds, (line, cursor_x), window, cx| {
                        line.paint(bounds.origin, bounds.size.height, gpui::TextAlign::Left, None, window, cx)
                            .ok();
                        if focused {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(bounds.origin.x + cursor_x, bounds.origin.y + px(2.)),
                                    size(px(1.5), bounds.size.height - px(4.)),
                                ),
                                accent,
                            ));
                        }
                        window.handle_input(
                            &focus_handle,
                            gpui::ElementInputHandler::new(bounds, entity),
                            cx,
                        );
                    },
                )
                .size_full(),
            )
    }
}
