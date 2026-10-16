//! Modelo de una sesión: PTY + emulador de alacritty, con eventos reenviados a gpui.

use std::{
    borrow::Cow,
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use alacritty_terminal::{
    event::{Event as AlacEvent, EventListener, WindowSize},
    event_loop::{EventLoop, EventLoopSender, Msg},
    grid::{Dimensions, Scroll},
    sync::FairMutex,
    term::{Config, Term, TermMode},
    tty,
};
use anyhow::Result;
use futures::{
    FutureExt as _, StreamExt,
    channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded},
};
use gpui::{App, ClipboardItem, Context, EventEmitter, Pixels, Task, px};

use crate::{
    completions,
    git::{self, GitStatus},
    hosts::{self, HostIdentity},
    ssh::{self, ResolvedHost, SshTarget},
    highlight::{self, CommandResolver, Token},
    palette,
    shell::{self, History, ShellEvent, ShellPty},
    state,
    tokens::TokensExt,
};

#[derive(Clone)]
pub struct Listener(UnboundedSender<AlacEvent>);

impl EventListener for Listener {
    fn send_event(&self, event: AlacEvent) {
        self.0.unbounded_send(event).ok();
    }
}

/// Tamaño de la grilla en celdas, más el tamaño de celda que reporta el PTY.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TermSize {
    pub columns: usize,
    pub lines: usize,
    pub cell_width: Pixels,
    pub line_height: Pixels,
}

impl TermSize {
    fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.lines as u16,
            num_cols: self.columns as u16,
            cell_width: f32::from(self.cell_width) as u16,
            cell_height: f32::from(self.line_height) as u16,
        }
    }
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.lines
    }

    fn screen_lines(&self) -> usize {
        self.lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

pub enum TerminalEvent {
    /// Cambió algo que se muestra en la pestaña o la barra de estado (título, cwd, comando).
    TitleChanged,
    Bell,
    Exited,
}

/// La línea que zsh está editando, tal como la reporta la integración.
#[derive(Clone, Debug, PartialEq)]
pub struct EditLine {
    pub buffer: String,
    /// Posición del cursor en caracteres.
    pub cursor: usize,
}

pub struct RunningCommand {
    pub text: String,
    pub started: Instant,
}

pub struct FinishedCommand {
    pub exit_code: Option<i32>,
    pub duration: Duration,
}

/// Estado que conocemos gracias a la integración con la shell.
#[derive(Default)]
pub struct ShellState {
    pub integrated: bool,
    pub cwd: Option<PathBuf>,
    pub git: Option<GitStatus>,
    pub line: Option<EditLine>,
    pub running: Option<RunningCommand>,
    pub last: Option<FinishedCommand>,
    /// Sesión SSH interactiva en curso.
    pub remote: Option<RemoteSession>,
    /// Última sesión SSH que se cortó (exit 255), para reconectar.
    pub disconnected: Option<Disconnected>,
}

pub struct RemoteSession {
    /// El comando tal como se ejecutó, para reconectar.
    pub command: String,
    pub target: SshTarget,
    pub identity: HostIdentity,
    pub resolved: Option<ResolvedHost>,
    pub started: Instant,
    /// Carpeta remota, si el servidor la pone en el título (`usuario@host: ~/dir`).
    pub remote_cwd: Option<String>,
}

impl RemoteSession {
    /// `usuario@host` con lo mejor que se sepa.
    pub fn user_at_host(&self) -> String {
        let user = self
            .resolved
            .as_ref()
            .map(|r| r.user.clone())
            .or_else(|| self.target.user.clone());
        match user {
            Some(user) => format!("{user}@{}", self.target.alias),
            None => self.target.alias.clone(),
        }
    }
}

pub struct Disconnected {
    pub command: String,
    pub target: SshTarget,
    pub identity: HostIdentity,
    pub exit_code: Option<i32>,
    pub at: Instant,
}

pub struct Terminal {
    pub term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    size: TermSize,
    title: Option<String>,
    shell: ShellState,
    pending_command: Option<String>,
    resolver: CommandResolver,
    git_refresh: Option<Task<()>>,
    ssh_resolve: Option<Task<()>>,
    /// Comando a ejecutar apenas la shell muestre el prompt (pestañas de "Conectar a…").
    startup_command: Option<String>,
    _events: Task<()>,
}

enum Incoming {
    Terminal(AlacEvent),
    Shell(ShellEvent),
}

impl EventEmitter<TerminalEvent> for Terminal {}

/// PTY y emulador ya creados, antes de tener un `Context` de gpui donde colgar los eventos.
pub struct TerminalBuilder {
    term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    size: TermSize,
    events_rx: UnboundedReceiver<AlacEvent>,
    shell_rx: UnboundedReceiver<ShellEvent>,
}

impl TerminalBuilder {
    /// Abre una sesión en `cwd`; sin carpeta (o si ya no existe), en el home.
    pub fn new(cwd: Option<PathBuf>) -> Result<Self> {
        let size = TermSize {
            columns: 100,
            lines: 30,
            cell_width: px(8.),
            line_height: px(20.),
        };

        let mut env = HashMap::from([
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
            ("TERM_PROGRAM".to_string(), "terminalapp".to_string()),
        ]);
        env.extend(shell::locale_env());
        env.extend(shell::integration_env().unwrap_or_default());
        // La shell se lanza directa como shell de login (`-l`, carga .zprofile y .zshrc). Sin
        // `shell`, alacritty pasa por /usr/bin/login, que imprime "Last login: …" al abrir.
        let program = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/zsh".into());
        let working_directory = cwd
            .filter(|dir| dir.is_dir())
            .or_else(|| std::env::var_os("HOME").map(Into::into));
        let options = tty::Options {
            shell: Some(tty::Shell::new(program, vec!["-l".into()])),
            working_directory,
            drain_on_exit: true,
            env,
            ..Default::default()
        };

        let (events_tx, events_rx) = unbounded();
        let (shell_tx, shell_rx) = unbounded();
        let listener = Listener(events_tx);
        let pty = ShellPty::new(tty::new(&options, size.window_size(), 0)?, shell_tx)?;
        let term = Arc::new(FairMutex::new(Term::new(
            Config::default(),
            &size,
            listener.clone(),
        )));
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
        let sender = event_loop.channel();
        event_loop.spawn();

        Ok(Self {
            term,
            sender,
            size,
            events_rx,
            shell_rx,
        })
    }

    pub fn build(self, cx: &mut Context<Terminal>) -> Terminal {
        let mut incoming = futures::stream::select(
            self.events_rx.map(Incoming::Terminal),
            self.shell_rx.map(Incoming::Shell),
        );
        // Los `Wakeup` llegan en ráfagas: se agrupan y se procesan en un solo update.
        let events = cx.spawn(async move |this, cx| {
            while let Some(event) = incoming.next().await {
                let mut batch = vec![event];
                while let Some(Some(event)) = incoming.next().now_or_never() {
                    batch.push(event);
                }
                let updated = this.update(cx, |this, cx| {
                    for event in batch {
                        match event {
                            Incoming::Terminal(event) => this.handle_event(event, cx),
                            Incoming::Shell(event) => this.handle_shell_event(event, cx),
                        }
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        });

        Terminal {
            term: self.term,
            sender: self.sender,
            size: self.size,
            title: None,
            shell: ShellState::default(),
            pending_command: None,
            resolver: CommandResolver::from_env(),
            git_refresh: None,
            ssh_resolve: None,
            startup_command: None,
            _events: events,
        }
    }
}

impl Terminal {
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn shell(&self) -> &ShellState {
        &self.shell
    }

    /// Lo que falta para completar la línea actual con la entrada más reciente del historial.
    /// Solo con el cursor al final de la línea.
    pub fn suggestion(&self, cx: &App) -> Option<String> {
        let line = self.shell.line.as_ref()?;
        if self.shell.running.is_some() || line.cursor != line.buffer.chars().count() {
            return None;
        }
        shell::history(cx)?
            .suggest(&line.buffer)
            .filter(|rest| !rest.contains('\n'))
            .map(str::to_string)
    }

    /// La línea en edición con sus tokens de resaltado.
    pub fn highlighted_line(&mut self) -> Option<(EditLine, Vec<Token>)> {
        let line = self.shell.line.clone()?;
        if line.buffer.is_empty() {
            return None;
        }
        let cursor = line
            .buffer
            .char_indices()
            .nth(line.cursor)
            .map_or(line.buffer.len(), |(i, _)| i);
        let tokens = highlight::highlight(
            &line.buffer,
            cursor,
            self.shell.cwd.as_deref(),
            &mut self.resolver,
        );
        Some((line, tokens))
    }

    fn handle_shell_event(&mut self, event: ShellEvent, cx: &mut Context<Self>) {
        match event {
            ShellEvent::PromptStart => self.shell.integrated = true,
            ShellEvent::PromptEnd => {}
            ShellEvent::Cwd(cwd) => {
                // Llega en cada prompt: buen momento para refrescar git (cambió de rama, commit…).
                self.refresh_git(cwd.clone(), cx);
                if self.shell.cwd.as_ref() != Some(&cwd) {
                    let save = cwd.clone();
                    cx.background_executor()
                        .spawn(async move { state::record_dir(&save) })
                        .detach();
                    self.shell.cwd = Some(cwd);
                    cx.emit(TerminalEvent::TitleChanged);
                }
            }
            ShellEvent::CommandText(text) => {
                cx.default_global::<History>().push(&text);
                self.pending_command = Some(text);
            }
            ShellEvent::CommandStart => {
                self.shell.line = None;
                let text = self.pending_command.take().unwrap_or_default();
                // Un comando nuevo deja atrás la sesión cortada.
                self.shell.disconnected = None;
                self.start_remote_session(&text, cx);
                self.shell.running = Some(RunningCommand {
                    text,
                    started: Instant::now(),
                });
                cx.emit(TerminalEvent::TitleChanged);
            }
            ShellEvent::CommandFinished(exit_code) => {
                if let Some(running) = self.shell.running.take() {
                    self.shell.last = Some(FinishedCommand {
                        exit_code,
                        duration: running.started.elapsed(),
                    });
                    if let Some(remote) = self.shell.remote.take() {
                        self.ssh_resolve = None;
                        // 255 es el código de ssh cuando la conexión falla o se corta.
                        if exit_code == Some(255) {
                            self.shell.disconnected = Some(Disconnected {
                                command: remote.command,
                                target: remote.target,
                                identity: remote.identity,
                                exit_code,
                                at: Instant::now(),
                            });
                        }
                    }
                    cx.emit(TerminalEvent::TitleChanged);
                }
            }
            ShellEvent::Line { cursor, buffer } => {
                self.shell.line = Some(EditLine { buffer, cursor });
                if let Some(command) = self.startup_command.take() {
                    self.insert_command(&command, true, cx);
                }
                cx.notify();
            }
            ShellEvent::LineFinished => {
                self.shell.line = None;
                cx.notify();
            }
            ShellEvent::HistoryFile(path) => {
                let already = shell::history(cx).is_some_and(|h| h.loaded_from.as_ref() == Some(&path));
                if !already {
                    let read = cx.background_executor().spawn({
                        let path = path.clone();
                        async move { shell::read_history_file(&path) }
                    });
                    cx.spawn(async move |_, cx| {
                        let entries = read.await;
                        cx.update(|cx| {
                            let history = cx.default_global::<History>();
                            // Lo ejecutado en esta sesión queda como lo más reciente.
                            let session = std::mem::replace(&mut history.entries, entries);
                            for command in session {
                                history.push(&command);
                            }
                            history.loaded_from = Some(path);
                        });
                    })
                    .detach();
                }
            }
            ShellEvent::Path(path) => self.resolver.set_path(&path),
            ShellEvent::Names(names) => self.resolver.set_names(&names),
        }
    }

    fn refresh_git(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        let read = cx
            .background_executor()
            .spawn(async move { git::read_status(&cwd) });
        // Reemplazar la tarea cancela una lectura anterior que todavía no terminó.
        self.git_refresh = Some(cx.spawn(async move |this, cx| {
            let status = read.await;
            this.update(cx, |this, cx| {
                if this.shell.git != status {
                    this.shell.git = status;
                    cx.emit(TerminalEvent::TitleChanged);
                }
            })
            .ok();
        }));
    }

    fn start_remote_session(&mut self, command: &str, cx: &mut Context<Self>) {
        let Some(target) = ssh::parse_ssh_command(command).filter(|t| !t.remote_command) else {
            return;
        };
        let rules = hosts::rules(cx);
        let identity = rules.identify(&target.alias, None);
        let resolve = {
            let target = target.clone();
            cx.background_executor().spawn(async move { ssh::resolve(&target) })
        };
        self.ssh_resolve = Some(cx.spawn(async move |this, cx| {
            let resolved = resolve.await;
            this.update(cx, |this, cx| {
                if let Some(remote) = &mut this.shell.remote {
                    // Con el hostname real, una regla puede coincidir aunque el alias no.
                    if let Some(resolved) = &resolved {
                        remote.identity = rules.identify(&remote.target.alias, Some(&resolved.hostname));
                    }
                    remote.resolved = resolved;
                    cx.emit(TerminalEvent::TitleChanged);
                }
            })
            .ok();
        }));
        self.shell.remote = Some(RemoteSession {
            command: command.to_string(),
            target,
            identity,
            resolved: None,
            started: Instant::now(),
            remote_cwd: None,
        });
    }

    /// Qué se puede escribir en la posición del cursor de la línea en edición.
    pub fn complete(&mut self, cx: &App) -> Option<completions::Completion> {
        let line = self.shell.line.clone().filter(|_| self.at_prompt())?;
        let cursor = line
            .buffer
            .char_indices()
            .nth(line.cursor)
            .map_or(line.buffer.len(), |(i, _)| i);
        let git = self.shell.git.clone();
        let branches = git.as_ref().map(|g| g.branches.clone()).unwrap_or_default();
        let history = shell::history(cx).map(|h| h.entries.clone()).unwrap_or_default();
        let hosts: Vec<(String, String)> = ssh::config_hosts()
            .into_iter()
            .map(|h| {
                let describe = h.describe();
                (h.alias, describe)
            })
            .collect();
        let default_specs = completions::Specs::default();
        let specs = cx.try_global::<completions::Specs>().unwrap_or(&default_specs);
        let cwd = self.shell.cwd.clone();
        let resolver = std::cell::RefCell::new(std::mem::take(&mut self.resolver));
        let commands = |prefix: &str| resolver.borrow_mut().names_with_prefix(prefix);
        let scripts = || cwd.as_deref().map(completions::package_scripts).unwrap_or_default();
        let result = completions::complete(&completions::Context {
            buffer: &line.buffer,
            cursor,
            cwd: cwd.as_deref(),
            specs,
            branches: &branches,
            current_branch: git.as_ref().and_then(|g| g.branch.as_deref()),
            history: &history,
            hosts: &hosts,
            commands: &commands,
            list_dir: &completions::list_dir,
            scripts: &scripts,
        });
        self.resolver = resolver.into_inner();
        result
    }

    /// Vuelve a ejecutar el `ssh` que se cortó.
    pub fn reconnect(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(command) = self.shell.disconnected.as_ref().map(|d| d.command.clone()) else {
            return false;
        };
        self.insert_command(&command, true, cx)
    }

    /// Ejecuta `command` en cuanto la shell esté lista (o ya, si lo está).
    pub fn run_when_ready(&mut self, command: String, cx: &mut Context<Self>) {
        if self.at_prompt() {
            self.insert_command(&command, true, cx);
        } else {
            self.startup_command = Some(command);
        }
    }

    /// La shell está esperando input: se le puede escribir un comando sin pisar un programa.
    pub fn at_prompt(&self) -> bool {
        self.shell.line.is_some() && self.shell.running.is_none()
    }

    /// Escribe y ejecuta `git switch` en la shell, para que el cambio (y cualquier error)
    /// quede visible en la terminal. Solo cuando la shell está en el prompt.
    pub fn switch_branch(&mut self, branch: &str, cx: &mut Context<Self>) -> bool {
        if !self.at_prompt() {
            return false;
        }
        // Ctrl-E + Ctrl-U: va al final y borra lo que hubiera escrito en la línea.
        let command = format!("\x05\x15{}\r", git::switch_command(branch));
        self.input(command.into_bytes(), cx);
        true
    }

    /// Escribe `command` en el prompt, reemplazando lo que hubiera; con `run`, además lo ejecuta.
    pub fn insert_command(&mut self, command: &str, run: bool, cx: &mut Context<Self>) -> bool {
        if !self.at_prompt() {
            return false;
        }
        let mut bytes = format!("\x05\x15{command}");
        if run {
            bytes.push('\r');
        }
        self.input(bytes.into_bytes(), cx);
        true
    }

    /// Borra el scrollback y, si la shell está en el prompt, le pide redibujar la pantalla.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.term.lock().grid_mut().clear_history();
        if self.at_prompt() {
            self.input(b"\x0c".to_vec(), cx);
        }
        cx.notify();
    }

    pub fn size(&self) -> TermSize {
        self.size
    }

    pub fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    fn handle_event(&mut self, event: AlacEvent, cx: &mut Context<Self>) {
        match event {
            AlacEvent::Wakeup | AlacEvent::MouseCursorDirty | AlacEvent::CursorBlinkingChange => {
                cx.notify()
            }
            AlacEvent::Title(title) => {
                if let Some(remote) = &mut self.shell.remote
                    && let Some(cwd) = remote_cwd_from_title(&title)
                {
                    remote.remote_cwd = Some(cwd);
                }
                self.title = Some(title);
                cx.emit(TerminalEvent::TitleChanged);
            }
            AlacEvent::ResetTitle => {
                self.title = None;
                cx.emit(TerminalEvent::TitleChanged);
            }
            AlacEvent::PtyWrite(text) => self.write(text.into_bytes()),
            AlacEvent::ColorRequest(index, format) => {
                self.write(format(palette::rgb_for_index(index, &cx.tokens().terminal)).into_bytes())
            }
            AlacEvent::TextAreaSizeRequest(format) => {
                self.write(format(self.size.window_size()).into_bytes())
            }
            AlacEvent::ClipboardStore(_, text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            AlacEvent::ClipboardLoad(_, format) => {
                let text = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())
                    .unwrap_or_default();
                self.write(format(&text).into_bytes());
            }
            AlacEvent::Bell => cx.emit(TerminalEvent::Bell),
            AlacEvent::Exit | AlacEvent::ChildExit(_) => cx.emit(TerminalEvent::Exited),
        }
    }

    fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        // Una escritura vacía traba la cola de escritura del event loop de alacritty: todo lo
        // que se mande después queda sin enviarse al PTY.
        if !bytes.is_empty() {
            self.sender.send(Msg::Input(bytes)).ok();
        }
    }

    /// Entrada del usuario: vuelve al final del scrollback antes de escribir.
    pub fn input(&mut self, bytes: impl Into<Cow<'static, [u8]>>, cx: &mut Context<Self>) {
        let mut term = self.term.lock();
        // Escribir descarta la selección, como en cualquier terminal.
        if term.selection.take().is_some() {
            cx.notify();
        }
        if term.grid().display_offset() != 0 {
            term.scroll_display(Scroll::Bottom);
            cx.notify();
        }
        drop(term);
        self.write(bytes);
    }

    pub fn paste(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        let bytes = if self.mode().contains(TermMode::BRACKETED_PASTE) {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
        } else {
            text
        };
        self.input(bytes.into_bytes(), cx);
    }

    pub fn scroll(&mut self, lines: i32, cx: &mut Context<Self>) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
        cx.notify();
    }

    pub fn resize(&mut self, size: TermSize) {
        if size == self.size {
            return;
        }
        self.size = size;
        self.term.lock().resize(size);
        self.sender.send(Msg::Resize(size.window_size())).ok();
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.sender.send(Msg::Shutdown).ok();
    }
}

/// Carpeta del título que ponen muchos servidores: `usuario@host: ~/dir` o `usuario@host:~/dir`.
fn remote_cwd_from_title(title: &str) -> Option<String> {
    let (who, rest) = title.split_once(':')?;
    if !who.contains('@') || who.contains(' ') {
        return None;
    }
    let cwd = rest.trim();
    (cwd.starts_with('/') || cwd.starts_with('~')).then(|| cwd.to_string())
}

#[cfg(test)]
mod tests {
    use super::remote_cwd_from_title;

    #[test]
    fn reads_remote_cwd_from_titles() {
        assert_eq!(remote_cwd_from_title("angel@prod-db-01: ~/app").as_deref(), Some("~/app"));
        assert_eq!(remote_cwd_from_title("root@box:/var/log").as_deref(), Some("/var/log"));
        assert_eq!(remote_cwd_from_title("vim notes.md"), None);
        assert_eq!(remote_cwd_from_title("Make: building"), None);
    }
}
