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
    git::{self, GitStatus},
    highlight::{self, CommandResolver, Token},
    palette,
    shell::{self, History, ShellEvent, ShellPty},
    state,
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
                        .spawn(async move { state::save_last_cwd(&save) })
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
                self.shell.running = Some(RunningCommand {
                    text: self.pending_command.take().unwrap_or_default(),
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
                    cx.emit(TerminalEvent::TitleChanged);
                }
            }
            ShellEvent::Line { cursor, buffer } => {
                self.shell.line = Some(EditLine { buffer, cursor });
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
                self.title = Some(title);
                cx.emit(TerminalEvent::TitleChanged);
            }
            AlacEvent::ResetTitle => {
                self.title = None;
                cx.emit(TerminalEvent::TitleChanged);
            }
            AlacEvent::PtyWrite(text) => self.write(text.into_bytes()),
            AlacEvent::ColorRequest(index, format) => {
                self.write(format(palette::rgb_for_index(index)).into_bytes())
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
        self.sender.send(Msg::Input(bytes.into())).ok();
    }

    /// Entrada del usuario: vuelve al final del scrollback antes de escribir.
    pub fn input(&mut self, bytes: impl Into<Cow<'static, [u8]>>, cx: &mut Context<Self>) {
        let mut term = self.term.lock();
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
