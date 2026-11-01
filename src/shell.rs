//! Shell integration: installs the zsh script, detects its OSC sequences in the PTY
//! output (before they reach alacritty) and maintains the shared history.

use std::{
    collections::HashMap,
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

use crate::{assets::Assets, blocks::PromptMark};

/// What the shell tells us over OSC.
#[derive(Debug, Clone, PartialEq)]
pub enum ShellEvent {
    PromptStart,
    /// End of the prompt, where typed input begins. Leaves a mark in the grid (see `blocks`).
    PromptEnd(PromptMark),
    CommandStart,
    CommandFinished(Option<i32>),
    CommandText(String),
    Cwd(PathBuf),
    Line {
        cursor: usize,
        buffer: String,
    },
    LineFinished,
    HistoryFile(PathBuf),
    Path(String),
    Names(String),
}

/// Environment variables that make zsh load the integration. `None` if the shell isn't zsh
/// or the files couldn't be written.
pub fn integration_env(program: &str) -> Option<HashMap<String, String>> {
    let name = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str());
    if name != Some("zsh") {
        return None;
    }
    let dir = install_dir()?;
    let mut env = HashMap::from([
        ("ZDOTDIR".to_string(), dir.display().to_string()),
        (
            "MOTH_SHELL_INTEGRATION".to_string(),
            dir.join("moth.zsh").display().to_string(),
        ),
    ]);
    if let Ok(orig) = std::env::var("ZDOTDIR") {
        env.insert("MOTH_ORIG_ZDOTDIR".to_string(), orig);
    }
    Some(env)
}

fn install_dir() -> Option<PathBuf> {
    let dir = crate::state::state_dir()?.join("shell/zsh");
    let write = || -> io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        for (asset, name) in [
            ("shell/zsh/zshenv", ".zshenv"),
            ("shell/zsh/moth.zsh", "moth.zsh"),
        ] {
            let data = Assets::get(asset).ok_or(io::ErrorKind::NotFound)?.data;
            let target = dir.join(name);
            // Only rewrite it if it changed, so the file isn't touched on every tab.
            if std::fs::read(&target).ok().as_deref() != Some(&data[..]) {
                std::fs::write(&target, &data)?;
            }
        }
        Ok(())
    };
    match write() {
        Ok(()) => Some(dir),
        Err(err) => {
            crate::log::error(format!("failed to install the zsh integration: {err}"));
            None
        }
    }
}

/// The shell each tab runs and its arguments: `shell` from the settings (a command line) if
/// set; otherwise `$SHELL` as a login shell on macOS and Linux, and PowerShell on Windows
/// (PowerShell 7 when it's installed, Windows PowerShell otherwise).
pub fn shell_command(configured: Option<&str>) -> (String, Vec<String>) {
    if let Some(command) = configured
        .map(str::trim)
        .filter(|command| !command.is_empty())
    {
        let mut parts = command.split_whitespace().map(str::to_string);
        if let Some(program) = parts.next() {
            return (program, parts.collect());
        }
    }
    if cfg!(windows) {
        let has_pwsh = std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path).any(|dir| dir.join("pwsh.exe").is_file())
        });
        let program = if has_pwsh {
            "pwsh.exe"
        } else {
            "powershell.exe"
        };
        return (program.into(), vec!["-NoLogo".into()]);
    }
    let fallback = if cfg!(target_os = "macos") {
        "/bin/zsh"
    } else {
        "/bin/bash"
    };
    let program = std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| fallback.into());
    // A login shell loads .zprofile/.bash_profile and the rc files. Without an explicit shell,
    // alacritty goes through /usr/bin/login, which prints "Last login: …" on startup.
    (program, vec!["-l".into()])
}

/// The shell's name for the tab title: `zsh`, `pwsh`…
pub fn shell_name(program: &str) -> String {
    let name = Path::new(program)
        .file_stem()
        .and_then(|name| name.to_str());
    name.unwrap_or("shell").to_string()
}

/// `LANG` for the shell when the app didn't receive a locale (launching from Finder doesn't
/// provide one): without UTF-8, zsh and many programs mangle non-ASCII characters. It's
/// derived from the macOS language, as Terminal.app does.
pub fn locale_env() -> Option<(String, String)> {
    let has_locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .any(|var| std::env::var(var).is_ok_and(|v| !v.is_empty()));
    // Only macOS launches apps without a locale; elsewhere the app's environment has one.
    if has_locale || !cfg!(target_os = "macos") {
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
// OSC detection

const MAX_OSC_LEN: usize = 64 * 1024;

#[derive(Default)]
enum ScanState {
    #[default]
    Ground,
    Escape,
    Osc,
    OscEscape,
}

/// Minimal state machine that extracts the payload of `ESC ] … (BEL | ESC \)`.
/// It doesn't alter the stream: alacritty still receives every byte and ignores these OSCs.
#[derive(Default)]
pub struct OscScanner {
    state: ScanState,
    payload: Vec<u8>,
}

impl OscScanner {
    /// Calls `on_osc` with each sequence's payload and the position in `bytes` of the
    /// last byte of its terminator.
    pub fn feed(&mut self, bytes: &[u8], mut on_osc: impl FnMut(&[u8], usize)) {
        for (ix, &byte) in bytes.iter().enumerate() {
            self.state = match self.state {
                ScanState::Ground | ScanState::Escape if byte == 0x1b => ScanState::Escape,
                ScanState::Escape if byte == b']' => {
                    self.payload.clear();
                    ScanState::Osc
                }
                ScanState::Ground | ScanState::Escape => ScanState::Ground,
                ScanState::Osc => match byte {
                    0x07 => {
                        on_osc(&self.payload, ix);
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
                        on_osc(&self.payload, ix);
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
                "B" => {
                    let mut mark = PromptMark {
                        rows: 1,
                        previous_failed: false,
                    };
                    for param in parts {
                        match param.split_once('=') {
                            Some(("rows", rows)) => mark.rows = rows.parse().unwrap_or(1),
                            Some(("prev_err", err)) => mark.previous_failed = err == "1",
                            _ => {}
                        }
                    }
                    Some(ShellEvent::PromptEnd(mark))
                }
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
// Scanning PTY

pub struct ScanningReader {
    pty: Pty,
    scanner: OscScanner,
    events: UnboundedSender<ShellEvent>,
}

/// Space left free on each read to insert the prompt marks. They go in the same buffer and
/// aren't kept for the next read: alacritty's event loop doesn't read again until the PTY has
/// new data, and whatever was kept would be stuck.
const MARK_ROOM: usize = 64;

impl Read for ScanningReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let room = if buf.len() > 4 * MARK_ROOM {
            MARK_ROOM
        } else {
            0
        };
        let readable = buf.len() - room;
        let n = self.pty.reader().read(&mut buf[..readable])?;
        let events = &self.events;
        let mut marks = Vec::new();
        self.scanner.feed(&buf[..n], |payload, end| {
            if let Some(event) = parse_osc(payload) {
                if let ShellEvent::PromptEnd(mark) = &event {
                    marks.push((end + 1, mark.encode()));
                }
                events.unbounded_send(event).ok();
            }
        });
        Ok(insert_marks(buf, n, room, &marks))
    }
}

/// Inserts each mark at its position while it fits in `room`; returns the new length.
fn insert_marks(buf: &mut [u8], len: usize, room: usize, marks: &[(usize, String)]) -> usize {
    let mut len = len;
    let mut used = 0;
    // Back to front, so the positions of earlier marks stay valid.
    for (at, mark) in marks.iter().rev() {
        let mark = mark.as_bytes();
        if used + mark.len() > room {
            continue;
        }
        buf.copy_within(*at..len, at + mark.len());
        buf[*at..at + mark.len()].copy_from_slice(mark);
        len += mark.len();
        used += mark.len();
    }
    len
}

/// Wraps alacritty's `Pty` (a Unix PTY or a Windows ConPTY): everything goes straight to it,
/// except reads, which go through the scanner first.
pub struct ShellPty {
    reader: ScanningReader,
}

impl ShellPty {
    pub fn new(pty: Pty, events: UnboundedSender<ShellEvent>) -> Self {
        Self {
            reader: ScanningReader {
                pty,
                scanner: OscScanner::default(),
                events,
            },
        }
    }
}

impl EventedReadWrite for ShellPty {
    type Reader = ScanningReader;
    type Writer = <Pty as EventedReadWrite>::Writer;

    unsafe fn register(
        &mut self,
        poll: &Arc<Poller>,
        event: Event,
        mode: PollMode,
    ) -> io::Result<()> {
        unsafe { self.reader.pty.register(poll, event, mode) }
    }

    fn reregister(&mut self, poll: &Arc<Poller>, event: Event, mode: PollMode) -> io::Result<()> {
        self.reader.pty.reregister(poll, event, mode)
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        self.reader.pty.deregister(poll)
    }

    fn reader(&mut self) -> &mut ScanningReader {
        &mut self.reader
    }

    fn writer(&mut self) -> &mut Self::Writer {
        self.reader.pty.writer()
    }
}

impl EventedPty for ShellPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.reader.pty.next_child_event()
    }
}

impl OnResize for ShellPty {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.reader.pty.on_resize(window_size)
    }
}

// ---------------------------------------------------------------------------------------
// History

/// Command history shared by all tabs, most recent last.
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

    /// The rest of the most recent entry that starts with `prefix`.
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

/// Reads a `.zsh_history` (simple or extended format, with zsh's "metafication").
pub fn read_history_file(path: &Path) -> Vec<String> {
    let Ok(raw) = std::fs::read(path) else {
        return Vec::new();
    };
    // zsh escapes special bytes as 0x83 followed by the byte XOR 32.
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
        // Lines ending in `\` continue the previous command.
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

    // Deduplicate, keeping the most recent occurrence.
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
        OscScanner::default().feed(bytes, |p, _| out.extend(parse_osc(p)));
        out
    }

    #[test]
    fn parses_shell_markers() {
        let events = scan(
            b"hello\x1b]133;A\x07$ \x1b]133;B\x07ls\x1b]7777;E;ls -la\x07\x1b]133;C\x07out\x1b]133;D;1\x1b\\",
        );
        assert_eq!(
            events,
            vec![
                ShellEvent::PromptStart,
                ShellEvent::PromptEnd(PromptMark {
                    rows: 1,
                    previous_failed: false
                }),
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
        scanner.feed(b"\x1b]7777;L;3;git", |p, _| out.extend(parse_osc(p)));
        scanner.feed(b" st\x07", |p, _| out.extend(parse_osc(p)));
        assert_eq!(
            out,
            vec![ShellEvent::Line {
                cursor: 3,
                buffer: "git st".into()
            }]
        );
    }

    #[test]
    fn prompt_end_carries_rows_and_previous_result() {
        assert_eq!(
            scan(b"\x1b]133;B;rows=3;prev_err=1\x07"),
            vec![ShellEvent::PromptEnd(PromptMark {
                rows: 3,
                previous_failed: true
            })]
        );
    }

    #[test]
    fn inserts_marks_after_the_prompt_end() {
        let mark = PromptMark {
            rows: 2,
            previous_failed: false,
        }
        .encode();
        let input = b"$ \x1b]133;B\x07ls";
        let mut buf = [0u8; 64];
        buf[..input.len()].copy_from_slice(input);
        let mut ends = Vec::new();
        OscScanner::default().feed(&buf[..input.len()], |_, end| ends.push(end));
        let marks: Vec<_> = ends.iter().map(|&end| (end + 1, mark.clone())).collect();
        let len = insert_marks(&mut buf, input.len(), 32, &marks);
        let expected = format!("$ \x1b]133;B\x07{mark}ls");
        assert_eq!(&buf[..len], expected.as_bytes());
        // With no room, the text is unchanged.
        let mut buf = input.to_vec();
        assert_eq!(insert_marks(&mut buf, input.len(), 0, &marks), input.len());
    }

    #[test]
    fn decodes_cwd() {
        assert_eq!(
            scan(b"\x1b]7;file://mac.local/Users/dev/my%20project\x07"),
            vec![ShellEvent::Cwd("/Users/dev/my project".into())]
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
        let dir = std::env::temp_dir().join(format!("moth-hist-{}", std::process::id()));
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
