//! Design tokens: the colors from the "Tokens and components" sheet, loaded from JSON themes.
//!
//! Built-in themes live in `assets/themes/`. User themes go in
//! `~/Library/Application Support/moth/themes/*.json` and can inherit from another with
//! `"base"`, defining only what they change:
//!
//! ```json
//! { "name": "Graphite Orange", "base": "Graphite", "ui": { "accent": "#FF9F43" } }
//! ```

use std::{path::Path, sync::Arc};

use anyhow::{Context as _, Result, anyhow};
use gpui::{App, Global, Hsla, Rgba};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::assets::Assets;

/// Spacing, radius and type scale (doesn't change with the theme).
pub mod metrics {
    use gpui::{Pixels, px};

    pub const SPACE_2: Pixels = px(8.);
    pub const SPACE_4: Pixels = px(16.);
    pub const SPACE_5: Pixels = px(18.);
    pub const SPACE_7: Pixels = px(28.);

    pub const RADIUS_KBD: Pixels = px(4.);
    pub const RADIUS_ROW: Pixels = px(6.);
    pub const RADIUS_POPUP: Pixels = px(12.);

    pub const TEXT_META: Pixels = px(11.);
}

/// Colors in the JSON as `#RRGGBB` or `#RRGGBBAA`.
mod hex {
    use gpui::{Hsla, Rgba};
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn parse(text: &str) -> Option<Hsla> {
        let digits = text.strip_prefix('#')?;
        let value = u32::from_str_radix(digits, 16).ok()?;
        let (rgb, alpha) = match digits.len() {
            6 => (value, 0xFF),
            8 => (value >> 8, value & 0xFF),
            _ => return None,
        };
        Some(
            Rgba {
                r: ((rgb >> 16) & 0xFF) as f32 / 255.,
                g: ((rgb >> 8) & 0xFF) as f32 / 255.,
                b: (rgb & 0xFF) as f32 / 255.,
                a: alpha as f32 / 255.,
            }
            .into(),
        )
    }

    pub fn format(color: &Hsla) -> String {
        let c: Rgba = (*color).into();
        let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
        let rgb = format!("#{:02X}{:02X}{:02X}", byte(c.r), byte(c.g), byte(c.b));
        if c.a < 1. {
            format!("{rgb}{:02X}", byte(c.a))
        } else {
            rgb
        }
    }

    pub fn serialize<S: Serializer>(color: &Hsla, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format(color))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Hsla, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse(&text).ok_or_else(|| D::Error::custom(format!("invalid color: {text}")))
    }
}

macro_rules! color_group {
    ($(#[$doc:meta])* $name:ident { $($field:ident),* $(,)? }) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $(
                #[serde(with = "hex")]
                pub $field: Hsla,
            )*
        }
    };
}

color_group!(
    /// UI surfaces, borders and text.
    UiColors {
        background,
        chrome,
        surface,
        element_background,
        element_hover,
        element_selected,
        border,
        border_strong,
        divider,
        text,
        text_muted,
        text_placeholder,
        accent,
        overlay,
    }
);

color_group!(
    /// Command results and notices.
    StatusColors { success, warning, error }
);

color_group!(
    /// Command line highlighting.
    SyntaxColors {
        command,
        subcommand,
        flag,
        string,
        path,
        operator,
        number,
        error,
        ghost,
    }
);

color_group!(
    /// The 16 ANSI colors. The integration prompt uses `white` for secondary text,
    /// `bright_black` for metadata, `magenta` for the branch and `green`/`red` for status.
    AnsiColors {
        black,
        red,
        green,
        yellow,
        blue,
        magenta,
        cyan,
        white,
        bright_black,
        bright_red,
        bright_green,
        bright_yellow,
        bright_blue,
        bright_magenta,
        bright_cyan,
        bright_white,
    }
);

impl AnsiColors {
    pub fn get(&self, index: usize) -> Hsla {
        [
            self.black,
            self.red,
            self.green,
            self.yellow,
            self.blue,
            self.magenta,
            self.cyan,
            self.white,
            self.bright_black,
            self.bright_red,
            self.bright_green,
            self.bright_yellow,
            self.bright_blue,
            self.bright_magenta,
            self.bright_cyan,
            self.bright_white,
        ][index.min(15)]
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalColors {
    #[serde(with = "hex")]
    pub foreground: Hsla,
    #[serde(with = "hex")]
    pub background: Hsla,
    #[serde(with = "hex")]
    pub cursor: Hsla,
    /// Background of selected text (with transparency).
    #[serde(with = "hex")]
    pub selection: Hsla,
    pub ansi: AnsiColors,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    Dark,
    Light,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tokens {
    pub name: String,
    pub appearance: Appearance,
    pub ui: UiColors,
    pub status: StatusColors,
    pub syntax: SyntaxColors,
    pub terminal: TerminalColors,
}

pub const DEFAULT_THEME: &str = "Graphite";

fn builtin_sources() -> Vec<Value> {
    Assets::iter()
        .filter(|path| path.starts_with("themes/") && path.ends_with(".json"))
        .filter_map(|path| Assets::get(&path))
        .filter_map(|file| serde_json::from_slice(&file.data).ok())
        .collect()
}

fn user_sources(dir: &Path) -> Vec<(String, Value)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut sources: Vec<(String, Value)> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            match serde_json::from_str(&text) {
                Ok(value) => Some((path.display().to_string(), value)),
                Err(err) => {
                    crate::log::error(format!("invalid theme in {}: {err}", path.display()));
                    None
                }
            }
        })
        .collect();
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    sources
}

/// Merges `overlay` onto `base`, recursing into objects.
fn merge(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(existing) => merge(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

/// Resolves a user theme on top of its `base` (or Graphite) and checks that it's complete.
fn resolve(source: Value, builtins: &[Tokens]) -> Result<Tokens> {
    let base_name = source
        .get("base")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_THEME)
        .to_string();
    let base = builtins
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(&base_name))
        .ok_or_else(|| anyhow!("base theme \"{base_name}\" doesn't exist"))?;
    let mut value = serde_json::to_value(base)?;
    let mut source = source;
    if let Value::Object(map) = &mut source {
        map.remove("base");
    }
    merge(&mut value, source);
    serde_json::from_value(value).context("missing colors or unknown keys")
}

/// All available themes: built-in first, then the user's.
pub fn load_themes(user_dir: Option<&Path>) -> Vec<Arc<Tokens>> {
    let mut builtins: Vec<Tokens> = builtin_sources()
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect();
    builtins.sort_by_key(|t| t.name != DEFAULT_THEME);

    let mut themes: Vec<Arc<Tokens>> = builtins.iter().cloned().map(Arc::new).collect();
    for (path, source) in user_dir.map(user_sources).unwrap_or_default() {
        match resolve(source, &builtins) {
            Ok(tokens) => {
                themes.retain(|t| t.name != tokens.name);
                themes.push(Arc::new(tokens));
            }
            Err(err) => crate::log::error(format!("invalid theme in {path}: {err:#}")),
        }
    }
    themes
}

pub fn user_themes_dir() -> Option<std::path::PathBuf> {
    Some(crate::state::state_dir()?.join("themes"))
}

/// The active theme.
pub struct ActiveTokens(pub Arc<Tokens>);

impl Global for ActiveTokens {}

pub trait TokensExt {
    fn tokens(&self) -> &Arc<Tokens>;
}

impl TokensExt for App {
    fn tokens(&self) -> &Arc<Tokens> {
        &self.global::<ActiveTokens>().0
    }
}

/// A color with a different opacity, for subtle badge backgrounds.
pub fn tint(color: Hsla, alpha: f32) -> Hsla {
    let mut rgba: Rgba = color.into();
    rgba.a = alpha;
    rgba.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtins() -> Vec<Tokens> {
        load_themes(None).iter().map(|t| (**t).clone()).collect()
    }

    #[test]
    fn builtin_themes_parse_and_graphite_is_first() {
        let themes = builtins();
        assert_eq!(themes[0].name, "Graphite");
        assert!(
            themes
                .iter()
                .any(|t| t.name == "Graphite Light" && t.appearance == Appearance::Light)
        );
        assert_eq!(hex::format(&themes[0].ui.accent), "#82AAFF");
        assert_eq!(hex::format(&themes[0].ui.overlay), "#0506089E");
    }

    #[test]
    fn user_theme_inherits_from_base() {
        let source = serde_json::json!({
            "name": "Orange",
            "base": "Graphite Light",
            "ui": { "accent": "#FF9F43" }
        });
        let tokens = resolve(source, &builtins()).unwrap();
        assert_eq!(tokens.name, "Orange");
        assert_eq!(tokens.appearance, Appearance::Light);
        assert_eq!(hex::format(&tokens.ui.accent), "#FF9F43");
        assert_eq!(hex::format(&tokens.ui.background), "#FBFBFC");
    }

    #[test]
    fn rejects_bad_colors_and_unknown_keys() {
        let bad_color = serde_json::json!({ "name": "x", "ui": { "accent": "orange" } });
        assert!(resolve(bad_color, &builtins()).is_err());
        let typo = serde_json::json!({ "name": "x", "ui": { "acent": "#FFFFFF" } });
        assert!(resolve(typo, &builtins()).is_err());
        let missing_base = serde_json::json!({ "name": "x", "base": "Nope" });
        assert!(resolve(missing_base, &builtins()).is_err());
    }

    #[test]
    fn user_dir_themes_override_by_name() {
        let dir = std::env::temp_dir().join(format!("moth-themes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("mine.json"),
            r##"{ "name": "Mine", "ui": { "accent": "#00FF00" } }"##,
        )
        .unwrap();
        std::fs::write(dir.join("broken.json"), "{ not json").unwrap();
        let themes = load_themes(Some(&dir));
        let mine = themes.iter().find(|t| t.name == "Mine").unwrap();
        assert_eq!(hex::format(&mine.ui.accent), "#00FF00");
        assert_eq!(themes.len(), 3);
        std::fs::remove_dir_all(dir).ok();
    }
}
