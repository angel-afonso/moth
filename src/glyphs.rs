//! Characters that must fill the whole cell: blocks (U+2580–259F), sextants (U+1FB00–1FB3B)
//! and box-drawing lines (U+2500–257F). From a font, the glyph only covers the letter height
//! and leaves gaps between rows (and some don't even exist in Geist Mono), so they're drawn as
//! rectangles on the cell geometry, like Zed, kitty and Ghostty do.

/// Rectangle relative to the cell origin, in pixels, with its opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub alpha: f32,
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect {
        x0,
        y0,
        x1,
        y1,
        alpha: 1.,
    }
}

/// Rectangles to draw `c` in a `w`×`h` pixel cell, or `None` if it's drawn with the font.
/// `stroke` is the thickness of a thin line.
pub fn custom_glyph(c: char, w: f32, h: f32, stroke: f32) -> Option<Vec<Rect>> {
    match c as u32 {
        0x2580..=0x259F => block(c, w, h),
        0x2500..=0x257F => box_drawing(c, w, h, stroke),
        0x1FB00..=0x1FB3B => Some(sextant(c as u32 - 0x1FB00, w, h)),
        _ => None,
    }
}

/// A 2×3 grid of blocks, used for QR codes (Expo's) and pictures drawn in text. Each character
/// is one of the 64 patterns, numbered by its blocks (bit 0 the upper left, bit 1 the upper
/// right, and so on down) in order, minus four that exist elsewhere: none (a space), the left
/// and right columns (▌ ▐) and all (█).
fn sextant(index: u32, w: f32, h: f32) -> Vec<Rect> {
    let mut pattern = index + 1;
    for skipped in [0b010101, 0b101010] {
        if pattern >= skipped {
            pattern += 1;
        }
    }
    let xs = [0., w / 2., w];
    let ys = [0., h / 3., h * 2. / 3., h];
    (0..6)
        .filter(|bit| pattern & (1 << bit) != 0)
        .map(|bit| {
            let (column, row) = ((bit % 2) as usize, (bit / 2) as usize);
            rect(xs[column], ys[row], xs[column + 1], ys[row + 1])
        })
        .collect()
}

fn block(c: char, w: f32, h: f32) -> Option<Vec<Rect>> {
    let (hw, hh) = (w / 2., h / 2.);
    let ul = rect(0., 0., hw, hh);
    let ur = rect(hw, 0., w, hh);
    let ll = rect(0., hh, hw, h);
    let lr = rect(hw, hh, w, h);
    let shade = |alpha: f32| {
        vec![Rect {
            alpha,
            ..rect(0., 0., w, h)
        }]
    };
    Some(match c {
        '▀' => vec![rect(0., 0., w, hh)],
        '▁'..='▇' => {
            let eighths = (c as u32 - 0x2580) as f32;
            vec![rect(0., h - h * eighths / 8., w, h)]
        }
        '█' => vec![rect(0., 0., w, h)],
        '▉'..='▏' => {
            let eighths = (0x2590 - c as u32) as f32;
            vec![rect(0., 0., w * eighths / 8., h)]
        }
        '▐' => vec![rect(hw, 0., w, h)],
        '░' => shade(0.25),
        '▒' => shade(0.5),
        '▓' => shade(0.75),
        '▔' => vec![rect(0., 0., w, h / 8.)],
        '▕' => vec![rect(w - w / 8., 0., w, h)],
        '▖' => vec![ll],
        '▗' => vec![lr],
        '▘' => vec![ul],
        '▙' => vec![ul, ll, lr],
        '▚' => vec![ul, lr],
        '▛' => vec![ul, ur, ll],
        '▜' => vec![ul, ur, lr],
        '▝' => vec![ur],
        '▞' => vec![ur, ll],
        '▟' => vec![ur, ll, lr],
        _ => return None,
    })
}

/// Weight of each arm of a box-drawing line: 0 none, 1 light, 2 heavy.
#[derive(Clone, Copy)]
struct Arms {
    up: u8,
    right: u8,
    down: u8,
    left: u8,
}

fn arms(c: char) -> Option<Arms> {
    let a = |up, right, down, left| {
        Some(Arms {
            up,
            right,
            down,
            left,
        })
    };
    match c {
        '─' => a(0, 1, 0, 1),
        '━' => a(0, 2, 0, 2),
        '│' => a(1, 0, 1, 0),
        '┃' => a(2, 0, 2, 0),
        '┌' | '╭' => a(0, 1, 1, 0),
        '┐' | '╮' => a(0, 0, 1, 1),
        '└' | '╰' => a(1, 1, 0, 0),
        '┘' | '╯' => a(1, 0, 0, 1),
        '┏' => a(0, 2, 2, 0),
        '┓' => a(0, 0, 2, 2),
        '┗' => a(2, 2, 0, 0),
        '┛' => a(2, 0, 0, 2),
        '├' => a(1, 1, 1, 0),
        '┤' => a(1, 0, 1, 1),
        '┬' => a(0, 1, 1, 1),
        '┴' => a(1, 1, 0, 1),
        '┼' => a(1, 1, 1, 1),
        '┣' => a(2, 2, 2, 0),
        '┫' => a(2, 0, 2, 2),
        '┳' => a(0, 2, 2, 2),
        '┻' => a(2, 2, 0, 2),
        '╋' => a(2, 2, 2, 2),
        '╴' => a(0, 0, 0, 1),
        '╵' => a(1, 0, 0, 0),
        '╶' => a(0, 1, 0, 0),
        '╷' => a(0, 0, 1, 0),
        _ => None,
    }
}

fn box_drawing(c: char, w: f32, h: f32, stroke: f32) -> Option<Vec<Rect>> {
    if let Some(rects) = double_line(c, w, h, stroke) {
        return Some(rects);
    }
    let arms = arms(c)?;
    let (cx, cy) = (w / 2., h / 2.);
    let thickness = |weight: u8| if weight == 2 { stroke * 2. } else { stroke };
    // The center extends by half the thickness (of the heaviest perpendicular arm) so that
    // corners and crossings are closed.
    let vertical = thickness(arms.up.max(arms.down));
    let horizontal = thickness(arms.left.max(arms.right));
    let mut rects = Vec::new();
    if arms.left > 0 {
        let t = thickness(arms.left);
        rects.push(rect(0., cy - t / 2., cx + vertical / 2., cy + t / 2.));
    }
    if arms.right > 0 {
        let t = thickness(arms.right);
        rects.push(rect(cx - vertical / 2., cy - t / 2., w, cy + t / 2.));
    }
    if arms.up > 0 {
        let t = thickness(arms.up);
        rects.push(rect(cx - t / 2., 0., cx + t / 2., cy + horizontal / 2.));
    }
    if arms.down > 0 {
        let t = thickness(arms.down);
        rects.push(rect(cx - t / 2., cy - horizontal / 2., cx + t / 2., h));
    }
    Some(rects)
}

/// Straight double lines and corners (`═ ║ ╔ ╗ ╚ ╝`); other double lines use the font.
fn double_line(c: char, w: f32, h: f32, stroke: f32) -> Option<Vec<Rect>> {
    let (cx, cy) = (w / 2., h / 2.);
    let g = stroke.max(1.);
    let t = stroke;
    let hline = |y: f32, x0: f32, x1: f32| rect(x0, y - t / 2., x1, y + t / 2.);
    let vline = |x: f32, y0: f32, y1: f32| rect(x - t / 2., y0, x + t / 2., y1);
    Some(match c {
        '═' => vec![hline(cy - g, 0., w), hline(cy + g, 0., w)],
        '║' => vec![vline(cx - g, 0., h), vline(cx + g, 0., h)],
        '╔' => vec![
            hline(cy - g, cx - g - t / 2., w),
            vline(cx - g, cy - g, h),
            hline(cy + g, cx + g - t / 2., w),
            vline(cx + g, cy + g, h),
        ],
        '╗' => vec![
            hline(cy - g, 0., cx + g + t / 2.),
            vline(cx + g, cy - g, h),
            hline(cy + g, 0., cx - g + t / 2.),
            vline(cx - g, cy + g, h),
        ],
        '╚' => vec![
            hline(cy + g, cx - g - t / 2., w),
            vline(cx - g, 0., cy + g),
            hline(cy - g, cx + g - t / 2., w),
            vline(cx + g, 0., cy - g),
        ],
        '╝' => vec![
            hline(cy + g, 0., cx + g + t / 2.),
            vline(cx + g, 0., cy + g),
            hline(cy - g, 0., cx - g + t / 2.),
            vline(cx - g, 0., cy - g),
        ],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 8.;
    const H: f32 = 22.;

    fn covers(rects: &[Rect], x: f32, y: f32) -> bool {
        rects
            .iter()
            .any(|r| x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1)
    }

    #[test]
    fn blocks_fill_the_whole_cell_height() {
        let full = custom_glyph('█', W, H, 1.).unwrap();
        assert_eq!(full, vec![rect(0., 0., W, H)]);
        let upper = custom_glyph('▀', W, H, 1.).unwrap();
        assert!(covers(&upper, 1., 0.) && !covers(&upper, 1., H - 1.));
        let lower_quarter = custom_glyph('▂', W, H, 1.).unwrap();
        assert_eq!(lower_quarter[0].y0, H - H / 4.);
        let left_half = custom_glyph('▌', W, H, 1.).unwrap();
        assert_eq!(left_half[0].x1, W / 2.);
    }

    #[test]
    fn quadrants_used_by_the_claude_logo() {
        // ▛ = upper left, upper right and lower left.
        let q = custom_glyph('▛', W, H, 1.).unwrap();
        assert!(covers(&q, 1., 1.) && covers(&q, W - 1., 1.) && covers(&q, 1., H - 1.));
        assert!(!covers(&q, W - 1., H - 1.));
        let q = custom_glyph('▝', W, H, 1.).unwrap();
        assert!(covers(&q, W - 1., 1.) && !covers(&q, 1., 1.));
    }

    #[test]
    fn sextants_fill_thirds_of_the_cell() {
        // 🬀 = upper left only.
        let first = custom_glyph('\u{1FB00}', W, H, 1.).unwrap();
        assert_eq!(first, vec![rect(0., 0., W / 2., H / 3.)]);
        // 🬓 = middle and lower left, just before ▌'s pattern is skipped.
        let before = custom_glyph('\u{1FB13}', W, H, 1.).unwrap();
        assert!(covers(&before, 1., H / 2.) && covers(&before, 1., H - 1.));
        assert!(!covers(&before, 1., 1.) && !covers(&before, W - 1., H - 1.));
        // 🬔 = the pattern after ▌: upper right, middle left and lower left.
        let after = custom_glyph('\u{1FB14}', W, H, 1.).unwrap();
        assert!(covers(&after, W - 1., 1.) && covers(&after, 1., H - 1.));
        assert!(!covers(&after, 1., 1.));
        // 🬻 = everything but the upper left.
        let last = custom_glyph('\u{1FB3B}', W, H, 1.).unwrap();
        assert_eq!(last.len(), 5);
        assert!(!covers(&last, 1., 1.) && covers(&last, W - 1., H - 1.));
        assert!(custom_glyph('\u{1FB3C}', W, H, 1.).is_none());
    }

    #[test]
    fn box_lines_reach_the_cell_edges() {
        let v = custom_glyph('│', W, H, 1.).unwrap();
        assert!(covers(&v, W / 2., 0.) && covers(&v, W / 2., H - 0.5));
        let corner = custom_glyph('╭', W, H, 1.).unwrap();
        assert!(covers(&corner, W - 0.5, H / 2.) && covers(&corner, W / 2., H - 0.5));
        assert!(!covers(&corner, W / 2., 0.5));
        let heavy = custom_glyph('━', W, H, 1.).unwrap();
        assert_eq!(heavy[0].y1 - heavy[0].y0, 2.);
        assert_eq!(custom_glyph('═', W, H, 1.).unwrap().len(), 2);
    }

    #[test]
    fn other_characters_use_the_font() {
        assert!(custom_glyph('a', W, H, 1.).is_none());
        assert!(custom_glyph('┄', W, H, 1.).is_none());
        assert!(custom_glyph('╬', W, H, 1.).is_none());
    }
}
