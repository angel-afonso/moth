//! Turns the grid into what gets painted: shaped text runs, backgrounds, the cursor,
//! selection, links, block decorations and pictures drawn with block characters.

use super::*;

#[derive(Clone, Copy, PartialEq)]
struct CellStyle {
    fg: Hsla,
    bold: bool,
    italic: bool,
    /// Color, and whether it's wavy.
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

pub(super) struct GridLayout {
    backgrounds: Vec<PaintQuad>,
    glyph_quads: Vec<PaintQuad>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<PaintQuad>,
    cursor_text: Option<(Point<Pixels>, ShapedLine)>,
    pub(super) cursor_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    pub(super) geometry: GridGeometry,
}

impl GridLayout {
    pub(super) fn paint(&self, window: &mut Window, cx: &mut App) {
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

/// Left-margin bar for failed commands, plus background and accent bar for the
/// command block selected with ⌘↑/⌘↓. Only looks at visible rows and those below.
/// Where each visible row is drawn when it holds only a picture made of blocks or sextants
/// (a QR code, three rows or more): its top, from the grid's origin, and its height. These
/// pictures are made for cells twice as tall as wide and ours are taller, which stretches
/// them, so their rows are drawn shorter from the picture's top down and a QR code stays
/// square. The text around keeps its place in the grid. `None` for every other row.
fn picture_rows<T>(
    term: &Term<T>,
    lines: usize,
    display_offset: i32,
    cell_width: f32,
    line_height: f32,
) -> Vec<Option<(f32, f32)>> {
    const MIN_ROWS: i32 = 3;
    // Rows above the screen count too, so a picture half scrolled out doesn't move.
    const MAX_ROWS: i32 = 200;
    let grid = term.grid();
    let first = -(grid.history_size() as i32);
    let last = grid.screen_lines() as i32 - 1;
    let mut memo = HashMap::new();
    let mut grain = |line: i32| -> Option<glyphs::Grain> {
        if !(first..=last).contains(&line) {
            return None;
        }
        *memo.entry(line).or_insert_with(|| {
            let row = &grid[Line(line)];
            let mut finest = None;
            for column in 0..grid.columns() {
                let c = row[alacritty_terminal::index::Column(column)].c;
                if c != ' ' {
                    finest = finest.max(Some(glyphs::grain(c)?));
                }
            }
            finest
        })
    };

    let mut rows = vec![None; lines];
    let mut visible = 0;
    while visible < lines {
        let line = visible as i32 - display_offset;
        let Some(mut finest) = grain(line) else {
            visible += 1;
            continue;
        };
        let mut top = line;
        while line - top < MAX_ROWS
            && let Some(above) = grain(top - 1)
        {
            finest = finest.max(above);
            top -= 1;
        }
        let mut bottom = line;
        while bottom - line < MAX_ROWS
            && let Some(below) = grain(bottom + 1)
        {
            finest = finest.max(below);
            bottom += 1;
        }
        let height = glyphs::picture_row_height(cell_width, line_height, finest);
        if bottom - top + 1 >= MIN_ROWS && height < line_height {
            let top_y = (top + display_offset) as f32 * line_height;
            for row in line..=bottom {
                let at = (row + display_offset) as usize;
                if at < lines {
                    rows[at] = Some((top_y + (row - top) as f32 * height, height));
                }
            }
        }
        visible = (bottom + display_offset + 1).max(visible as i32 + 1) as usize;
    }
    rows
}

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

/// The font from config.json, or Geist Mono. Nerd Fonts icons missing from the font
/// come from "Symbols Nerd Font" if it's installed.
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

/// What the view overlays on the grid this frame.
pub(super) struct Overlays {
    pub(super) focused: bool,
    /// IME composition text.
    pub(super) marked_text: Option<String>,
    /// Cells of the link under the mouse (underlined).
    pub(super) link_cells: std::collections::HashSet<(i32, usize)>,
    /// Rest of the selected candidate in the completions popup.
    pub(super) completion_ghost: Option<String>,
    /// Command block selected with ⌘↑/⌘↓.
    pub(super) selected_block: Option<blocks::Anchor>,
}

pub(super) fn layout_grid(
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
        // The completions popup candidate takes priority over the history suggestion.
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

    // Cells of the line being edited → token kind. Only if the grid matches what zsh
    // reported; otherwise (e.g. during an intermediate frame) nothing is colored.
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
    // Block and box-drawing characters drawn as rectangles, snapped to device pixels
    // so neighboring cells share exact edges (no seams).
    let mut glyph_quads = Vec::new();
    let scale = window.scale_factor();
    let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
    let stroke = (f32::from(cell_width) * 0.12 * scale).round().max(1.) / scale;
    let pictures = picture_rows(
        &term,
        lines,
        display_offset,
        f32::from(cell_width),
        f32::from(line_height),
    );
    // A cell's own rectangle, or the shorter one of its picture row.
    let cell_box = |line: usize, column: usize, width: usize| {
        let at = cell_origin(line, column);
        let width = cell_width * width as f32;
        match pictures.get(line).copied().flatten() {
            Some((top, height)) => {
                Bounds::new(point(at.x, origin.y + px(top)), size(width, px(height)))
            }
            None => Bounds::new(at, size(width, line_height)),
        }
    };
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
            backgrounds.push(fill(cell_box(line, column, width), bg));
        }
        // The ⌘F match in yellow, so it isn't confused with the selection.
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

        let glyph_box = cell_box(line, column, width);
        if let Some(rects) = glyphs::custom_glyph(
            cell.c,
            f32::from(glyph_box.size.width),
            f32::from(glyph_box.size.height),
            stroke,
        ) {
            flush(current.take(), &mut shaped);
            let at = glyph_box.origin;
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
        // A wide character ends the segment so the next one starts aligned to the grid.
        if wide {
            flush(current.take(), &mut shaped);
        }
    }
    flush(current.take(), &mut shaped);

    // History suggestion, in gray starting at the cursor.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_drawn_with_blocks_keep_their_proportions() {
        // What Expo prints: half blocks, then text; a box and a bar don't count.
        let term = search_term(
            "› Scan\r\n▄▄▄▄\r\n█▀ █\r\n█▄▀█\r\n▀▀▀▀\r\nMetro\r\n╭──╮\r\n│  │\r\n╰──╯\r\n▂▄█▆\r\n▀▀",
            10,
            12,
        );
        let rows = super::picture_rows(&term, 12, 0, 8., 22.);
        // Rows 16 px tall (a half block, 8×8) from where the picture starts.
        assert_eq!(rows[0], None);
        assert_eq!(rows[1], Some((22., 16.)));
        assert_eq!(rows[2], Some((38., 16.)));
        assert_eq!(rows[4], Some((70., 16.)));
        assert!(rows[5..].iter().all(Option::is_none));

        // Scrolled two rows down, the picture stays where it was in the text.
        let mut term = search_term(&"\r\n".repeat(20), 10, 6);
        let mut parser: alacritty_terminal::vte::ansi::Processor =
            alacritty_terminal::vte::ansi::Processor::new();
        parser.advance(&mut term, "a\r\n🬀🬀\r\n🬀🬀\r\n🬀🬀\r\n🬀🬀\r\nb\r\n".as_bytes());
        let rows = super::picture_rows(&term, 6, 0, 8., 22.);
        let scrolled = super::picture_rows(&term, 6, 2, 8., 22.);
        // Sextants: 12 px rows, so a sextant is 4×4.
        assert_eq!(rows[0].map(|(_, h)| h), Some(12.));
        assert_eq!(rows[1].unwrap().0 - rows[0].unwrap().0, 12.);
        // "a" and the row above it come into view and the picture moves down two rows.
        assert_eq!(scrolled[..2], [None, None]);
        assert_eq!(scrolled[2].unwrap().0 - rows[0].unwrap().0, 2. * 22.);
        assert_eq!(scrolled[3].unwrap().0 - scrolled[2].unwrap().0, 12.);

        // Scrolled so the picture's first two rows are above the screen: it doesn't move.
        let mut term = search_term(&"\r\n".repeat(20), 10, 3);
        parser.advance(&mut term, "🬀🬀\r\n🬀🬀\r\n🬀🬀\r\n🬀🬀\r\nb\r\n".as_bytes());
        let rows = super::picture_rows(&term, 3, 1, 8., 22.);
        // Screen line 0 is the picture's third row (line -2), drawn 24 px below its top,
        // which is 2 rows (44 px) above the screen.
        assert_eq!(rows[0], Some((-44. + 24., 12.)));
    }
}
