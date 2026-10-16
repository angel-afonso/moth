//! Temas: los tokens activos (ver `tokens.rs`) aplicados a la app y al sistema de temas de
//! Zed, para que los componentes del crate `ui` también los usen. Sin el crate `settings`.

use std::sync::Arc;

use gpui::{App, Font, FontFeatures, FontStyle, FontWeight, Global, Pixels, px};
use theme::{
    ActiveTheme, GlobalTheme, LoadThemes, ThemeSettingsProvider, UiDensity,
    set_theme_settings_provider,
};

use crate::{
    state,
    tokens::{self, ActiveTokens, Appearance, Tokens},
};

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

/// Temas disponibles (incluidos + del usuario).
pub struct Themes(pub Vec<Arc<Tokens>>);

impl Global for Themes {}

pub fn init(cx: &mut App) {
    set_theme_settings_provider(
        Box::new(Settings {
            ui_font: font(UI_FONT_FAMILY),
            mono_font: font(MONO_FONT_FAMILY),
        }),
        cx,
    );
    theme::init(LoadThemes::JustBase, cx);

    let themes = reload_themes(cx);
    let saved = state::saved_theme();
    let active = saved
        .and_then(|name| themes.iter().find(|t| t.name == name).cloned())
        .unwrap_or_else(|| themes[0].clone());
    apply(active, cx);
}

/// Vuelve a leer los temas (por si el usuario agregó o editó alguno).
pub fn reload_themes(cx: &mut App) -> Vec<Arc<Tokens>> {
    let themes = tokens::load_themes(tokens::user_themes_dir().as_deref());
    cx.set_global(Themes(themes.clone()));
    themes
}

/// Activa un tema: tokens propios, colores del tema de Zed y redibujo de las ventanas.
pub fn apply(tokens: Arc<Tokens>, cx: &mut App) {
    let mut theme = (**cx.theme()).clone();
    theme.name = tokens.name.clone().into();
    theme.appearance = match tokens.appearance {
        Appearance::Dark => theme::Appearance::Dark,
        Appearance::Light => theme::Appearance::Light,
    };

    let ui = &tokens.ui;
    let c = &mut theme.styles.colors;
    c.background = ui.background;
    c.editor_background = ui.background;
    c.terminal_background = tokens.terminal.background;
    c.surface_background = ui.chrome;
    c.elevated_surface_background = ui.surface;
    c.title_bar_background = ui.chrome;
    c.status_bar_background = ui.chrome;
    c.tab_bar_background = ui.chrome;
    c.tab_inactive_background = ui.chrome;
    c.tab_active_background = ui.background;
    c.border = ui.border;
    c.border_variant = ui.border_strong;
    c.border_focused = ui.accent;
    c.border_selected = ui.accent;
    c.border_disabled = ui.border;
    c.element_background = ui.element_background;
    c.element_hover = ui.element_hover;
    c.element_active = ui.element_selected;
    c.element_selected = ui.element_selected;
    c.ghost_element_background = tokens::tint(ui.background, 0.);
    c.ghost_element_hover = ui.element_hover;
    c.ghost_element_active = ui.element_selected;
    c.ghost_element_selected = ui.element_selected;
    c.text = ui.text;
    c.text_muted = ui.text_muted;
    c.text_placeholder = ui.text_placeholder;
    c.text_disabled = ui.text_placeholder;
    c.text_accent = ui.accent;
    c.icon = ui.text_muted;
    c.icon_muted = ui.text_placeholder;
    c.icon_placeholder = ui.text_placeholder;
    c.icon_disabled = ui.text_placeholder;
    c.icon_accent = ui.accent;
    c.scrollbar_thumb_background = tokens::tint(ui.text_placeholder, 0.3);
    c.scrollbar_thumb_hover_background = tokens::tint(ui.text_placeholder, 0.5);

    let status = &mut theme.styles.status;
    status.success = tokens.status.success;
    status.warning = tokens.status.warning;
    status.error = tokens.status.error;
    status.info = ui.accent;
    status.hint = ui.text_placeholder;

    GlobalTheme::update_theme(cx, Arc::new(theme));
    cx.set_global(ActiveTokens(tokens));
    cx.refresh_windows();
}

/// Cambia al tema llamado `name` y lo recuerda para la próxima vez.
pub fn select(name: &str, cx: &mut App) {
    let theme = cx
        .try_global::<Themes>()
        .and_then(|themes| themes.0.iter().find(|t| t.name == name).cloned());
    if let Some(theme) = theme {
        state::save_theme(&theme.name);
        apply(theme, cx);
    }
}
