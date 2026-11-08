//! Desktop notifications. On macOS they come from the app itself, through gpui, so the system
//! shows Moth's name and icon and lists it in its settings. That needs an app bundle: a bare
//! binary (`cargo run`) goes through `osascript`, which macOS credits to Script Editor. Linux
//! uses `notify-send` and Windows a PowerShell toast.

use std::{
    path::Path,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use gpui::{App, SystemNotification};

pub fn send(title: &str, body: &str, cx: &App) {
    let bundled = std::env::current_exe().is_ok_and(|exe| in_app_bundle(&exe));
    if cfg!(target_os = "macos") && bundled {
        // Each one has its own tag: the same tag would replace the previous notification.
        let sent = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        cx.show_system_notification(SystemNotification {
            tag: format!("command-{}", sent.as_millis()).into(),
            title: title.to_string().into(),
            body: body.to_string().into(),
            actions: Vec::new(),
        });
    } else if let Err(err) = crate::process::spawn_detached(&mut command(title, body)) {
        crate::log::error(format!("couldn't show the notification: {err}"));
    }
}

/// Whether `exe` is the program of a macOS app: `Moth.app/Contents/MacOS/moth`.
fn in_app_bundle(exe: &Path) -> bool {
    let mut folders = exe.ancestors().skip(1);
    folders
        .next()
        .is_some_and(|dir| dir.ends_with("Contents/MacOS"))
        && folders
            .nth(1)
            .is_some_and(|app| app.extension().is_some_and(|ext| ext == "app"))
}

#[cfg(target_os = "macos")]
fn command(title: &str, body: &str) -> Command {
    let script = format!(
        "display notification {} with title {}",
        quote(body),
        quote(title)
    );
    let mut command = Command::new("/usr/bin/osascript");
    command.args(["-e", &script]);
    command
}

/// An AppleScript string: quoted, with `\` and `"` escaped.
#[cfg(target_os = "macos")]
fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(windows)]
fn command(title: &str, body: &str) -> Command {
    // The texts go through the environment, so nothing in them is read as PowerShell. Toasts
    // need an app id the system knows: Windows PowerShell's own.
    const SCRIPT: &str = "\
        [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null; \
        $toast = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
        $texts = $toast.GetElementsByTagName('text'); \
        $texts.Item(0).AppendChild($toast.CreateTextNode($env:MOTH_NOTIFY_TITLE)) > $null; \
        $texts.Item(1).AppendChild($toast.CreateTextNode($env:MOTH_NOTIFY_BODY)) > $null; \
        $app = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe'; \
        [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($app).Show([Windows.UI.Notifications.ToastNotification]::new($toast))";
    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("MOTH_NOTIFY_TITLE", title)
        .env("MOTH_NOTIFY_BODY", body);
    command
}

#[cfg(not(any(target_os = "macos", windows)))]
fn command(title: &str, body: &str) -> Command {
    let mut command = Command::new("notify-send");
    command.args(["--app-name=Moth", title, body]);
    command
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn only_the_program_of_an_app_is_in_a_bundle() {
        let bundled = |path: &str| super::in_app_bundle(Path::new(path));
        assert!(bundled("/Applications/Moth.app/Contents/MacOS/moth"));
        assert!(!bundled("/repo/target/debug/moth"));
        assert!(!bundled("/repo/dist/moth"));
        assert!(!bundled("/tmp/Contents/MacOS/moth"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn quotes_applescript_strings() {
        assert_eq!(
            super::quote(r#"echo "hello" \ bye"#),
            r#""echo \"hello\" \\ bye""#
        );
    }
}
