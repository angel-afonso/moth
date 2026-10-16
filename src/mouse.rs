//! Reporte de mouse a los programas que lo piden (vim, htop, lazygit…): codifica clics,
//! arrastres, movimiento y rueda en las secuencias de xterm (SGR o el formato clásico X10).

use alacritty_terminal::term::TermMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Press,
    Release,
    /// Movimiento; con `button`, arrastre.
    Motion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
}

/// El programa quiere eventos del mouse (y por lo tanto la selección local queda para Shift).
pub fn wants_mouse(mode: TermMode) -> bool {
    mode.intersects(TermMode::MOUSE_MODE)
}

/// Secuencia para el evento, o `None` si el modo actual no lo pide. `column` y `line` son de
/// la pantalla visible, desde 0.
pub fn report(
    button: Option<Button>,
    action: Action,
    column: usize,
    line: usize,
    mods: Mods,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if !wants_mouse(mode) {
        return None;
    }
    if action == Action::Motion {
        let wanted = if button.is_some() {
            mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
        } else {
            mode.contains(TermMode::MOUSE_MOTION)
        };
        if !wanted {
            return None;
        }
    }

    let sgr = mode.contains(TermMode::SGR_MOUSE);
    let mut code: u32 = match button {
        Some(Button::Left) => 0,
        Some(Button::Middle) => 1,
        Some(Button::Right) => 2,
        Some(Button::WheelUp) => 64,
        Some(Button::WheelDown) => 65,
        None => 3,
    };
    // En el formato clásico la liberación no dice qué botón: siempre es 3.
    if action == Action::Release && !sgr {
        code = 3;
    }
    if action == Action::Motion {
        code += 32;
    }
    code += (mods.shift as u32) * 4 + (mods.alt as u32) * 8 + (mods.control as u32) * 16;

    let (x, y) = (column as u32 + 1, line as u32 + 1);
    if sgr {
        let kind = if action == Action::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{kind}").into_bytes());
    }

    let utf8 = mode.contains(TermMode::UTF8_MOUSE);
    let max = if utf8 { 2015 } else { 223 };
    if x > max || y > max {
        return None;
    }
    let mut bytes = b"\x1b[M".to_vec();
    bytes.push((32 + code) as u8);
    for value in [x, y] {
        let value = 32 + value;
        if utf8 && value >= 128 {
            let mut buf = [0; 4];
            bytes.extend_from_slice(char::from_u32(value)?.encode_utf8(&mut buf).as_bytes());
        } else {
            bytes.push(value as u8);
        }
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(bytes: Option<Vec<u8>>) -> Option<String> {
        bytes.map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    #[test]
    fn nothing_without_mouse_mode() {
        assert_eq!(report(Some(Button::Left), Action::Press, 0, 0, Mods::default(), TermMode::NONE), None);
    }

    #[test]
    fn sgr_press_release_and_drag() {
        let mode = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let m = Mods::default();
        assert_eq!(s(report(Some(Button::Left), Action::Press, 9, 4, m, mode)).unwrap(), "\x1b[<0;10;5M");
        assert_eq!(s(report(Some(Button::Left), Action::Release, 9, 4, m, mode)).unwrap(), "\x1b[<0;10;5m");
        assert_eq!(s(report(Some(Button::Left), Action::Motion, 11, 4, m, mode)).unwrap(), "\x1b[<32;12;5M");
        assert_eq!(s(report(Some(Button::WheelDown), Action::Press, 0, 0, m, mode)).unwrap(), "\x1b[<65;1;1M");
        let ctrl = Mods { control: true, ..m };
        assert_eq!(s(report(Some(Button::Right), Action::Press, 0, 0, ctrl, mode)).unwrap(), "\x1b[<18;1;1M");
        // Movimiento sin botón solo con MOUSE_MOTION.
        assert_eq!(report(None, Action::Motion, 1, 1, m, mode), None);
        let any = TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE;
        assert_eq!(s(report(None, Action::Motion, 1, 1, m, any)).unwrap(), "\x1b[<35;2;2M");
    }

    #[test]
    fn x10_format() {
        let mode = TermMode::MOUSE_REPORT_CLICK;
        let m = Mods::default();
        assert_eq!(report(Some(Button::Left), Action::Press, 0, 0, m, mode).unwrap(), b"\x1b[M !!".to_vec());
        // La liberación no indica el botón.
        assert_eq!(report(Some(Button::Right), Action::Release, 0, 0, m, mode).unwrap(), b"\x1b[M#!!".to_vec());
        // Fuera de rango en X10 no se reporta.
        assert_eq!(report(Some(Button::Left), Action::Press, 300, 0, m, mode), None);
        // Con UTF8_MOUSE sí, codificado en UTF-8.
        let utf8 = report(Some(Button::Left), Action::Press, 300, 0, m, mode | TermMode::UTF8_MOUSE).unwrap();
        assert_eq!(String::from_utf8(utf8).unwrap(), format!("\x1b[M {}!", char::from_u32(32 + 301).unwrap()));
    }
}
