//! Agent mode's panel, beside the terminal: the conversation, each command and edit the agent
//! makes with its output, and the requests waiting for the user (⏎ allows, ⌘⏎ allows that kind
//! of command for the rest of the conversation, esc turns it down).

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionId, PermissionOptionKind, ToolCallStatus, ToolKind,
};
use futures::{StreamExt as _, channel::oneshot};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    ScrollHandle, Subscription, Task, Window, div, prelude::*, px, relative,
};
use ui::{
    Button, ButtonCommon, ButtonSize, ButtonStyle, Clickable, Color, Icon, IconButton, IconName,
    IconSize, Label, LabelCommon, LabelSize, h_flex, v_flex,
};

use super::{
    Conversation, Event, FileDiff, Permission, ToolUpdate, TurnHandle,
    policy::{self, Rules, Verdict},
};
use crate::{
    agent::{self, AgentCommand, Resolution},
    agent_panel::ai_badge,
    components::Kbd,
    shortcuts,
    terminal::Terminal,
    terminal_view::ToggleAgentMode,
    text_input::{Changed, TextInput},
    theme::MONO_FONT_FAMILY,
    tokens::{TokensExt, metrics, tint},
};

/// Output kept for each tool call, from its end.
const OUTPUT_BYTES: usize = 64 * 1024;
/// Output lines shown until the user asks for all of them.
const SHOWN_LINES: usize = 8;
const ALL_LINES: usize = 400;

pub enum AgentModeEvent {
    /// The × button: end the conversation.
    Close,
    /// Give the keyboard back to the terminal.
    FocusTerminal,
}

enum Entry {
    User(String),
    Agent(String),
    Tool(ToolEntry),
    Notice { text: String, error: bool },
}

struct ToolEntry {
    id: String,
    kind: ToolKind,
    title: String,
    status: ToolCallStatus,
    command: Option<String>,
    paths: Vec<PathBuf>,
    output: String,
    exit_code: Option<i32>,
    diffs: Vec<FileDiff>,
    approval: Approval,
}

enum Approval {
    None,
    /// Ran without asking.
    Auto,
    Waiting(Box<Ask>),
    Approved,
    Rejected,
}

struct Ask {
    verdict: Verdict,
    options: Vec<PermissionOption>,
    reply: oneshot::Sender<Option<PermissionOptionId>>,
    remember: Remember,
}

/// What "Always allow" covers for the rest of the conversation.
#[derive(Clone)]
enum Remember {
    /// Commands that start this way (`cargo test`).
    Commands(Vec<String>),
    /// Every edit inside the folder.
    Edits,
    Nothing,
}

#[derive(Clone, Copy)]
enum Answer {
    Allow,
    Always,
    Reject,
}

pub struct AgentModePanel {
    terminal: Entity<Terminal>,
    focus_handle: FocusHandle,
    input: Entity<TextInput>,
    agent: Result<AgentCommand, String>,
    conversation: Option<Conversation>,
    /// Where the conversation works, fixed when it starts.
    cwd: Option<PathBuf>,
    /// The agent's mode, as it names it ("Read-only", "Manual").
    mode: Option<String>,
    entries: Vec<Entry>,
    /// The turn in progress: dropping it cancels the turn.
    turn: Option<TurnHandle>,
    remembered: Vec<String>,
    allow_edits: bool,
    /// The user turned a request down in this turn: the turn ending early is expected.
    rejected: bool,
    expanded: HashSet<String>,
    scroll: ScrollHandle,
    _events: Option<Task<()>>,
    _input_changed: Subscription,
}

impl EventEmitter<AgentModeEvent> for AgentModePanel {}

impl AgentModePanel {
    pub fn new(terminal: Entity<Terminal>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_path = search_path(&terminal, cx);
        let agent = match agent::find(&search_path, cx) {
            Resolution::Found(command) => Ok(command),
            Resolution::Off => Err("The agent is off (\"agent\": false in the settings).".into()),
            Resolution::NotFound(why) => Err(format!(
                "{why}. Install an ACP adapter or set \"agent\" in the settings."
            )),
        };
        let input = cx.new(|cx| TextInput::new("Ask the agent to do something…", cx));
        let input_changed = cx.subscribe(&input, |_, _, _: &Changed, cx| cx.notify());
        input.focus_handle(cx).focus(window, cx);
        Self {
            terminal,
            focus_handle: cx.focus_handle(),
            input,
            agent,
            conversation: None,
            cwd: None,
            mode: None,
            entries: Vec::new(),
            turn: None,
            remembered: Vec::new(),
            allow_edits: false,
            rejected: false,
            expanded: HashSet::new(),
            scroll: ScrollHandle::new(),
            _events: None,
            _input_changed: input_changed,
        }
    }

    fn agent_name(&self) -> String {
        self.agent
            .as_ref()
            .map_or_else(|_| "the agent".to_string(), |agent| agent.name.clone())
    }

    fn working(&self) -> bool {
        self.turn.is_some()
    }

    fn waiting(&self) -> bool {
        self.entries.iter().any(|entry| {
            matches!(
                entry,
                Entry::Tool(ToolEntry {
                    approval: Approval::Waiting(_),
                    ..
                })
            )
        })
    }

    /// Why the conversation can't start here, if it can't.
    fn unavailable(&self, cx: &App) -> Option<String> {
        if let Err(why) = &self.agent {
            return Some(why.clone());
        }
        if self.terminal.read(cx).shell().remote.is_some() {
            return Some(
                "Agent mode works in this computer's folders, so it's off in SSH sessions.".into(),
            );
        }
        None
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let request = self.input.read(cx).text().trim().to_string();
        if request.is_empty() || self.working() {
            return;
        }
        if let Some(why) = self.unavailable(cx) {
            self.notice(why, true);
            cx.notify();
            return;
        }
        let first = self.conversation.is_none();
        if first && !self.start(cx) {
            cx.notify();
            return;
        }
        let redacted = crate::redact::redact(&request);
        let mut text = redacted.text;
        if first {
            text = format!("{}\n\n{text}", self.preface(cx));
        }
        let Some(turn) = self.conversation.as_ref().and_then(|c| c.send(text)) else {
            self.notice("The agent stopped.".into(), true);
            self.conversation = None;
            cx.notify();
            return;
        };
        self.turn = Some(turn);
        self.rejected = false;
        self.entries.push(Entry::User(request));
        if redacted.count > 0 {
            let text = match redacted.count {
                1 => "1 secret removed before sending".to_string(),
                n => format!("{n} secrets removed before sending"),
            };
            self.notice(text, false);
        }
        self.input.update(cx, |input, cx| input.set_text("", cx));
        self.input.focus_handle(cx).focus(window, cx);
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// Starts the agent in the shell's folder.
    fn start(&mut self, cx: &mut Context<Self>) -> bool {
        let Ok(command) = self.agent.clone() else {
            return false;
        };
        let shell = self.terminal.read(cx).shell();
        let cwd = shell
            .cwd
            .clone()
            .filter(|cwd| cwd.is_dir())
            .or_else(crate::state::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"));
        let search_path = search_path(&self.terminal, cx);
        let (conversation, mut events) =
            match Conversation::start(&command, search_path, cwd.clone()) {
                Ok(started) => started,
                Err(err) => {
                    self.notice(format!("Couldn't start the agent: {err}"), true);
                    return false;
                }
            };
        self.conversation = Some(conversation);
        self.cwd = Some(cwd);
        self._events = Some(cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                let alive = this.update(cx, |this, cx| this.handle(event, cx));
                if alive.is_err() {
                    return;
                }
            }
        }));
        true
    }

    /// What the agent is told before the first request.
    fn preface(&self, cx: &App) -> String {
        let configured = crate::config::get(cx).shell.clone();
        let (program, _) = crate::shell::shell_command(configured.as_deref());
        let shell = crate::process::command_name(Path::new(&program)).unwrap_or(program);
        let os = if cfg!(target_os = "macos") {
            "macOS"
        } else if cfg!(windows) {
            "Windows"
        } else {
            "Linux"
        };
        let cwd = self
            .cwd
            .as_deref()
            .map(agent::tilde)
            .unwrap_or_else(|| "~".into());
        format!(
            "You're the agent in Moth, a terminal app. The user works in {shell} on {os}, in {cwd}: work there. \
Commands that only read run right away; anything that changes something waits for the user's approval, \
so before such a step say in one sentence what it will change. Keep your messages short."
        )
    }

    fn notice(&mut self, text: String, error: bool) {
        self.entries.push(Entry::Notice { text, error });
    }

    fn handle(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Ready { mode } => {
                if mode.is_none() {
                    self.notice(
                        format!(
                            "{} has no mode that asks before changing things: it decides on its own what to run.",
                            self.agent_name()
                        ),
                        true,
                    );
                }
                self.mode = mode;
            }
            Event::Text(text) => match self.entries.last_mut() {
                Some(Entry::Agent(reply)) => reply.push_str(&text),
                _ => self.entries.push(Entry::Agent(text)),
            },
            Event::Tool(update) => {
                self.merge(update, false);
            }
            Event::Permission(permission) => self.permission(permission, cx),
            Event::TurnEnded(result) => {
                self.turn = None;
                self.reject_waiting();
                match result {
                    // Some agents (Codex) end the turn when a request is turned down.
                    Err(_) if self.rejected => self.notice(
                        "Turned down. Tell the agent what to do instead.".into(),
                        false,
                    ),
                    Err(why) => self.notice(why, true),
                    Ok(()) => {}
                }
            }
            Event::Stopped(why) => {
                let was_working = self.conversation.is_some();
                self.conversation = None;
                self.turn = None;
                self.mode = None;
                self.reject_waiting();
                if was_working {
                    self.notice(why, true);
                }
            }
        }
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// The entry for `update.id`, created if new, with what the update says. `fill` only sets
    /// what's still unknown: a permission request repeats the call with a vaguer title.
    fn merge(&mut self, update: ToolUpdate, fill: bool) -> &mut ToolEntry {
        let index = self
            .entries
            .iter()
            .position(|entry| matches!(entry, Entry::Tool(tool) if tool.id == update.id));
        let index = match index {
            Some(index) => index,
            None => {
                self.entries.push(Entry::Tool(ToolEntry {
                    id: update.id.clone(),
                    kind: ToolKind::Other,
                    title: String::new(),
                    status: ToolCallStatus::Pending,
                    command: None,
                    paths: Vec::new(),
                    output: String::new(),
                    exit_code: None,
                    diffs: Vec::new(),
                    approval: Approval::None,
                }));
                self.entries.len() - 1
            }
        };
        let Entry::Tool(tool) = &mut self.entries[index] else {
            unreachable!("the entry was just found or made as a tool call");
        };
        let new = tool.title.is_empty();
        if let Some(kind) = update.kind
            && (!fill || tool.kind == ToolKind::Other)
        {
            tool.kind = kind;
        }
        if let Some(title) = update.title
            && (!fill || new)
        {
            tool.title = title;
        }
        if let Some(status) = update.status
            && !fill
        {
            tool.status = status;
        }
        if let Some(command) = update.command
            && (!fill || tool.command.is_none())
        {
            tool.command = Some(command);
        }
        for path in update.paths {
            if !tool.paths.contains(&path) {
                tool.paths.push(path);
            }
        }
        if let Some(output) = update.output {
            tool.output = output;
        }
        if let Some(delta) = update.output_delta {
            tool.output.push_str(&delta);
        }
        if tool.output.len() > OUTPUT_BYTES {
            let mut cut = tool.output.len() - OUTPUT_BYTES;
            while !tool.output.is_char_boundary(cut) {
                cut += 1;
            }
            tool.output.drain(..cut);
        }
        if update.exit_code.is_some() {
            tool.exit_code = update.exit_code;
        }
        if !update.diffs.is_empty() {
            tool.diffs = update.diffs;
        }
        tool
    }

    fn permission(&mut self, permission: Permission, cx: &App) {
        let Permission {
            tool,
            options,
            reply,
        } = permission;
        let settings = crate::config::get(cx).agent_mode.clone();
        let cwd = self.cwd.clone();
        let remembered = self.remembered.clone();
        let allow_edits = self.allow_edits;
        let entry = self.merge(tool, true);
        let rules = Rules {
            allow: &settings.allow,
            deny: &settings.deny,
            remembered: &remembered,
        };
        let inside = |paths: &[PathBuf]| {
            !paths.is_empty()
                && paths.iter().all(|path| {
                    cwd.as_ref().is_some_and(|cwd| path.starts_with(cwd))
                        && !policy::secret_path(path)
                })
        };
        let verdict = match entry.kind {
            ToolKind::Execute => entry
                .command
                .as_deref()
                .map_or(Verdict::Ask, |command| policy::judge(command, &rules)),
            ToolKind::Read | ToolKind::Search if inside(&entry.paths) => Verdict::Run,
            ToolKind::Think => Verdict::Run,
            ToolKind::Edit if allow_edits && inside(&entry.paths) => Verdict::Run,
            ToolKind::Delete => Verdict::Warn,
            _ => Verdict::Ask,
        };
        let allow_once = option(&options, &[PermissionOptionKind::AllowOnce]);
        if verdict == Verdict::Run
            && let Some(id) = allow_once
        {
            reply.send(Some(id)).ok();
            entry.approval = Approval::Auto;
            return;
        }
        let remember = match (entry.kind, verdict) {
            (_, Verdict::Warn) => Remember::Nothing,
            (ToolKind::Execute, _) => {
                let prefixes = entry
                    .command
                    .as_deref()
                    .map(|command| policy::prefixes(command, &rules))
                    .unwrap_or_default();
                if prefixes.is_empty() {
                    Remember::Nothing
                } else {
                    Remember::Commands(prefixes)
                }
            }
            (ToolKind::Edit, _) if inside(&entry.paths) => Remember::Edits,
            _ => Remember::Nothing,
        };
        entry.approval = Approval::Waiting(Box::new(Ask {
            verdict,
            options,
            reply,
            remember,
        }));
    }

    fn answer(&mut self, answer: Answer, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tool) = self.entries.iter_mut().rev().find_map(|entry| match entry {
            Entry::Tool(tool) if matches!(tool.approval, Approval::Waiting(_)) => Some(tool),
            _ => None,
        }) else {
            return;
        };
        let Approval::Waiting(ask) = std::mem::replace(&mut tool.approval, Approval::None) else {
            return;
        };
        let Ask {
            options,
            reply,
            remember,
            ..
        } = *ask;
        let (choice, approval) = match answer {
            Answer::Allow | Answer::Always => (
                option(
                    &options,
                    &[
                        PermissionOptionKind::AllowOnce,
                        PermissionOptionKind::AllowAlways,
                    ],
                ),
                Approval::Approved,
            ),
            Answer::Reject => (reject_option(&options), Approval::Rejected),
        };
        tool.approval = approval;
        reply.send(choice).ok();
        if matches!(answer, Answer::Reject) {
            self.rejected = true;
        }
        if matches!(answer, Answer::Always) {
            match remember {
                Remember::Commands(prefixes) => {
                    for prefix in prefixes {
                        if !self.remembered.contains(&prefix) {
                            self.remembered.push(prefix);
                        }
                    }
                }
                Remember::Edits => self.allow_edits = true,
                Remember::Nothing => {}
            }
        }
        self.input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Requests still waiting when the turn ends are turned down (their senders dropped).
    fn reject_waiting(&mut self) {
        for entry in &mut self.entries {
            if let Entry::Tool(tool) = entry
                && matches!(tool.approval, Approval::Waiting(_))
            {
                tool.approval = Approval::Rejected;
            }
        }
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        // The handle's drop tells the agent to stop; the turn ends with the agent's answer.
        self.turn = None;
        self.reject_waiting();
        self.notice("Stopped.".into(), false);
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let m = &keystroke.modifiers;
        let plain = !(m.control || m.alt || m.shift || m.platform);
        match keystroke.key.as_str() {
            "enter" if self.waiting() && m.secondary() => self.answer(Answer::Always, window, cx),
            "enter" if self.waiting() && plain => self.answer(Answer::Allow, window, cx),
            "escape" if self.waiting() => self.answer(Answer::Reject, window, cx),
            "escape" if self.working() => self.stop(cx),
            "escape" => cx.emit(AgentModeEvent::FocusTerminal),
            "enter" if plain => self.send(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn icon(&self) -> IconName {
        let name = self.agent_name().to_lowercase();
        if name.contains("claude") {
            IconName::AiClaude
        } else if name.contains("gemini") {
            IconName::AiGemini
        } else if name.contains("codex") {
            IconName::AiOpenAi
        } else {
            IconName::Sparkle
        }
    }
}

/// The first option of the kinds asked for, in that order.
fn option(
    options: &[PermissionOption],
    kinds: &[PermissionOptionKind],
) -> Option<PermissionOptionId> {
    kinds.iter().find_map(|kind| {
        options
            .iter()
            .find(|option| option.kind == *kind)
            .map(|option| option.option_id.clone())
    })
}

/// The option that turns a request down and lets the agent go on, if there's one that doesn't
/// end the turn ("cancel", "abort"); otherwise any that turns it down.
fn reject_option(options: &[PermissionOption]) -> Option<PermissionOptionId> {
    let rejects = || {
        options
            .iter()
            .filter(|option| option.kind == PermissionOptionKind::RejectOnce)
    };
    let ends_turn = |option: &&PermissionOption| {
        let id = option.option_id.0.to_lowercase();
        id.contains("cancel") || id.contains("abort")
    };
    rejects()
        .find(|option| !ends_turn(option))
        .or_else(|| rejects().next())
        .map(|option| option.option_id.clone())
}

fn search_path(terminal: &Entity<Terminal>, cx: &App) -> String {
    terminal
        .read(cx)
        .shell()
        .path_env
        .clone()
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default()
}

/// The keyboard stays in the input: the panel's keys (⏎, ⌘⏎, esc) reach it from there.
impl Focusable for AgentModePanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

/// The last `lines` lines of `text`, and how many there are in all.
fn tail(text: &str, lines: usize) -> (String, usize) {
    let all: Vec<&str> = text.trim_end().lines().collect();
    let start = all.len().saturating_sub(lines);
    (all[start..].join("\n"), all.len())
}

/// Prose and code blocks (between ``` fences), in order.
fn blocks(text: &str) -> Vec<(bool, String)> {
    let mut blocks = Vec::new();
    for (i, part) in text.split("```").enumerate() {
        let code = i % 2 == 1;
        let part = if code {
            // The language after the fence.
            part.split_once('\n').map_or(part, |(first, rest)| {
                if first
                    .trim()
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
                {
                    rest
                } else {
                    part
                }
            })
        } else {
            part
        };
        let part = part.trim_matches('\n');
        if !part.trim().is_empty() {
            blocks.push((code, part.to_string()));
        }
    }
    blocks
}

fn tool_icon(kind: ToolKind) -> IconName {
    match kind {
        ToolKind::Execute => IconName::ToolTerminal,
        ToolKind::Read | ToolKind::Search => IconName::ToolSearch,
        ToolKind::Edit | ToolKind::Move => IconName::ToolPencil,
        ToolKind::Delete => IconName::ToolDeleteFile,
        ToolKind::Fetch => IconName::ToolWeb,
        ToolKind::Think => IconName::ToolThink,
        _ => IconName::ToolHammer,
    }
}

impl AgentModePanel {
    fn render_entry(
        &self,
        index: usize,
        entry: &Entry,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = cx.tokens();
        let ui = tokens.ui.clone();
        let error = tokens.terminal.ansi.red;
        let code_box = |text: String| {
            div()
                .w_full()
                .px(px(8.))
                .py(px(6.))
                .rounded(metrics::RADIUS_ROW)
                .bg(ui.background)
                .font_family(MONO_FONT_FAMILY)
                .text_size(px(11.5))
                .text_color(ui.text_muted)
                .child(text)
        };
        match entry {
            Entry::User(text) => h_flex()
                .w_full()
                .justify_end()
                .child(
                    div()
                        .max_w(relative(0.9))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(8.))
                        .bg(tint(ui.accent, 0.14))
                        .text_size(px(12.5))
                        .text_color(ui.text)
                        .child(text.clone()),
                )
                .into_any_element(),
            Entry::Agent(text) => v_flex()
                .w_full()
                .gap(px(6.))
                .children(blocks(text).into_iter().map(|(code, text)| {
                    if code {
                        code_box(text).into_any_element()
                    } else {
                        div()
                            .w_full()
                            .text_size(px(12.5))
                            .text_color(ui.text)
                            .child(text)
                            .into_any_element()
                    }
                }))
                .into_any_element(),
            Entry::Notice { text, error: bad } => h_flex()
                .gap(px(6.))
                .child(
                    Icon::new(if *bad {
                        IconName::Warning
                    } else {
                        IconName::Info
                    })
                    .size(IconSize::XSmall)
                    .color(if *bad {
                        Color::Custom(error)
                    } else {
                        Color::Placeholder
                    }),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        Label::new(text.clone())
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
                )
                .into_any_element(),
            Entry::Tool(tool) => self.render_tool(index, tool, cx),
        }
    }

    fn render_tool(
        &self,
        index: usize,
        tool: &ToolEntry,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = cx.tokens();
        let ui = tokens.ui.clone();
        let error = tokens.terminal.ansi.red;
        let green = tokens.terminal.ansi.green;
        let waiting = match &tool.approval {
            Approval::Waiting(ask) => Some(ask),
            _ => None,
        };
        let title = match (&tool.command, tool.kind) {
            (Some(command), ToolKind::Execute) => command.clone(),
            _ if !tool.title.is_empty() => tool.title.clone(),
            _ => "Tool".to_string(),
        };
        let failed =
            tool.status == ToolCallStatus::Failed || tool.exit_code.is_some_and(|code| code != 0);
        let status = match (&tool.approval, tool.status) {
            (Approval::Rejected, _) => Some(("rejected".to_string(), Color::Muted)),
            (Approval::Waiting(_), _) => Some(("waiting for you".to_string(), Color::Warning)),
            (_, ToolCallStatus::Completed) if failed => Some((
                tool.exit_code
                    .map_or_else(|| "failed".to_string(), |code| format!("exit {code}")),
                Color::Custom(error),
            )),
            (_, ToolCallStatus::Failed) => Some(("failed".to_string(), Color::Custom(error))),
            (_, ToolCallStatus::Completed) => None,
            _ => Some(("running".to_string(), Color::Placeholder)),
        };
        let border = match waiting {
            Some(ask) if ask.verdict == Verdict::Warn => tint(error, 0.7),
            Some(_) => tint(ui.accent, 0.7),
            None => ui.border,
        };
        let header = h_flex()
            .w_full()
            .gap(px(6.))
            .child(
                Icon::new(tool_icon(tool.kind))
                    .size(IconSize::XSmall)
                    .color(Color::Muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(tool.kind == ToolKind::Execute, |el| {
                        el.font_family(MONO_FONT_FAMILY)
                    })
                    .text_size(px(12.))
                    .text_color(ui.text)
                    .child(title),
            )
            .when(matches!(tool.approval, Approval::Auto), |el| {
                el.child(
                    Label::new("auto")
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder),
                )
            })
            .when_some(status, |el, (text, color)| {
                el.child(Label::new(text).size(LabelSize::XSmall).color(color))
            })
            .when(tool.status == ToolCallStatus::Completed && !failed, |el| {
                el.child(
                    Icon::new(IconName::Check)
                        .size(IconSize::XSmall)
                        .color(Color::Custom(green)),
                )
            });

        let diffs = (!tool.diffs.is_empty()).then(|| {
            v_flex().gap(px(2.)).children(tool.diffs.iter().map(|diff| {
                h_flex()
                    .gap(px(8.))
                    .child(
                        div().flex_1().min_w_0().child(
                            Label::new(agent::tilde(&diff.path))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted)
                                .truncate(),
                        ),
                    )
                    .child(
                        Label::new(format!("+{}", diff.added))
                            .size(LabelSize::XSmall)
                            .color(Color::Custom(green)),
                    )
                    .child(
                        Label::new(format!("−{}", diff.removed))
                            .size(LabelSize::XSmall)
                            .color(Color::Custom(error)),
                    )
            }))
        });
        let paths =
            (tool.diffs.is_empty() && tool.kind != ToolKind::Execute && !tool.paths.is_empty())
                .then(|| {
                    Label::new(
                        tool.paths
                            .iter()
                            .map(|path| agent::tilde(path))
                            .collect::<Vec<_>>()
                            .join(", "),
                    )
                    .size(LabelSize::XSmall)
                    .color(Color::Placeholder)
                    .truncate()
                });

        let output = (!tool.output.trim().is_empty()).then(|| {
            let expanded = self.expanded.contains(&tool.id);
            let (shown, total) = tail(&tool.output, if expanded { ALL_LINES } else { SHOWN_LINES });
            let id = tool.id.clone();
            v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .id(("agent-mode-output", index))
                        .w_full()
                        .max_h(px(if expanded { 360. } else { 160. }))
                        .overflow_y_scroll()
                        .px(px(8.))
                        .py(px(6.))
                        .rounded(metrics::RADIUS_ROW)
                        .bg(ui.background)
                        .font_family(MONO_FONT_FAMILY)
                        .text_size(px(11.))
                        .text_color(ui.text_muted)
                        .child(shown),
                )
                .when(total > SHOWN_LINES, |el| {
                    el.child(
                        Button::new(
                            ("agent-mode-more", index),
                            if expanded {
                                "Show less".to_string()
                            } else {
                                format!("Show all {total} lines")
                            },
                        )
                        .style(ButtonStyle::Subtle)
                        .size(ButtonSize::Compact)
                        .label_size(LabelSize::XSmall)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.expanded.remove(&id) {
                                this.expanded.insert(id.clone());
                            }
                            cx.notify();
                        })),
                    )
                })
        });

        let ask = waiting.map(|ask| {
            let warn = ask.verdict == Verdict::Warn;
            let question = match tool.kind {
                ToolKind::Execute if warn => "This command can't be undone. Run it?",
                ToolKind::Execute => "Run this command?",
                ToolKind::Edit => "Make this edit?",
                ToolKind::Delete => "Delete this? It can't be undone.",
                ToolKind::Fetch => "Let the agent go online?",
                _ if warn => "This can't be undone. Allow it?",
                _ => "Allow this?",
            };
            let always = match &ask.remember {
                Remember::Commands(prefixes) => Some(format!(
                    "Always allow {}",
                    prefixes
                        .iter()
                        .map(|p| format!("`{p}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
                Remember::Edits => Some("Allow all edits".to_string()),
                Remember::Nothing => None,
            };
            let run_label = if tool.kind == ToolKind::Execute {
                "Run"
            } else {
                "Allow"
            };
            v_flex()
                .gap(px(8.))
                .pt(px(4.))
                .child(
                    h_flex()
                        .gap(px(6.))
                        .when(warn, |el| {
                            el.child(
                                Icon::new(IconName::Warning)
                                    .size(IconSize::XSmall)
                                    .color(Color::Custom(error)),
                            )
                        })
                        .child(Label::new(question).size(LabelSize::Small).color(if warn {
                            Color::Custom(error)
                        } else {
                            Color::Default
                        })),
                )
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .child(
                            Button::new(("agent-mode-allow", index), run_label)
                                .style(ButtonStyle::Filled)
                                .size(ButtonSize::Medium)
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.answer(Answer::Allow, window, cx)
                                })),
                        )
                        .child(Kbd::new("⏎"))
                        .when_some(always, |el, label| {
                            el.child(div().w(px(4.)))
                                .child(
                                    Button::new(("agent-mode-always", index), label)
                                        .style(ButtonStyle::Outlined)
                                        .size(ButtonSize::Medium)
                                        .label_size(LabelSize::Small)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.answer(Answer::Always, window, cx)
                                        })),
                                )
                                .child(Kbd::new(shortcuts::label("cmd-enter")))
                        })
                        .child(div().w(px(4.)))
                        .child(
                            Button::new(("agent-mode-reject", index), "Reject")
                                .style(ButtonStyle::Subtle)
                                .size(ButtonSize::Medium)
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.answer(Answer::Reject, window, cx)
                                })),
                        )
                        .child(Kbd::new("esc")),
                )
        });

        v_flex()
            .w_full()
            .gap(px(6.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(8.))
            .border_1()
            .border_color(border)
            .bg(ui.surface)
            .child(header)
            .children(paths)
            .children(diffs)
            .children(output)
            .children(ask)
            .into_any_element()
    }
}

impl Render for AgentModePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = cx.tokens();
        let ui = tokens.ui.clone();
        let accent = ui.accent;
        let hint = |key: String, label: &'static str| {
            h_flex().gap(px(5.)).child(Kbd::new(key)).child(
                Label::new(label)
                    .size(LabelSize::XSmall)
                    .color(Color::Placeholder),
            )
        };

        let place = self
            .cwd
            .clone()
            .or_else(|| self.terminal.read(cx).shell().cwd.clone())
            .map(|cwd| agent::tilde(&cwd));
        let header = v_flex()
            .gap(px(2.))
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(ui.divider)
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(ai_badge(accent))
                    .child(
                        Icon::new(self.icon())
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(
                        Label::new("Agent Mode")
                            .size(LabelSize::Small)
                            .weight(gpui::FontWeight::SEMIBOLD),
                    )
                    .child(
                        div().min_w_0().child(
                            Label::new(self.agent_name())
                                .size(LabelSize::XSmall)
                                .color(Color::Placeholder)
                                .truncate(),
                        ),
                    )
                    .child(div().flex_1())
                    .child(
                        IconButton::new("agent-mode-close", IconName::Close)
                            .icon_size(IconSize::Small)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(AgentModeEvent::Close))),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(6.))
                    .when_some(place, |el, place| {
                        el.child(
                            div().min_w_0().child(
                                Label::new(place)
                                    .size(LabelSize::XSmall)
                                    .color(Color::Placeholder)
                                    .truncate(),
                            ),
                        )
                    })
                    .when_some(self.mode.clone(), |el, mode| {
                        el.child(
                            Label::new(format!("· {mode} mode"))
                                .size(LabelSize::XSmall)
                                .color(Color::Placeholder),
                        )
                    }),
            );

        let empty = self.entries.is_empty().then(|| {
            let why = self.unavailable(cx);
            v_flex()
                .gap(px(8.))
                .pt(px(4.))
                .child(
                    Label::new(why.clone().unwrap_or_else(|| {
                        "Describe a task, like “find why the tests fail and fix it”. The agent works in this folder: commands that only read run on their own, and anything that changes something asks you first.".into()
                    }))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
                .when(why.is_none(), |el| {
                    el.child(
                        Label::new(
                            "Allow more in the settings with \"agent_mode\": { \"allow\": [\"cargo test\"] }.",
                        )
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder),
                    )
                })
        });

        let entries: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| self.render_entry(index, entry, cx))
            .collect();

        let transcript = div()
            .id("agent-mode-transcript")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                v_flex()
                    .w_full()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(10.))
                    .children(empty)
                    .children(entries)
                    .when(self.working() && !self.waiting(), |el| {
                        el.child(
                            Label::new(format!("{} is working…", self.agent_name()))
                                .size(LabelSize::XSmall)
                                .color(Color::Placeholder),
                        )
                    }),
            );

        let working = self.working();
        let composer = v_flex()
            .gap(px(6.))
            .px(px(12.))
            .py(px(10.))
            .border_t_1()
            .border_color(ui.divider)
            .child(
                h_flex()
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(metrics::RADIUS_ROW)
                    .bg(ui.background)
                    .border_1()
                    .border_color(ui.border)
                    .when(working, |el| el.opacity(0.6))
                    .child(div().flex_1().min_w_0().child(self.input.clone())),
            )
            .child(h_flex().gap(px(12.)).map(|el| {
                if self.waiting() {
                    el.child(
                        Label::new("The agent is waiting for your answer above.")
                            .size(LabelSize::XSmall)
                            .color(Color::Warning),
                    )
                } else if working {
                    el.child(hint("esc".into(), "stop")).child(
                        Button::new("agent-mode-stop", "Stop")
                            .style(ButtonStyle::Subtle)
                            .size(ButtonSize::Compact)
                            .label_size(LabelSize::XSmall)
                            .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
                    )
                } else {
                    el.child(hint("⏎".into(), "send"))
                        .child(hint("esc".into(), "back to the terminal"))
                }
            }));

        v_flex()
            .key_context("AgentMode")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .on_action(
                cx.listener(|_, _: &ToggleAgentMode, _, cx| cx.emit(AgentModeEvent::FocusTerminal)),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .h_full()
            .w(relative(0.42))
            .min_w(px(300.))
            .max_w(px(460.))
            .flex_none()
            .bg(ui.surface)
            .border_l_1()
            .border_color(ui.border)
            .child(header)
            .child(transcript)
            .child(composer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_replies_into_prose_and_code() {
        assert_eq!(
            blocks("Run this:\n```sh\ncargo test\n```\nThen look."),
            [
                (false, "Run this:".to_string()),
                (true, "cargo test".to_string()),
                (false, "Then look.".to_string())
            ]
        );
        assert_eq!(blocks("plain"), [(false, "plain".to_string())]);
    }

    #[test]
    fn shows_the_end_of_long_output() {
        assert_eq!(tail("a\nb\nc\n", 2), ("b\nc".to_string(), 3));
        assert_eq!(tail("a", 8), ("a".to_string(), 1));
    }
}
