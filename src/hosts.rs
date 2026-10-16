//! Identidad de los hosts remotos: etiqueta, color y si es de producción.
//!
//! Se configura en `~/Library/Application Support/terminalapp/hosts.json`. Cada regla se
//! compara (glob con `*` y `?`) contra el alias que escribiste y contra el hostname real; gana
//! la primera que coincide. Sin ese archivo se usan reglas por defecto para `prod` y `staging`.
//!
//! ```json
//! { "hosts": [
//!   { "match": "prod-*", "label": "PROD", "color": "red", "production": true },
//!   { "match": "*.staging.internal", "label": "STG", "color": "cyan" }
//! ] }
//! ```
//!
//! `color` puede ser un color ANSI del tema (`red`, `green`, `yellow`, `blue`, `magenta`,
//! `cyan`) o un hex `#RRGGBB`.

use std::path::{Path, PathBuf};

use gpui::{App, Global, Hsla};
use serde::Deserialize;

use crate::tokens::{TerminalColors, TokensExt};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct HostRule {
    #[serde(rename = "match")]
    pub pattern: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub production: bool,
}

#[derive(Deserialize)]
struct HostsFile {
    hosts: Vec<HostRule>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RulesSource {
    Defaults,
    File(PathBuf),
}

/// Reglas activas (global).
#[derive(Clone, Debug)]
pub struct HostRules {
    pub rules: Vec<HostRule>,
    pub source: RulesSource,
}

impl Global for HostRules {}

/// Color de un host: del tema (ANSI) o fijo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HostColor {
    Ansi(usize),
    Fixed(Hsla),
}

impl HostColor {
    pub fn resolve(&self, colors: &TerminalColors) -> Hsla {
        match self {
            HostColor::Ansi(index) => colors.ansi.get(*index),
            HostColor::Fixed(color) => *color,
        }
    }
}

/// Cómo se muestra un host: chip, color y si se trata como producción.
#[derive(Clone, Debug, PartialEq)]
pub struct HostIdentity {
    pub label: Option<String>,
    pub color: HostColor,
    pub production: bool,
    /// Patrón de la regla que coincidió, para explicarlo en la franja.
    pub matched: Option<String>,
}

impl HostIdentity {
    pub fn color(&self, cx: &App) -> Hsla {
        self.color.resolve(&cx.tokens().terminal)
    }
}

fn default_rules() -> Vec<HostRule> {
    let rule = |pattern: &str, label: &str, color: &str, production: bool| HostRule {
        pattern: pattern.into(),
        label: Some(label.into()),
        color: Some(color.into()),
        production,
    };
    vec![
        rule("*prod*", "PROD", "red", true),
        rule("*production*", "PROD", "red", true),
        rule("*live*", "PROD", "red", true),
        rule("*staging*", "STG", "cyan", false),
        rule("*stg*", "STG", "cyan", false),
    ]
}

pub fn hosts_file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Application Support/terminalapp/hosts.json"))
}

pub fn load_rules(path: Option<&Path>) -> HostRules {
    if let Some(path) = path
        && let Ok(text) = std::fs::read_to_string(path)
    {
        match serde_json::from_str::<HostsFile>(&text) {
            Ok(file) => {
                return HostRules {
                    rules: file.hosts,
                    source: RulesSource::File(path.to_path_buf()),
                };
            }
            Err(err) => eprintln!("hosts.json inválido ({}): {err}", path.display()),
        }
    }
    HostRules {
        rules: default_rules(),
        source: RulesSource::Defaults,
    }
}

/// Vuelve a leer `hosts.json` (se llama al abrir sesiones y "Conectar a…").
pub fn reload(cx: &mut App) {
    let rules = load_rules(hosts_file().as_deref());
    cx.set_global(rules);
}

/// Glob simple sin distinguir mayúsculas: `*` cualquier cosa, `?` un carácter.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

fn parse_color(name: &str) -> Option<HostColor> {
    let ansi = match name.to_lowercase().as_str() {
        "red" => Some(1),
        "green" => Some(2),
        "yellow" => Some(3),
        "blue" => Some(4),
        "magenta" | "purple" => Some(5),
        "cyan" | "teal" => Some(6),
        _ => None,
    };
    if let Some(index) = ansi {
        return Some(HostColor::Ansi(index));
    }
    let hex = name.strip_prefix('#').filter(|h| h.len() == 6)?;
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(HostColor::Fixed(gpui::rgb(value).into()))
}

/// Color estable por nombre para hosts sin regla (nunca rojo, que queda para producción).
fn hashed_color(name: &str) -> HostColor {
    const PALETTE: [usize; 4] = [4, 5, 6, 3];
    let hash = name
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    HostColor::Ansi(PALETTE[hash as usize % PALETTE.len()])
}

impl HostRules {
    pub fn identify(&self, alias: &str, hostname: Option<&str>) -> HostIdentity {
        let rule = self.rules.iter().find(|rule| {
            glob_match(&rule.pattern, alias) || hostname.is_some_and(|h| glob_match(&rule.pattern, h))
        });
        match rule {
            Some(rule) => HostIdentity {
                label: rule.label.clone(),
                color: rule
                    .color
                    .as_deref()
                    .and_then(parse_color)
                    .unwrap_or_else(|| {
                        if rule.production {
                            HostColor::Ansi(1)
                        } else {
                            hashed_color(alias)
                        }
                    }),
                production: rule.production,
                matched: Some(rule.pattern.clone()),
            },
            None => HostIdentity {
                label: None,
                color: hashed_color(alias),
                production: false,
                matched: None,
            },
        }
    }
}

pub fn rules(cx: &App) -> HostRules {
    cx.try_global::<HostRules>()
        .cloned()
        .unwrap_or_else(|| load_rules(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("prod-*", "prod-db-01"));
        assert!(glob_match("*prod*", "api.PROD.example.com"));
        assert!(glob_match("web-?", "web-3"));
        assert!(!glob_match("web-?", "web-12"));
        assert!(!glob_match("prod-*", "staging-db"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn defaults_mark_production_and_staging() {
        let rules = load_rules(None);
        let prod = rules.identify("prod-db", Some("10.0.4.12"));
        assert!(prod.production);
        assert_eq!(prod.label.as_deref(), Some("PROD"));
        assert_eq!(prod.color, HostColor::Ansi(1));
        let stg = rules.identify("api", Some("api.staging.internal"));
        assert!(!stg.production);
        assert_eq!(stg.label.as_deref(), Some("STG"));
        let other = rules.identify("raspberrypi.local", None);
        assert_eq!(other.label, None);
        assert_ne!(other.color, HostColor::Ansi(1));
        assert_eq!(other.color, rules.identify("raspberrypi.local", None).color);
    }

    #[test]
    fn file_rules_replace_defaults() {
        let dir = std::env::temp_dir().join(format!("tapp-hosts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hosts.json");
        std::fs::write(
            &path,
            r##"{ "hosts": [ { "match": "db-*", "label": "DB", "color": "#FF9F43", "production": true } ] }"##,
        )
        .unwrap();
        let rules = load_rules(Some(&path));
        assert_eq!(rules.source, RulesSource::File(path.clone()));
        let db = rules.identify("db-main", None);
        assert!(db.production);
        assert!(matches!(db.color, HostColor::Fixed(_)));
        // La regla por defecto de `prod` ya no aplica.
        assert!(!rules.identify("prod-web", None).production);
        std::fs::remove_dir_all(dir).ok();
    }
}
