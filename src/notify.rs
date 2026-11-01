//! macOS notifications. The app runs as a bare binary without a signed bundle, so
//! `UNUserNotificationCenter` doesn't work; `osascript` does.

pub fn send(title: &str, body: &str) {
    let script = format!(
        "display notification {} with title {}",
        quote(body),
        quote(title)
    );
    let child = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match child {
        // Reap it on another thread so it doesn't become a zombie.
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(err) => crate::log::error(format!("couldn't show the notification: {err}")),
    }
}

/// An AppleScript string: quoted, with `\` and `"` escaped.
fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    #[test]
    fn quotes_applescript_strings() {
        assert_eq!(
            super::quote(r#"echo "hello" \ bye"#),
            r#""echo \"hello\" \\ bye""#
        );
    }
}
