//! Integración con la shell: instala el script de zsh, detecta sus secuencias OSC en la
//! salida del PTY (antes de que lleguen a alacritty) y mantiene el historial compartido.

use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

use alacritty_terminal::{
    event::{OnResize, WindowSize},
    tty::{ChildEvent, EventedPty, EventedReadWrite, Pty},
};
use futures::channel::mpsc::UnboundedSender;
use gpui::{App, Global};
use polling::{Event, PollMode, Poller};

use crate::assets::Assets;

/// Lo que la shell nos cuenta por OSC.
#[derive(Debug, Clone, PartialEq)]
pub enum ShellEvent {
    PromptStart,
    PromptEnd,
    CommandStart,
    CommandFinished(Option<i32>),
    CommandText(String),
    Cwd(PathBuf),
    Line { cursor: usize, buffer: String },
    LineFinished,
    HistoryFile(PathBuf),
    Path(String),
    Names(String),
}

/// Variables de entorno para que zsh cargue la integración. `None` si la shell no es zsh
/// o si no se pudieron escribir los archivos.
pub fn integration_env() -> Option<HashMap<String, String>> {
    let shell = std::env::var("SHELL").unwrap_or_default();
    if !shell.ends_with("/zsh") {
        return None;
    }
    let dir = install_dir()?;
    let mut env = HashMap::from([
        ("ZDOTDIR".to_string(), dir.display().to_string()),
        (
            "TERMINALAPP_SHELL_INTEGRATION".to_string(),
            dir.join("terminalapp.zsh").display().to_string(),
        ),
    ]);
    if let Ok(orig) = std::env::var("ZDOTDIR") {
        env.insert("TERMINALAPP_ORIG_ZDOTDIR".to_string(), orig);
    }
    Some(env)
}

fn install_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = Path::new(&home).join("Library/Application Support/terminalapp/shell/zsh");
    let write = || -> io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        for (asset, name) in [
            ("shell/zsh/zshenv", ".zshenv"),
            ("shell/zsh/terminalapp.zsh", "terminalapp.zsh"),
        ] {
            let data = Assets::get(asset).ok_or(io::ErrorKind::NotFound)?.data;
            let target = dir.join(name);
            // Se reescribe solo si cambió, para no tocar el archivo en cada pestaña.
            if std::fs::read(&target).ok().as_deref() != Some(&data[..]) {
                std::fs::write(&target, &data)?;
            }
        }
        Ok(())
    };
    match write() {
        Ok(()) => Some(dir),
        Err(err) => {
            crate::log::error(format!("no se pudo instalar la integración de zsh: {err}"));
            None
        }
    }
}

/// `LANG` para la shell cuando la app no recibió locale (lanzada desde el Finder no lo trae):
/// sin UTF-8, zsh y muchos programas tratan mal los acentos. Se deriva del idioma de macOS,
/// como hace Terminal.app.
pub fn locale_env() -> Option<(String, String)> {
    let has_locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .any(|var| std::env::var(var).is_ok_and(|v| !v.is_empty()));
    if has_locale {
        return None;
    }
    static LANG: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let lang = LANG.get_or_init(|| {
        let apple_locale = std::process::Command::new("defaults")
            .args(["read", "-g", "AppleLocale"])
            .output()
            .ok()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_default();
        let base = apple_locale.split('@').next().unwrap_or_default();
        let candidate = format!("{base}.UTF-8");
        if !base.is_empty() && Path::new("/usr/share/locale").join(&candidate).is_dir() {
            candidate
        } else {
            "en_US.UTF-8".to_string()
        }
    });
    Some(("LANG".to_string(), lang.clone()))
}

// ---------------------------------------------------------------------------------------
// Detección de OSC

const MAX_OSC_LEN: usize = 64 * 1024;

#[derive(Default)]
enum ScanState {
    #[default]
    Ground,
    Escape,
    Osc,
    OscEscape,
}

/// Máquina de estados mínima que extrae el contenido de `ESC ] … (BEL | ESC \)`.
/// No altera el flujo: alacritty sigue recibiendo todos los bytes e ignora estos OSC.
#[derive(Default)]
pub struct OscScanner {
    state: ScanState,
    payload: Vec<u8>,
}

impl OscScanner {
    pub fn feed(&mut self, bytes: &[u8], mut on_osc: impl FnMut(&[u8])) {
        for &byte in bytes {
            self.state = match self.state {
                ScanState::Ground | ScanState::Escape if byte == 0x1b => ScanState::Escape,
                ScanState::Escape if byte == b']' => {
                    self.payload.clear();
                    ScanState::Osc
                }
                ScanState::Ground | ScanState::Escape => ScanState::Ground,
                ScanState::Osc => match byte {
                    0x07 => {
                        on_osc(&self.payload);
                        ScanState::Ground
                    }
                    0x1b => ScanState::OscEscape,
                    _ => {
                        if self.payload.len() < MAX_OSC_LEN {
                            self.payload.push(byte);
                        }
                        ScanState::Osc
                    }
                },
                ScanState::OscEscape => match byte {
                    b'\\' => {
                        on_osc(&self.payload);
                        ScanState::Ground
                    }
                    b']' => {
                        self.payload.clear();
                        ScanState::Osc
                    }
                    0x1b => ScanState::Escape,
                    _ => ScanState::Ground,
                },
            };
        }
    }
}

pub fn parse_osc(payload: &[u8]) -> Option<ShellEvent> {
    let text = std::str::from_utf8(payload).ok()?;
    let (code, rest) = text.split_once(';').unwrap_or((text, ""));
    match code {
        "133" => {
            let mut parts = rest.split(';');
            match parts.next()? {
                "A" => Some(ShellEvent::PromptStart),
                "B" => Some(ShellEvent::PromptEnd),
                "C" => Some(ShellEvent::CommandStart),
                "D" => Some(ShellEvent::CommandFinished(
                    parts.next().and_then(|c| c.parse().ok()),
                )),
                _ => None,
            }
        }
        "7" => {
            let url = rest.strip_prefix("file://")?;
            let path = &url[url.find('/')?..];
            Some(ShellEvent::Cwd(PathBuf::from(percent_decode(path))))
        }
        "7777" => {
            let (kind, rest) = rest.split_once(';').unwrap_or((rest, ""));
            match kind {
                "E" => Some(ShellEvent::CommandText(rest.to_string())),
                "L" => {
                    let (cursor, buffer) = rest.split_once(';')?;
                    Some(ShellEvent::Line {
                        cursor: cursor.parse().ok()?,
                        buffer: buffer.to_string(),
                    })
                }
                "F" => Some(ShellEvent::LineFinished),
                "H" => Some(ShellEvent::HistoryFile(PathBuf::from(rest))),
                "P" => Some(ShellEvent::Path(rest.to_string())),
                "N" => Some(ShellEvent::Names(rest.to_string())),
                _ => None,
            }
        }
        _ => None,
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&text[i + 1..i + 3], 16)
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------------------
// PTY con escáner

pub struct ScanningReader {
    file: File,
    scanner: OscScanner,
    events: UnboundedSender<ShellEvent>,
}

impl Read for ScanningReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.file.read(buf)?;
        let events = &self.events;
        self.scanner.feed(&buf[..n], |payload| {
            if let Some(event) = parse_osc(payload) {
                events.unbounded_send(event).ok();
            }
        });
        Ok(n)
    }
}

/// Envuelve el `Pty` de alacritty: lee por un descriptor duplicado que pasa por el escáner.
pub struct ShellPty {
    pty: Pty,
    reader: ScanningReader,
}

impl ShellPty {
    pub fn new(pty: Pty, events: UnboundedSender<ShellEvent>) -> io::Result<Self> {
        let file = pty.file().try_clone()?;
        Ok(Self {
            pty,
            reader: ScanningReader {
                file,
                scanner: OscScanner::default(),
                events,
            },
        })
    }
}

impl EventedReadWrite for ShellPty {
    type Reader = ScanningReader;
    type Writer = File;

    unsafe fn register(
        &mut self,
        poll: &Arc<Poller>,
        event: Event,
        mode: PollMode,
    ) -> io::Result<()> {
        unsafe { self.pty.register(poll, event, mode) }
    }

    fn reregister(&mut self, poll: &Arc<Poller>, event: Event, mode: PollMode) -> io::Result<()> {
        self.pty.reregister(poll, event, mode)
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        self.pty.deregister(poll)
    }

    fn reader(&mut self) -> &mut ScanningReader {
        &mut self.reader
    }

    fn writer(&mut self) -> &mut File {
        self.pty.writer()
    }
}

impl EventedPty for ShellPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.pty.next_child_event()
    }
}

impl OnResize for ShellPty {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.pty.on_resize(window_size)
    }
}

// ---------------------------------------------------------------------------------------
// Historial

/// Historial de comandos compartido por todas las pestañas, el más reciente al final.
#[derive(Default)]
pub struct History {
    pub loaded_from: Option<PathBuf>,
    pub entries: Vec<String>,
}

impl Global for History {}

impl History {
    pub fn push(&mut self, command: &str) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }
        self.entries.retain(|e| e != command);
        self.entries.push(command.to_string());
    }

    /// El resto de la entrada más reciente que empieza con `prefix`.
    pub fn suggest(&self, prefix: &str) -> Option<&str> {
        if prefix.trim().is_empty() {
            return None;
        }
        self.entries
            .iter()
            .rev()
            .find(|e| e.len() > prefix.len() && e.starts_with(prefix))
            .map(|e| &e[prefix.len()..])
    }
}

pub fn history(cx: &App) -> Option<&History> {
    cx.try_global::<History>()
}

/// Lee un `.zsh_history` (formato simple o extendido, con la "metafication" de zsh).
pub fn read_history_file(path: &Path) -> Vec<String> {
    let Ok(raw) = std::fs::read(path) else {
        return Vec::new();
    };
    // zsh escapa bytes especiales como 0x83 seguido del byte XOR 32.
    let mut bytes = Vec::with_capacity(raw.len());
    let mut iter = raw.into_iter();
    while let Some(b) = iter.next() {
        if b == 0x83 {
            if let Some(next) = iter.next() {
                bytes.push(next ^ 32);
            }
        } else {
            bytes.push(b);
        }
    }
    let text = String::from_utf8_lossy(&bytes);

    let mut entries: Vec<String> = Vec::new();
    let mut pending = String::new();
    for line in text.lines() {
        // Las líneas terminadas en `\` continúan el comando anterior.
        if let Some(stripped) = line.strip_suffix('\\') {
            pending.push_str(stripped);
            pending.push('\n');
            continue;
        }
        pending.push_str(line);
        let command = match pending.strip_prefix(": ") {
            Some(rest) => rest.split_once(';').map(|(_, c)| c).unwrap_or(rest),
            None => pending.as_str(),
        };
        let command = command.trim();
        if !command.is_empty() && !command.contains('\n') {
            entries.push(command.to_string());
        }
        pending.clear();
    }

    // Sin duplicados, conservando la aparición más reciente.
    let mut seen = std::collections::HashSet::new();
    let mut deduped: Vec<String> = entries
        .into_iter()
        .rev()
        .filter(|e| seen.insert(e.clone()))
        .collect();
    deduped.reverse();
    deduped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(bytes: &[u8]) -> Vec<ShellEvent> {
        let mut out = Vec::new();
        OscScanner::default().feed(bytes, |p| out.extend(parse_osc(p)));
        out
    }

    #[test]
    fn parses_shell_markers() {
        let events = scan(
            b"hola\x1b]133;A\x07$ \x1b]133;B\x07ls\x1b]7777;E;ls -la\x07\x1b]133;C\x07out\x1b]133;D;1\x1b\\",
        );
        assert_eq!(
            events,
            vec![
                ShellEvent::PromptStart,
                ShellEvent::PromptEnd,
                ShellEvent::CommandText("ls -la".into()),
                ShellEvent::CommandStart,
                ShellEvent::CommandFinished(Some(1)),
            ]
        );
    }

    #[test]
    fn handles_sequences_split_across_reads() {
        let mut scanner = OscScanner::default();
        let mut out = Vec::new();
        scanner.feed(b"\x1b]7777;L;3;git", |p| out.extend(parse_osc(p)));
        scanner.feed(b" st\x07", |p| out.extend(parse_osc(p)));
        assert_eq!(
            out,
            vec![ShellEvent::Line {
                cursor: 3,
                buffer: "git st".into()
            }]
        );
    }

    #[test]
    fn decodes_cwd() {
        assert_eq!(
            scan(b"\x1b]7;file://mac.local/Users/dev/mi%20proyecto\x07"),
            vec![ShellEvent::Cwd("/Users/dev/mi proyecto".into())]
        );
    }

    #[test]
    fn suggests_most_recent_match() {
        let mut history = History::default();
        history.push("git push origin main");
        history.push("git pull --rebase");
        history.push("ls");
        assert_eq!(history.suggest("git p"), Some("ull --rebase"));
        assert_eq!(history.suggest("git pus"), Some("h origin main"));
        assert_eq!(history.suggest("ls"), None);
        assert_eq!(history.suggest(""), None);
    }

    #[test]
    fn reads_extended_history() {
        let dir = std::env::temp_dir().join(format!("tapp-hist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hist");
        std::fs::write(
            &file,
            b": 1700000000:0;ls\n: 1700000001:0;cargo build\n: 1700000002:0;ls\n",
        )
        .unwrap();
        assert_eq!(read_history_file(&file), vec!["cargo build", "ls"]);
        std::fs::remove_dir_all(dir).ok();
    }
}
