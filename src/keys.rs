//! Teclas especiales → secuencias de escape (basado en el mapeo de Zed/alacritty).
//! El texto imprimible no pasa por aquí: llega por el `InputHandler` para soportar IME y teclas muertas.

use std::borrow::Cow;

use alacritty_terminal::term::TermMode;
use gpui::Keystroke;

pub fn to_esc_str(keystroke: &Keystroke, mode: TermMode) -> Option<Cow<'static, str>> {
    let m = &keystroke.modifiers;
    if m.platform {
        return None;
    }
    let key = keystroke.key.as_str();
    let plain = !m.alt && !m.control && !m.shift;
    let app_cursor = mode.contains(TermMode::APP_CURSOR);

    let fixed: Option<&'static str> = match key {
        "enter" if plain => Some("\r"),
        "enter" if m.shift && !m.alt && !m.control => Some("\n"),
        "enter" if m.alt && !m.control => Some("\x1b\r"),
        "tab" if plain => Some("\t"),
        "tab" if m.shift && !m.alt && !m.control => Some("\x1b[Z"),
        "escape" if plain => Some("\x1b"),
        "backspace" if m.control => Some("\x08"),
        "backspace" if m.alt => Some("\x1b\x7f"),
        "backspace" => Some("\x7f"),
        "space" if m.control => Some("\x00"),
        "up" if plain => Some(if app_cursor { "\x1bOA" } else { "\x1b[A" }),
        "down" if plain => Some(if app_cursor { "\x1bOB" } else { "\x1b[B" }),
        "right" if plain => Some(if app_cursor { "\x1bOC" } else { "\x1b[C" }),
        "left" if plain => Some(if app_cursor { "\x1bOD" } else { "\x1b[D" }),
        "home" if plain => Some(if app_cursor { "\x1bOH" } else { "\x1b[H" }),
        "end" if plain => Some(if app_cursor { "\x1bOF" } else { "\x1b[F" }),
        "insert" if plain => Some("\x1b[2~"),
        "delete" if plain => Some("\x1b[3~"),
        "pageup" if plain => Some("\x1b[5~"),
        "pagedown" if plain => Some("\x1b[6~"),
        "f1" if plain => Some("\x1bOP"),
        "f2" if plain => Some("\x1bOQ"),
        "f3" if plain => Some("\x1bOR"),
        "f4" if plain => Some("\x1bOS"),
        _ => None,
    };
    if let Some(s) = fixed {
        return Some(Cow::Borrowed(s));
    }

    // Ctrl + letra / símbolo → carácter de control.
    if m.control && !m.alt && key.len() == 1 {
        let c = key.as_bytes()[0].to_ascii_lowercase();
        let ctrl = match c {
            b'a'..=b'z' => Some(c - b'a' + 1),
            b'@' | b'2' => Some(0),
            b'[' | b'3' => Some(0x1b),
            b'\\' | b'4' => Some(0x1c),
            b']' | b'5' => Some(0x1d),
            b'^' | b'6' => Some(0x1e),
            b'_' | b'-' | b'7' => Some(0x1f),
            b'?' | b'8' => Some(0x7f),
            _ => None,
        };
        if let Some(byte) = ctrl {
            return Some(Cow::Owned((byte as char).to_string()));
        }
    }

    let code = 1 + (m.shift as u8) + ((m.alt as u8) << 1) + ((m.control as u8) << 2);
    if code > 1 {
        let modified = match key {
            "up" => Some(format!("\x1b[1;{code}A")),
            "down" => Some(format!("\x1b[1;{code}B")),
            "right" => Some(format!("\x1b[1;{code}C")),
            "left" => Some(format!("\x1b[1;{code}D")),
            "home" => Some(format!("\x1b[1;{code}H")),
            "end" => Some(format!("\x1b[1;{code}F")),
            "delete" => Some(format!("\x1b[3;{code}~")),
            "pageup" => Some(format!("\x1b[5;{code}~")),
            "pagedown" => Some(format!("\x1b[6;{code}~")),
            _ => None,
        };
        if let Some(s) = modified {
            return Some(Cow::Owned(s));
        }
    }

    let fkey = match key {
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        _ => None,
    };
    if let Some(n) = fkey {
        return Some(Cow::Owned(if code > 1 {
            format!("\x1b[{n};{code}~")
        } else {
            format!("\x1b[{n}~")
        }));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn esc(key: &str, mode: TermMode) -> Option<String> {
        to_esc_str(&Keystroke::parse(key).unwrap(), mode).map(|s| s.into_owned())
    }

    #[test]
    fn maps_special_keys() {
        assert_eq!(esc("enter", TermMode::NONE).as_deref(), Some("\r"));
        assert_eq!(esc("ctrl-c", TermMode::NONE).as_deref(), Some("\x03"));
        assert_eq!(esc("up", TermMode::NONE).as_deref(), Some("\x1b[A"));
        assert_eq!(esc("up", TermMode::APP_CURSOR).as_deref(), Some("\x1bOA"));
        assert_eq!(esc("shift-right", TermMode::NONE).as_deref(), Some("\x1b[1;2C"));
    }

    #[test]
    fn leaves_text_and_app_shortcuts_alone() {
        assert_eq!(esc("a", TermMode::NONE), None);
        assert_eq!(esc("cmd-t", TermMode::NONE), None);
    }
}
