//! Desktop notifications, through what each system has without extra dependencies:
//! `osascript` on macOS (the app isn't a signed bundle registered with
//! `UNUserNotificationCenter`), `notify-send` on Linux and a PowerShell toast on Windows.

use std::process::Command;

pub fn send(title: &str, body: &str) {
    if let Err(err) = crate::process::spawn_detached(&mut command(title, body)) {
        crate::log::error(format!("couldn't show the notification: {err}"));
    }
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn quotes_applescript_strings() {
        assert_eq!(
            super::quote(r#"echo "hello" \ bye"#),
            r#""echo \"hello\" \\ bye""#
        );
    }
}
