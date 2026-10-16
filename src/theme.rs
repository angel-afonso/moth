//! Tema "Graphite" sobre el sistema de temas de Zed, sin depender de su crate `settings`.

use std::sync::Arc;

use gpui::{App, Font, FontFeatures, FontStyle, FontWeight, Pixels, px};
use theme::{
    ActiveTheme, GlobalTheme, LoadThemes, ThemeSettingsProvider, UiDensity,
    set_theme_settings_provider,
};

use crate::palette::*;

pub const UI_FONT_FAMILY: &str = "Geist";
pub const MONO_FONT_FAMILY: &str = "Geist Mono";
pub const UI_FONT_SIZE: Pixels = px(13.);
pub const TERMINAL_FONT_SIZE: Pixels = px(13.5);
pub const TERMINAL_LINE_HEIGHT: Pixels = px(22.);

pub fn font(family: &'static str) -> Font {
    Font {
        family: family.into(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

struct Settings {
    ui_font: Font,
    mono_font: Font,
}

impl ThemeSettingsProvider for Settings {
    fn ui_font<'a>(&'a self, _: &'a App) -> &'a Font {
        &self.ui_font
    }

    fn buffer_font<'a>(&'a self, _: &'a App) -> &'a Font {
        &self.mono_font
    }

    fn ui_font_size(&self, _: &App) -> Pixels {
        UI_FONT_SIZE
    }

    fn buffer_font_size(&self, _: &App) -> Pixels {
        TERMINAL_FONT_SIZE
    }

    fn ui_density(&self, _: &App) -> UiDensity {
        UiDensity::Default
    }
}

pub fn init(cx: &mut App) {
    set_theme_settings_provider(
        Box::new(Settings {
            ui_font: font(UI_FONT_FAMILY),
            mono_font: font(MONO_FONT_FAMILY),
        }),
        cx,
    );
    theme::init(LoadThemes::JustBase, cx);

    // Se parte del tema oscuro base de Zed y se sobrescriben los colores que usan los componentes de `ui`.
    let mut theme = (**cx.theme()).clone();
    theme.name = "Graphite".into();
    let c = &mut theme.styles.colors;
    c.background = hsla(BACKGROUND);
    c.editor_background = hsla(BACKGROUND);
    c.terminal_background = hsla(BACKGROUND);
    c.surface_background = hsla(CHROME);
    c.elevated_surface_background = hsla(SURFACE);
    c.title_bar_background = hsla(CHROME);
    c.status_bar_background = hsla(CHROME);
    c.tab_bar_background = hsla(CHROME);
    c.tab_inactive_background = hsla(CHROME);
    c.tab_active_background = hsla(BACKGROUND);
    c.border = hsla(BORDER_VARIANT);
    c.border_variant = hsla(BORDER_VARIANT);
    c.element_hover = hsla(ELEMENT_HOVER);
    c.element_selected = hsla(ELEMENT_SELECTED);
    c.ghost_element_hover = hsla(ELEMENT_HOVER);
    c.ghost_element_active = hsla(ELEMENT_SELECTED);
    c.ghost_element_selected = hsla(ELEMENT_SELECTED);
    c.text = hsla(TEXT);
    c.text_muted = hsla(TEXT_MUTED);
    c.text_accent = hsla(ACCENT);
    c.icon = hsla(TEXT_MUTED);
    c.icon_muted = hsla(TEXT_PLACEHOLDER);
    GlobalTheme::update_theme(cx, Arc::new(theme));
}
