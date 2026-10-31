//! Caracteres que deben llenar la celda entera: bloques (U+2580–259F) y líneas de caja
//! (U+2500–257F). Con una fuente, el glifo solo cubre la altura de la letra y quedan huecos
//! entre filas (y algunos ni siquiera existen en Geist Mono), así que se dibujan como
//! rectángulos sobre la geometría de la celda, como hacen Zed, kitty o Ghostty.

/// Rectángulo relativo al origen de la celda, en píxeles, con su opacidad.
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

/// Rectángulos para dibujar `c` en una celda de `w`×`h` píxeles, o `None` si se dibuja con
/// la fuente. `stroke` es el grosor de una línea fina.
pub fn custom_glyph(c: char, w: f32, h: f32, stroke: f32) -> Option<Vec<Rect>> {
    match c as u32 {
        0x2580..=0x259F => block(c, w, h),
        0x2500..=0x257F => box_drawing(c, w, h, stroke),
        _ => None,
    }
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

/// Peso de cada brazo de una línea de caja: 0 nada, 1 fino, 2 grueso.
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
    // El centro se extiende medio grosor (del brazo perpendicular más grueso) para que las
    // esquinas y cruces queden cerradas.
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

/// Líneas dobles rectas y esquinas (`═ ║ ╔ ╗ ╚ ╝`); el resto de las dobles, con la fuente.
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
        // ▛ = arriba a la izquierda, arriba a la derecha y abajo a la izquierda.
        let q = custom_glyph('▛', W, H, 1.).unwrap();
        assert!(covers(&q, 1., 1.) && covers(&q, W - 1., 1.) && covers(&q, 1., H - 1.));
        assert!(!covers(&q, W - 1., H - 1.));
        let q = custom_glyph('▝', W, H, 1.).unwrap();
        assert!(covers(&q, W - 1., 1.) && !covers(&q, 1., 1.));
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
