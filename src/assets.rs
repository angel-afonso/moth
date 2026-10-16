use std::borrow::Cow;

use anyhow::{Context as _, Result};
use gpui::{App, AssetSource, SharedString};
use rust_embed::RustEmbed;

/// Íconos (copiados de Zed, los que espera `ui::IconName`), fuentes Geist, scripts de
/// integración con la shell y temas, embebidos en el binario.
#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*.svg"]
#[include = "fonts/**/*.ttf"]
#[include = "shell/**/*"]
#[include = "themes/*.json"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(Self::get(path).map(|f| f.data))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter(|p| p.starts_with(path))
            .map(SharedString::from)
            .collect())
    }
}

impl Assets {
    pub fn load_fonts(&self, cx: &App) -> Result<()> {
        let fonts = Self::iter()
            .filter(|p| p.starts_with("fonts/") && p.ends_with(".ttf"))
            .filter_map(|p| Self::get(&p).map(|f| f.data))
            .collect::<Vec<_>>();
        cx.text_system()
            .add_fonts(fonts)
            .context("no se pudieron registrar las fuentes Geist")
    }
}
