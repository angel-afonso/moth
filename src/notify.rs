//! Notificaciones de macOS. La app se ejecuta como binario suelto, sin bundle firmado, y así
//! `UNUserNotificationCenter` no funciona: se usa `osascript`, que sí.

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
        // Se espera en otro hilo para no dejar el proceso zombi.
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(err) => crate::log::error(format!("no se pudo mostrar la notificación: {err}")),
    }
}

/// Un string de AppleScript: entre comillas, con `\` y `"` escapados.
fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    #[test]
    fn quotes_applescript_strings() {
        assert_eq!(
            super::quote(r#"echo "hola" \ chau"#),
            r#""echo \"hola\" \\ chau""#
        );
    }
}
