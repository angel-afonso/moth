//! Keyboard shortcuts on every platform. They're written the macOS way (`cmd-t`) and translated
//! on Linux and Windows, where Ctrl alone belongs to the shell (Ctrl+C, Ctrl+W, Ctrl+R…): there
//! `cmd` becomes `ctrl-shift`, as in GNOME Terminal and Windows Terminal, and a shortcut that
//! already had Shift or Alt gets both.

use gpui::{Action, KeyBinding};

/// The binding for `mac` on this platform, or `None` when it has no equivalent there.
pub fn key(mac: &str) -> Option<String> {
    if cfg!(target_os = "macos") {
        Some(mac.to_string())
    } else {
        translate(mac)
    }
}

/// A key binding from a macOS-style shortcut, for this platform.
pub fn bind<A: Action>(mac: &str, action: A, context: Option<&str>) -> Option<KeyBinding> {
    Some(KeyBinding::new(&key(mac)?, action, context))
}

fn translate(mac: &str) -> Option<String> {
    let Some(rest) = mac.strip_prefix("cmd-") else {
        return Some(mac.to_string());
    };
    let translated = match rest {
        // Tabs by number: Alt+1…9, as in most Linux terminals.
        digit if digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit() => {
            format!("alt-{digit}")
        }
        // Zoom with Ctrl alone, as in browsers and Windows Terminal.
        "=" | "+" | "-" | "0" => format!("ctrl-{rest}"),
        // The command palette and running from a picker, as in VS Code and Windows Terminal.
        "shift-p" => "ctrl-shift-p".into(),
        "enter" => "ctrl-enter".into(),
        // Brackets typed with Shift are different keys on many layouts, and minimizing is the
        // window manager's job: panes and tabs have other shortcuts.
        "[" | "]" | "{" | "}" | "m" => return None,
        _ => match rest
            .strip_prefix("shift-")
            .or_else(|| rest.strip_prefix("alt-"))
        {
            Some(key) => format!("ctrl-shift-alt-{key}"),
            None => format!("ctrl-shift-{rest}"),
        },
    };
    Some(translated)
}

/// How a shortcut reads in labels written by hand (tab tooltips, picker hints): `⇧⌘R` on
/// macOS, `Ctrl-Alt-Shift-R` elsewhere, as gpui writes the ones it draws. Empty when the
/// shortcut doesn't exist here.
pub fn label(mac: &str) -> String {
    key(mac)
        .map(|key| format_key(&key, cfg!(target_os = "macos")))
        .unwrap_or_default()
}

fn format_key(binding: &str, mac: bool) -> String {
    let mut rest = binding;
    let (mut ctrl, mut alt, mut shift, mut cmd) = (false, false, false, false);
    loop {
        let flag = match rest {
            _ if rest.starts_with("ctrl-") && rest.len() > 5 => &mut ctrl,
            _ if rest.starts_with("alt-") && rest.len() > 4 => &mut alt,
            _ if rest.starts_with("shift-") && rest.len() > 6 => &mut shift,
            _ if rest.starts_with("cmd-") && rest.len() > 4 => &mut cmd,
            _ => break,
        };
        *flag = true;
        rest = &rest[rest.find('-').unwrap_or(0) + 1..];
    }
    let key = match (rest, mac) {
        ("enter", true) => "⏎".to_string(),
        ("enter", false) => "Enter".to_string(),
        ("up", _) => "↑".to_string(),
        ("down", _) => "↓".to_string(),
        ("left", _) => "←".to_string(),
        ("right", _) => "→".to_string(),
        ("escape", _) => "Esc".to_string(),
        ("tab", _) => "Tab".to_string(),
        ("space", _) => "Space".to_string(),
        (key, _) => key.to_uppercase(),
    };
    if mac {
        // Apple's order: ⌃⌥⇧⌘.
        [(ctrl, "⌃"), (alt, "⌥"), (shift, "⇧"), (cmd, "⌘")]
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, symbol)| *symbol)
            .chain([key.as_str()])
            .collect()
    } else {
        [
            (ctrl, "Ctrl"),
            (alt, "Alt"),
            (shift, "Shift"),
            (cmd, "Super"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| *name)
        .chain([key.as_str()])
        .collect::<Vec<_>>()
        .join("-")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_mac_shortcuts_for_linux_and_windows() {
        assert_eq!(translate("cmd-t").as_deref(), Some("ctrl-shift-t"));
        assert_eq!(
            translate("cmd-shift-t").as_deref(),
            Some("ctrl-shift-alt-t")
        );
        assert_eq!(
            translate("cmd-alt-left").as_deref(),
            Some("ctrl-shift-alt-left")
        );
        assert_eq!(translate("cmd-3").as_deref(), Some("alt-3"));
        assert_eq!(translate("cmd--").as_deref(), Some("ctrl--"));
        assert_eq!(translate("cmd-shift-p").as_deref(), Some("ctrl-shift-p"));
        assert_eq!(translate("cmd-[").as_deref(), None);
        assert_eq!(translate("ctrl-tab").as_deref(), Some("ctrl-tab"));
    }

    #[test]
    fn formats_labels_per_platform() {
        assert_eq!(format_key("cmd-shift-r", true), "⇧⌘R");
        assert_eq!(format_key("cmd-enter", true), "⌘⏎");
        assert_eq!(format_key("ctrl-shift-alt-r", false), "Ctrl-Alt-Shift-R");
        assert_eq!(format_key("alt-3", false), "Alt-3");
        assert_eq!(format_key("ctrl--", false), "Ctrl--");
        assert_eq!(format_key("ctrl-enter", false), "Ctrl-Enter");
    }
}
