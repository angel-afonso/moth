//! Grid colors: maps alacritty's colors (ANSI, 256, truecolor) through the active theme's
//! tokens, so switching themes also recolors output that's already printed.

use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Rgb};
use gpui::{Hsla, Rgba};

use crate::tokens::TerminalColors;

fn from_rgb(c: Rgb) -> Hsla {
    Rgba {
        r: c.r as f32 / 255.,
        g: c.g as f32 / 255.,
        b: c.b as f32 / 255.,
        a: 1.,
    }
    .into()
}

fn to_rgb(color: Hsla) -> Rgb {
    let c: Rgba = color.into();
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    Rgb {
        r: byte(c.r),
        g: byte(c.g),
        b: byte(c.b),
    }
}

/// Resolves an index into xterm's 256-color palette.
pub fn indexed(
    index: u8,
    overrides: &dyn Fn(usize) -> Option<Rgb>,
    colors: &TerminalColors,
) -> Hsla {
    if let Some(c) = overrides(index as usize) {
        return from_rgb(c);
    }
    match index {
        0..=15 => colors.ansi.get(index as usize),
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

pub fn named(color: NamedColor, colors: &TerminalColors) -> Hsla {
    use NamedColor::*;
    let dim = |i: usize| colors.ansi.get(i).opacity(0.7);
    match color {
        Foreground | BrightForeground => colors.foreground,
        Background => colors.background,
        Cursor => colors.cursor,
        DimForeground => colors.foreground.opacity(0.7),
        DimBlack => dim(0),
        DimRed => dim(1),
        DimGreen => dim(2),
        DimYellow => dim(3),
        DimBlue => dim(4),
        DimMagenta => dim(5),
        DimCyan => dim(6),
        DimWhite => dim(7),
        other => colors.ansi.get(other as usize),
    }
}

pub fn resolve(
    color: AnsiColor,
    overrides: &dyn Fn(usize) -> Option<Rgb>,
    colors: &TerminalColors,
) -> Hsla {
    match color {
        AnsiColor::Spec(c) => from_rgb(c),
        AnsiColor::Indexed(i) => indexed(i, overrides, colors),
        AnsiColor::Named(n) => match overrides(n as usize) {
            Some(c) => from_rgb(c),
            None => named(n, colors),
        },
    }
}

/// Reply to OSC 4/10/11/12 (`ColorRequest`): programs asking for the actual colors.
pub fn rgb_for_index(index: usize, colors: &TerminalColors) -> Rgb {
    to_rgb(match index {
        0..=15 => colors.ansi.get(index),
        256 => colors.foreground,
        257 => colors.background,
        258 => colors.cursor,
        _ => indexed(index.min(255) as u8, &|_| None, colors),
    })
}
