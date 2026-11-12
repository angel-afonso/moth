//! Session model: PTY + alacritty emulator, with events forwarded to gpui.

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt as _;
use std::{
    borrow::Cow,
    collections::HashMap,
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::Arc,
    time::{Duration, Instant},
};

use alacritty_terminal::{
    event::{Event as AlacEvent, EventListener, WindowSize},
    event_loop::{EventLoop, EventLoopSender, Msg},
    grid::{Dimensions, Scroll},
    index::Line,
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
    blocks, completions,
    fixes::{self, Failure, Fix},
    git::{self, GitStatus},
    highlight::{self, CommandResolver, Token},
    hosts::{self, HostIdentity},
    palette,
    shell::{self, History, ShellEvent, ShellPty},
    ssh::{self, ResolvedHost, SshTarget},
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

/// Grid size in cells, plus the cell size reported to the PTY.
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
    /// Something shown in the tab or status bar changed (title, cwd, command).
    TitleChanged,
    CommandFinished {
        command: String,
        exit_code: Option<i32>,
        duration: Duration,
    },
    Bell,
    /// The shell exited; if it was an error (see `Terminal::exit_failure`), the tab stays open.
    Exited,
}

/// The line zsh is editing, as reported by the integration.
#[derive(Clone, Debug, PartialEq)]
pub struct EditLine {
    pub buffer: String,
    /// Cursor position in characters.
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

/// State we know about thanks to the shell integration.
#[derive(Default)]
pub struct ShellState {
    pub integrated: bool,
    pub cwd: Option<PathBuf>,
    pub git: Option<GitStatus>,
    pub line: Option<EditLine>,
    pub running: Option<RunningCommand>,
    pub last: Option<FinishedCommand>,
    /// Interactive SSH session in progress.
    pub remote: Option<RemoteSession>,
    /// Last SSH session that dropped (exit 255), for reconnecting.
    pub disconnected: Option<Disconnected>,
    /// The shell's PATH, to find the editor even when the app is launched from Finder.
    pub path_env: Option<String>,
    /// Names of the shell's exported variables, for completing `$NAME`.
    pub variables: Vec<String>,
    /// The last command, if it failed, with its output and a fix found on the machine.
    /// Cleared when the next command starts.
    pub failure: Option<LastFailure>,
    /// `cwd` is gone, most likely because its volume was unmounted (a disk unplugged, a share
    /// dropped). When it's back, the shell goes back into it (see `Terminal::cwd_checked`).
    pub cwd_missing: Option<MissingCwd>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MissingCwd {
    /// The volume it was on, when the whole volume is gone (`/Volumes/<name>` on macOS,
    /// `/media/<user>/<name>`, `/run/media/<user>/<name>` or `/mnt/<name>` on Linux).
    pub volume: Option<String>,
}

impl MissingCwd {
    /// Whether `cwd` is missing, and why. Touches the disk: off the UI thread.
    fn check(cwd: &Path) -> Option<Self> {
        if cwd.is_dir() {
            return None;
        }
        let volume = mount_root(cwd)
            .filter(|root| !root.exists())
            .and_then(|root| Some(root.file_name()?.to_string_lossy().into_owned()));
        Some(Self { volume })
    }
}

/// Where the volume holding `path` is mounted, for the usual places removable disks and
/// shares go.
fn mount_root(path: &Path) -> Option<PathBuf> {
    let parts: Vec<_> = path.components().collect();
    let names: Vec<_> = parts
        .iter()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect();
    let depth = match names.get(1..3)? {
        [first, _] if first == "Volumes" || first == "mnt" => 3,
        [first, _] if first == "media" => 4,
        [first, second] if first == "run" && second == "media" => 5,
        _ => return None,
    };
    (parts.len() >= depth).then(|| parts[..depth].iter().collect())
}

pub struct LastFailure {
    pub failure: Failure,
    pub fix: Option<Fix>,
}

pub struct RemoteSession {
    /// The command exactly as it was run, for reconnecting.
    pub command: String,
    pub target: SshTarget,
    pub identity: HostIdentity,
    pub resolved: Option<ResolvedHost>,
    pub started: Instant,
    /// Remote folder, if the server puts it in the title (`user@host: ~/dir`).
    pub remote_cwd: Option<String>,
}

impl RemoteSession {
    /// `user@host`, as best we know it.
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
    /// A command that just failed, read from the grid once the next prompt is drawn.
    pending_failure: Option<Failure>,
    capture_failure: Option<Task<()>>,
    resolver: CommandResolver,
    /// The last highlighted line and the folder it was highlighted in: highlighting checks
    /// whether arguments exist on disk, and the line is drawn every frame.
    highlight_cache: Option<(EditLine, Option<PathBuf>, Vec<Token>)>,
    git_refresh: Option<Task<()>>,
    ssh_resolve: Option<Task<()>>,
    /// Command to run as soon as the shell shows its prompt ("Connect to…" tabs).
    startup_command: Option<String>,
    started: Instant,
    /// The shell exited, with its status if known.
    exited: Option<Option<ExitStatus>>,
    /// The shell's process, when it runs the zsh integration and so can be woken with
    /// SIGUSR1 to go back into its folder.
    wakeable_pid: Option<u32>,
    _events: Task<()>,
    _watch_cwd: Task<()>,
}

/// How often each tab checks that its folder still exists.
const CWD_CHECK_INTERVAL: Duration = Duration::from_secs(2);

enum Incoming {
    Terminal(AlacEvent),
    Shell(ShellEvent),
}

impl EventEmitter<TerminalEvent> for Terminal {}

/// PTY and emulator already created, before there's a gpui `Context` to attach events to.
pub struct TerminalBuilder {
    term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    size: TermSize,
    events_rx: UnboundedReceiver<AlacEvent>,
    shell_rx: UnboundedReceiver<ShellEvent>,
    wakeable_pid: Option<u32>,
}

impl TerminalBuilder {
    /// Opens a session in `cwd`; with no folder (or if it no longer exists), in the home folder.
    pub fn new(cwd: Option<PathBuf>, config: &crate::config::Config) -> Result<Self> {
        let size = TermSize {
            columns: 100,
            lines: 30,
            cell_width: px(8.),
            line_height: px(20.),
        };

        let mut env = HashMap::from([
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
            ("TERM_PROGRAM".to_string(), "moth".to_string()),
        ]);
        env.extend(shell::locale_env());
        let working_directory = cwd.filter(|dir| dir.is_dir()).or_else(state::home_dir);
        let spawn = |configured: Option<&str>| {
            let (program, args) = shell::shell_command(configured);
            let mut env = env.clone();
            let integration = shell::integration_env(&program).unwrap_or_default();
            let token = integration.get(shell::TOKEN_VAR).cloned();
            env.extend(integration);
            let options = tty::Options {
                shell: Some(tty::Shell::new(program, args)),
                working_directory: working_directory.clone(),
                drain_on_exit: true,
                env,
                ..Default::default()
            };
            tty::new(&options, size.window_size(), 0).map(|pty| (pty, token))
        };
        // A `shell` from the settings that can't start (not on the PATH an app launched from
        // the Finder gets, say) falls back to the default one, so there's still a tab from
        // which to fix the settings.
        let (pty, token) = match spawn(config.shell.as_deref()) {
            Err(err) if config.shell.is_some() => {
                crate::log::error(format!(
                    "couldn't start the shell in the settings ({err}); using the default one"
                ));
                spawn(None)?
            }
            result => result?,
        };

        // Only the integration's shell has the SIGUSR1 handler.
        #[cfg(unix)]
        let wakeable_pid = token.as_ref().map(|_| pty.child().id());
        #[cfg(not(unix))]
        let wakeable_pid = None;

        let (events_tx, events_rx) = unbounded();
        let (shell_tx, shell_rx) = unbounded();
        let listener = Listener(events_tx);
        let pty = ShellPty::new(pty, shell_tx, token);
        let term = Arc::new(FairMutex::new(Term::new(
            Config {
                scrolling_history: config
                    .scrollback
                    .map_or(Config::default().scrolling_history, |lines| {
                        lines.min(crate::config::MAX_SCROLLBACK)
                    }),
                ..Config::default()
            },
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
            wakeable_pid,
        })
    }

    pub fn build(self, cx: &mut Context<Terminal>) -> Terminal {
        let mut incoming = futures::stream::select(
            self.events_rx.map(Incoming::Terminal),
            self.shell_rx.map(Incoming::Shell),
        );
        // `Wakeup`s arrive in bursts: batch them and process them in a single update.
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

        // The check runs off the UI thread: on a network share that dropped, it can block.
        let watch_cwd = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CWD_CHECK_INTERVAL).await;
                let Ok(cwd) = this.update(cx, |this, _| this.shell.cwd.clone()) else {
                    break;
                };
                let Some(cwd) = cwd else {
                    continue;
                };
                let check = cwd.clone();
                let missing = cx
                    .background_executor()
                    .spawn(async move { MissingCwd::check(&check) })
                    .await;
                if this
                    .update(cx, |this, cx| this.cwd_checked(&cwd, missing, cx))
                    .is_err()
                {
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
            pending_failure: None,
            capture_failure: None,
            resolver: CommandResolver::from_env(),
            highlight_cache: None,
            git_refresh: None,
            ssh_resolve: None,
            startup_command: None,
            started: Instant::now(),
            exited: None,
            wakeable_pid: self.wakeable_pid,
            _events: events,
            _watch_cwd: watch_cwd,
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

    /// Whether `cwd` (still the shell's folder) exists, from the periodic check. Going missing
    /// is shown in the view; coming back wakes the shell at the prompt, which goes back into
    /// the folder. A shell running a command does it at its next prompt.
    fn cwd_checked(&mut self, cwd: &Path, missing: Option<MissingCwd>, cx: &mut Context<Self>) {
        if missing == self.shell.cwd_missing
            || self.exited.is_some()
            || self.shell.cwd.as_deref() != Some(cwd)
        {
            return;
        }
        let came_back = missing.is_none();
        self.shell.cwd_missing = missing;
        if came_back && self.at_prompt() {
            self.wake_shell();
        }
        cx.notify();
    }

    /// Whether the shell goes back into its folder by itself when the folder comes back:
    /// only with the zsh integration.
    pub fn recovers_cwd(&self) -> bool {
        self.wakeable_pid.is_some()
    }

    /// Sends the shell SIGUSR1, which the zsh integration takes as "check your folder".
    fn wake_shell(&self) {
        #[cfg(unix)]
        if let Some(pid) = self
            .wakeable_pid
            .and_then(|pid| libc::pid_t::try_from(pid).ok())
        {
            // SAFETY: `kill` only sends a signal; the pid is our own child, which hasn't
            // exited (`exited` is checked before), so it can't have been reused.
            if unsafe { libc::kill(pid, libc::SIGUSR1) } != 0 {
                crate::log::error(format!(
                    "couldn't wake the shell: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
    }

    /// What's missing to complete the current line: the fix for the command that just failed,
    /// while the line is still a start of it, or else the best history entry (see
    /// [`History::suggest`]). Only when the cursor is at the end of the line.
    pub fn suggestion(&self, cx: &App) -> Option<String> {
        let line = self.shell.line.as_ref()?;
        if self.shell.running.is_some() || line.cursor != line.buffer.chars().count() {
            return None;
        }
        if let Some(rest) = self.fix_suggestion() {
            return Some(rest);
        }
        shell::history(cx)?
            .suggest(&line.buffer, self.shell.cwd.as_deref())
            .filter(|rest| !rest.contains('\n'))
            .map(str::to_string)
    }

    /// The rest of the fix for the last command, while what's typed is the start of it.
    pub fn fix_suggestion(&self) -> Option<String> {
        let line = self.shell.line.as_ref()?;
        let fix = self.shell.failure.as_ref()?.fix.as_ref()?;
        fix.command
            .strip_prefix(line.buffer.as_str())
            .filter(|rest| !rest.is_empty())
            .map(str::to_string)
    }

    /// The line being edited, with its highlighting tokens.
    pub fn highlighted_line(&mut self) -> Option<(EditLine, Vec<Token>)> {
        let line = self.shell.line.clone()?;
        if line.buffer.is_empty() {
            return None;
        }
        if let Some((cached, cwd, tokens)) = &self.highlight_cache
            && *cached == line
            && *cwd == self.shell.cwd
        {
            return Some((line, tokens.clone()));
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
        self.highlight_cache = Some((line.clone(), self.shell.cwd.clone(), tokens.clone()));
        Some((line, tokens))
    }

    fn handle_shell_event(&mut self, event: ShellEvent, cx: &mut Context<Self>) {
        match event {
            ShellEvent::PromptStart => {
                self.shell.integrated = true;
                // Files may have come and gone since the last prompt.
                self.highlight_cache = None;
            }
            ShellEvent::PromptEnd(_) => {}
            ShellEvent::Cwd(cwd) => {
                // Arrives on every prompt: a good time to refresh git (branch switch, commit…).
                self.refresh_git(cwd.clone(), cx);
                if self.shell.cwd.as_ref() != Some(&cwd) {
                    let save = cwd.clone();
                    cx.background_executor()
                        .spawn(async move { state::record_dir(&save) })
                        .detach();
                    self.shell.cwd = Some(cwd);
                    self.shell.cwd_missing = None;
                    cx.emit(TerminalEvent::TitleChanged);
                }
            }
            ShellEvent::CommandText(text) => {
                cx.default_global::<History>().push(&text);
                self.pending_command = Some(text);
            }
            ShellEvent::CommandStart => {
                self.shell.line = None;
                self.shell.failure = None;
                self.pending_failure = None;
                self.capture_failure = None;
                let text = self.pending_command.take().unwrap_or_default();
                // A new command leaves the dropped session behind.
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
                    // The folder is still the one the command was typed in: the new one
                    // arrives after this, with the prompt.
                    if let Some(run) =
                        shell::Run::new(&running.text, self.shell.cwd.clone(), exit_code)
                    {
                        cx.default_global::<History>().record(&run);
                        cx.background_executor()
                            .spawn(async move { shell::save_run(&run) })
                            .detach();
                    }
                    cx.emit(TerminalEvent::CommandFinished {
                        command: running.text.clone(),
                        exit_code,
                        duration: running.started.elapsed(),
                    });
                    self.shell.last = Some(FinishedCommand {
                        exit_code,
                        duration: running.started.elapsed(),
                    });
                    if exit_code.is_some_and(|code| code != 0) && self.shell.remote.is_none() {
                        self.pending_failure = Some(Failure {
                            command: running.text.clone(),
                            exit_code,
                            output: String::new(),
                        });
                    }
                    if let Some(remote) = self.shell.remote.take() {
                        self.ssh_resolve = None;
                        // 255 is ssh's exit code when the connection fails or drops.
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
                if let Some(failure) = self.pending_failure.take() {
                    self.capture_failure(failure, cx);
                }
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
                let already =
                    shell::history(cx).is_some_and(|h| h.loaded_from.as_ref() == Some(&path));
                if !already {
                    let read = cx.background_executor().spawn({
                        let path = path.clone();
                        async move { shell::read_history_file(&path) }
                    });
                    cx.spawn(async move |_, cx| {
                        let entries = read.await;
                        cx.update(|cx| {
                            let history = cx.default_global::<History>();
                            // Commands run in this session stay as the most recent.
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
            ShellEvent::Path(path) => {
                self.resolver.set_path(&path);
                self.shell.path_env = Some(path);
                self.highlight_cache = None;
            }
            ShellEvent::Variables(names) => {
                self.shell.variables = names.split_whitespace().map(str::to_string).collect();
            }
            ShellEvent::Names(names) => {
                self.resolver.set_names(&names);
                self.highlight_cache = None;
            }
        }
    }

    /// Reads the failed command's output from the grid and looks for a fix. The PTY scanner
    /// reports the new prompt before alacritty has drawn it, so this waits a moment.
    fn capture_failure(&mut self, mut failure: Failure, cx: &mut Context<Self>) {
        self.capture_failure = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
            this.update(cx, |this, cx| {
                failure.output = this.last_output();
                let fix = this.find_fix(&failure);
                this.shell.failure = Some(LastFailure { failure, fix });
                cx.notify();
            })
            .ok();
        }));
    }

    /// The output of the last command that ran, from the last few hundred rows.
    fn last_output(&self) -> String {
        let term = self.term.lock();
        let cursor = term.grid().cursor.point.line;
        let top = Line((cursor.0 - 400).max(term.topmost_line().0));
        blocks::blocks_from(&*term, top)
            .into_iter()
            .rfind(|block| block.input < cursor)
            .map(|block| blocks::output_text(&*term, &block))
            .unwrap_or_default()
    }

    fn find_fix(&mut self, failure: &Failure) -> Option<Fix> {
        if !failure.worth_fixing() {
            return None;
        }
        let cwd = self.shell.cwd.clone();
        let branches = self
            .shell
            .git
            .as_ref()
            .map(|git| git.branches.clone())
            .unwrap_or_default();
        let resolver = std::cell::RefCell::new(std::mem::take(&mut self.resolver));
        let closest_command = |word: &str| {
            let mut resolver = resolver.borrow_mut();
            // A command that exists failed for another reason (a missing subcommand's binary).
            if resolver.is_command(word, cwd.as_deref()) {
                return None;
            }
            resolver.closest(word)
        };
        let list_dirs = |dir: &std::path::Path| {
            std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect()
        };
        let fix = fixes::suggest(
            failure,
            &fixes::Context {
                cwd: cwd.as_deref(),
                closest_command: &closest_command,
                branches: &branches,
                list_dirs: &list_dirs,
            },
        );
        self.resolver = resolver.into_inner();
        fix
    }

    fn refresh_git(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        let read = cx
            .background_executor()
            .spawn(async move { git::read_status(&cwd) });
        // Replacing the task cancels a previous read that hasn't finished yet.
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
            cx.background_executor()
                .spawn(async move { ssh::resolve(&target) })
        };
        self.ssh_resolve = Some(cx.spawn(async move |this, cx| {
            let resolved = resolve.await;
            this.update(cx, |this, cx| {
                if let Some(remote) = &mut this.shell.remote {
                    // With the real hostname, a rule may match even if the alias didn't.
                    if let Some(resolved) = &resolved {
                        remote.identity =
                            rules.identify(&remote.target.alias, Some(&resolved.hostname));
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

    /// What can be typed at the cursor position of the line being edited.
    pub fn complete(&mut self, cx: &App) -> Option<completions::Completion> {
        let line = self.shell.line.clone().filter(|_| self.at_prompt())?;
        let cursor = line
            .buffer
            .char_indices()
            .nth(line.cursor)
            .map_or(line.buffer.len(), |(i, _)| i);
        // This runs on every keystroke while the popup is open: borrow rather than copy, and
        // read files only for what can be typed at the cursor.
        let git = self.shell.git.as_ref();
        let branches = git.map(|g| g.branches.as_slice()).unwrap_or_default();
        let hosts = || {
            ssh::config_hosts()
                .into_iter()
                .map(|h| {
                    let describe = h.describe();
                    (h.alias, describe)
                })
                .collect()
        };
        let default_specs = completions::Specs::default();
        let specs = cx
            .try_global::<completions::Specs>()
            .unwrap_or(&default_specs);
        let cwd = self.shell.cwd.clone();
        let resolver = std::cell::RefCell::new(std::mem::take(&mut self.resolver));
        let commands = |prefix: &str| resolver.borrow_mut().names_with_prefix(prefix);
        let scripts = || {
            cwd.as_deref()
                .map(completions::package_scripts)
                .unwrap_or_default()
        };
        let targets = || {
            cwd.as_deref()
                .map(completions::make_targets)
                .unwrap_or_default()
        };
        let changed_files = || cwd.as_deref().map(git::changed_files).unwrap_or_default();
        let result = completions::complete(&completions::Context {
            buffer: &line.buffer,
            cursor,
            cwd: cwd.as_deref(),
            specs,
            branches,
            current_branch: git.and_then(|g| g.branch.as_deref()),
            variables: &self.shell.variables,
            hosts: &hosts,
            commands: &commands,
            list_dir: &completions::list_dir,
            scripts: &scripts,
            targets: &targets,
            changed_files: &changed_files,
            recent_dirs: &state::recent_dirs,
        });
        self.resolver = resolver.into_inner();
        result
    }

    /// Reruns the `ssh` command that dropped.
    pub fn reconnect(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(command) = self.shell.disconnected.as_ref().map(|d| d.command.clone()) else {
            return false;
        };
        self.insert_command(&command, true, cx)
    }

    /// Runs `command` as soon as the shell is ready (or right away, if it already is).
    pub fn run_when_ready(&mut self, command: String, cx: &mut Context<Self>) {
        if self.at_prompt() {
            self.insert_command(&command, true, cx);
        } else {
            self.startup_command = Some(command);
        }
    }

    /// The shell is waiting for input: a command can be typed without clobbering a running program.
    pub fn at_prompt(&self) -> bool {
        self.shell.line.is_some() && self.shell.running.is_none()
    }

    /// Types and runs `git switch` in the shell, so the switch (and any error) shows up
    /// in the terminal. Only when the shell is at the prompt.
    pub fn switch_branch(&mut self, branch: &str, cx: &mut Context<Self>) -> bool {
        if !self.at_prompt() {
            return false;
        }
        // Ctrl-E + Ctrl-U: jump to the end and clear whatever was typed on the line.
        let command = format!("\x05\x15{}\r", git::switch_command(branch));
        self.input(command.into_bytes(), cx);
        true
    }

    /// Types `command` at the prompt, replacing whatever was there; with `run`, also runs it.
    pub fn insert_command(&mut self, command: &str, run: bool, cx: &mut Context<Self>) -> bool {
        if !self.at_prompt() {
            return false;
        }
        let bracketed = self.mode().contains(TermMode::BRACKETED_PASTE);
        let Some(text) = prompt_text(command, bracketed) else {
            return false;
        };
        let mut bytes = format!("\x05\x15{text}");
        if run {
            bytes.push('\r');
        }
        self.input(bytes.into_bytes(), cx);
        true
    }

    /// Clears the scrollback and, if the shell is at the prompt, asks it to redraw the screen.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.term.lock().grid_mut().clear_history();
        if self.at_prompt() {
            self.input(b"\x0c".to_vec(), cx);
        }
        cx.notify();
    }

    fn exited(&mut self, status: Option<ExitStatus>, cx: &mut Context<Self>) {
        if self.exited.is_none() {
            self.exited = Some(status);
            cx.emit(TerminalEvent::Exited);
            cx.notify();
        }
    }

    pub fn has_exited(&self) -> bool {
        self.exited.is_some()
    }

    /// Why the shell exited, if it was an error worth showing instead of closing the tab:
    /// it was killed by a signal, or it failed right after starting (a broken `.zshrc`, a
    /// shell that doesn't exist). A later `exit` or ⌃D may exit with the last command's
    /// code, so that doesn't count.
    pub fn exit_failure(&self) -> Option<String> {
        let status = (*self.exited.as_ref()?)?;
        exit_failure(status, self.started.elapsed())
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
            AlacEvent::ColorRequest(index, format) => self
                .write(format(palette::rgb_for_index(index, &cx.tokens().terminal)).into_bytes()),
            AlacEvent::TextAreaSizeRequest(format) => {
                self.write(format(self.size.window_size()).into_bytes())
            }
            AlacEvent::ClipboardStore(_, text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text))
            }
            // Answering OSC 52 would hand the clipboard to any program that asks: an SSH
            // server or a `cat` of a crafted file. Like alacritty, only copying is allowed.
            AlacEvent::ClipboardLoad(..) => {}
            AlacEvent::Bell => cx.emit(TerminalEvent::Bell),
            // `ChildExit` carries the status and arrives before `Exit`; `Exit` only if it wasn't known.
            AlacEvent::ChildExit(status) => self.exited(Some(status), cx),
            AlacEvent::Exit => self.exited(None, cx),
        }
    }

    fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        // An empty write jams the write queue of alacritty's event loop: anything sent
        // afterwards never reaches the PTY.
        if !bytes.is_empty() {
            self.sender.send(Msg::Input(bytes)).ok();
        }
    }

    /// User input: scrolls back to the bottom before writing.
    pub fn input(&mut self, bytes: impl Into<Cow<'static, [u8]>>, cx: &mut Context<Self>) {
        let mut term = self.term.lock();
        // Typing clears the selection, as in any terminal.
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

fn exit_failure(status: ExitStatus, lived: Duration) -> Option<String> {
    #[cfg(unix)]
    if let Some(signal) = status.signal() {
        return Some(format!("The shell was killed by signal {signal}"));
    }
    match status.code() {
        Some(code) if code != 0 && lived < Duration::from_secs(5) => {
            Some(format!("The shell exited with code {code} on startup"))
        }
        _ => None,
    }
}

/// Folder from the title many servers set: `user@host: ~/dir` or `user@host:~/dir`.
fn remote_cwd_from_title(title: &str) -> Option<String> {
    let (who, rest) = title.split_once(':')?;
    if !who.contains('@') || who.contains(' ') {
        return None;
    }
    let cwd = rest.trim();
    (cwd.starts_with('/') || cwd.starts_with('~')).then(|| cwd.to_string())
}

/// `command` as it can be typed at the prompt. It may come from the history, the agent or the
/// shell's own report, so control characters are dropped (an escape sequence, or a `\r` that
/// would run part of it) and tabs become spaces. Line breaks only survive inside a bracketed
/// paste, where the shell takes them as part of the line; without one there's nothing safe to
/// type (`None`).
fn prompt_text(command: &str, bracketed: bool) -> Option<String> {
    let command = command.replace("\r\n", "\n").replace('\r', "\n");
    let clean: String = command
        .trim_matches('\n')
        .chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|&c| c == '\n' || !c.is_control())
        .collect();
    if !clean.contains('\n') {
        return Some(clean);
    }
    bracketed.then(|| format!("\x1b[200~{}\x1b[201~", clean.replace('\n', "\r")))
}

#[cfg(test)]
mod tests {
    use super::remote_cwd_from_title;

    #[test]
    fn finds_the_volume_of_a_folder() {
        use super::mount_root;
        use std::path::{Path, PathBuf};
        let root = |path: &str| mount_root(Path::new(path));
        assert_eq!(
            root("/Volumes/EXTERNAL_DISK/work/app"),
            Some(PathBuf::from("/Volumes/EXTERNAL_DISK"))
        );
        assert_eq!(
            root("/media/dev/USB/photos"),
            Some(PathBuf::from("/media/dev/USB"))
        );
        assert_eq!(
            root("/run/media/dev/USB"),
            Some(PathBuf::from("/run/media/dev/USB"))
        );
        assert_eq!(root("/mnt/share/x"), Some(PathBuf::from("/mnt/share")));
        assert_eq!(root("/Volumes"), None);
        assert_eq!(root("/Users/dev/project"), None);
    }

    #[test]
    fn tells_a_missing_volume_from_a_missing_folder() {
        use super::MissingCwd;
        use std::path::Path;
        assert_eq!(MissingCwd::check(&std::env::temp_dir()), None);
        let gone = MissingCwd::check(Path::new("/Volumes/MothNoSuchVolume/work")).unwrap();
        assert_eq!(gone.volume.as_deref(), Some("MothNoSuchVolume"));
        let folder = std::env::temp_dir().join("moth-no-such-folder/x");
        assert_eq!(MissingCwd::check(&folder).unwrap().volume, None);
    }

    #[test]
    fn types_only_what_cannot_run_by_itself() {
        use super::prompt_text;
        assert_eq!(prompt_text("ls -la", false).as_deref(), Some("ls -la"));
        assert_eq!(
            prompt_text("echo\x1b]0;x\x07\tdone\u{9b}", false).as_deref(),
            Some("echo]0;x done")
        );
        assert_eq!(prompt_text("cd build\rmake install", false), None);
        assert_eq!(
            prompt_text("cd build\nmake install\n", true).as_deref(),
            Some("\x1b[200~cd build\rmake install\x1b[201~")
        );
    }

    #[cfg(unix)]
    #[test]
    fn keeps_tabs_only_for_abnormal_exits() {
        use std::{os::unix::process::ExitStatusExt as _, process::ExitStatus, time::Duration};

        use super::exit_failure;

        let quick = Duration::from_secs(1);
        let later = Duration::from_secs(60);
        // `from_raw` takes the waitpid status: exit code in the high byte, signal in the low one.
        let code = |code: i32| ExitStatus::from_raw(code << 8);
        assert_eq!(exit_failure(code(0), quick), None);
        assert_eq!(exit_failure(code(1), later), None);
        assert!(exit_failure(code(127), quick).unwrap().contains("127"));
        assert!(
            exit_failure(ExitStatus::from_raw(9), later)
                .unwrap()
                .contains("signal 9")
        );
    }

    #[test]
    fn reads_remote_cwd_from_titles() {
        assert_eq!(
            remote_cwd_from_title("angel@prod-db-01: ~/app").as_deref(),
            Some("~/app")
        );
        assert_eq!(
            remote_cwd_from_title("root@box:/var/log").as_deref(),
            Some("/var/log")
        );
        assert_eq!(remote_cwd_from_title("vim notes.md"), None);
        assert_eq!(remote_cwd_from_title("Make: building"), None);
    }
}
