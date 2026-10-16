//! Paleta "Graphite": la misma del canvas de diseño (tokens y sintaxis).

use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Rgb};
use gpui::{Hsla, Rgba, rgb};

pub const BACKGROUND: u32 = 0x0C0D10;
pub const CHROME: u32 = 0x121418;
pub const SURFACE: u32 = 0x15181D;
pub const ELEMENT_HOVER: u32 = 0x171A1F;
pub const ELEMENT_SELECTED: u32 = 0x1F2531;
pub const BORDER_VARIANT: u32 = 0x1E2127;
pub const TEXT: u32 = 0xE3E6EB;
pub const TEXT_MUTED: u32 = 0xA3AAB4;
pub const TEXT_PLACEHOLDER: u32 = 0x7D8590;
pub const ACCENT: u32 = 0x82AAFF;

pub const SUCCESS: u32 = 0x7EE0A1;
pub const WARNING: u32 = 0xF2A65A;
pub const ERROR: u32 = 0xFF7B72;

pub const SYNTAX_COMMAND: u32 = 0x82AAFF;
pub const SYNTAX_SUBCOMMAND: u32 = 0xC3A6FF;
pub const SYNTAX_FLAG: u32 = 0xF2A65A;
pub const SYNTAX_STRING: u32 = 0x9BD68A;
pub const SYNTAX_PATH: u32 = 0x6FD3D0;
pub const SYNTAX_OPERATOR: u32 = 0xF2777A;
pub const SYNTAX_NUMBER: u32 = 0xF5C97A;
pub const SYNTAX_ERROR: u32 = 0xFF7B72;
pub const SYNTAX_GHOST: u32 = 0x737B88;

/// ANSI 0-15, afinados para leerse sobre `BACKGROUND`.
const ANSI: [u32; 16] = [
    0x1A1D22, // black
    0xFF7B72, // red
    0x7EE0A1, // green
    0xF5C97A, // yellow
    0x82AAFF, // blue
    0xC3A6FF, // magenta
    0x6FD3D0, // cyan
    0xC9CED6, // white
    0x5C6470, // bright black
    0xFF9E97, // bright red
    0x9BEBB8, // bright green
    0xF9DA9E, // bright yellow
    0xA9C4FF, // bright blue
    0xD6C2FF, // bright magenta
    0x96E3E0, // bright cyan
    0xF2F4F7, // bright white
];

pub fn hsla(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn from_rgb(c: Rgb) -> Hsla {
    Rgba {
        r: c.r as f32 / 255.,
        g: c.g as f32 / 255.,
        b: c.b as f32 / 255.,
        a: 1.,
    }
    .into()
}

/// Resuelve un índice de la paleta de 256 colores de xterm.
pub fn indexed(index: u8, overrides: &dyn Fn(usize) -> Option<Rgb>) -> Hsla {
    if let Some(c) = overrides(index as usize) {
        return from_rgb(c);
    }
    match index {
        0..=15 => hsla(ANSI[index as usize]),
        16..=231 => {
            let i = index - 16;
            let step = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
            from_rgb(Rgb {
                r: step(i / 36),
                g: step((i / 6) % 6),
                b: step(i % 6),
            })
        }
        232..=255 => {
            let v = (index - 232) * 10 + 8;
            from_rgb(Rgb { r: v, g: v, b: v })
        }
    }
}

pub fn named(color: NamedColor) -> Hsla {
    use NamedColor::*;
    match color {
        Foreground | BrightForeground => hsla(TEXT),
        Background => hsla(BACKGROUND),
        Cursor => hsla(ACCENT),
        DimForeground => hsla(TEXT_MUTED),
        DimBlack => hsla(ANSI[0]).opacity(0.7),
        DimRed => hsla(ANSI[1]).opacity(0.7),
        DimGreen => hsla(ANSI[2]).opacity(0.7),
        DimYellow => hsla(ANSI[3]).opacity(0.7),
        DimBlue => hsla(ANSI[4]).opacity(0.7),
        DimMagenta => hsla(ANSI[5]).opacity(0.7),
        DimCyan => hsla(ANSI[6]).opacity(0.7),
        DimWhite => hsla(ANSI[7]).opacity(0.7),
        other => hsla(ANSI[other as usize]),
    }
}

pub fn resolve(color: AnsiColor, overrides: &dyn Fn(usize) -> Option<Rgb>) -> Hsla {
    match color {
        AnsiColor::Spec(c) => from_rgb(c),
        AnsiColor::Indexed(i) => indexed(i, overrides),
        AnsiColor::Named(n) => match overrides(n as usize) {
            Some(c) => from_rgb(c),
            None => named(n),
        },
    }
}

/// Respuesta a OSC 4/10/11/12 (`ColorRequest`).
pub fn rgb_for_index(index: usize) -> Rgb {
    let h = match index {
        0..=15 => ANSI[index],
        256 => TEXT,
        257 => BACKGROUND,
        258 => ACCENT,
        _ => {
            let c: Rgba = indexed(index.min(255) as u8, &|_| None).into();
            return Rgb {
                r: (c.r * 255.) as u8,
                g: (c.g * 255.) as u8,
                b: (c.b * 255.) as u8,
            };
        }
    };
    Rgb {
        r: (h >> 16) as u8,
        g: (h >> 8) as u8,
        b: h as u8,
    }
}
