//! Agent mode (⌘⇧A): a conversation with the agent that works in the shell's folder, reading,
//! running commands and editing files to do what the user asks, as in Warp.
//!
//! Unlike ⌘I, the agent here has its own tools. Moth starts each conversation in the agent's
//! most careful mode (Codex's read-only sandbox, Claude's "Manual"), where anything beyond
//! reading comes to Moth as a permission request; `policy` decides which of those go ahead on
//! their own and the panel asks the user about the rest. The conversation runs its own agent
//! process, so ⌘I stays quick while the agent works.

pub mod panel;
pub mod policy;

use std::{path::PathBuf, thread};

use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, Agent, Client, ConnectionTo, SessionMessage,
    schema::{
        ProtocolVersion,
        v1::{
            CancelNotification, ContentBlock, ContentChunk, Implementation, InitializeRequest,
            PermissionOption, PermissionOptionId, RequestPermissionOutcome,
            RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
            SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue, SessionId,
            SessionModeState, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
            SetSessionModeRequest, StopReason, ToolCall, ToolCallContent, ToolCallStatus,
            ToolCallUpdate, ToolKind,
        },
    },
    util::MatchDispatch,
};
use futures::{
    StreamExt as _,
    channel::{
        mpsc::{UnboundedReceiver, UnboundedSender, unbounded},
        oneshot,
    },
    future::{self, Either},
};

use crate::agent::{self, AgentCommand};

/// What the panel hears from the conversation, in order.
pub enum Event {
    /// The session is open, in the mode named (`None`: the agent has none that asks first).
    Ready {
        mode: Option<String>,
    },
    Text(String),
    /// A tool call, new or changed: merge by `id`.
    Tool(ToolUpdate),
    /// The agent wants to do something: answer through `reply`.
    Permission(Permission),
    /// The turn is over: `Err` says why it ended early.
    TurnEnded(Result<(), String>),
    /// The agent's process is gone; nothing more will come.
    Stopped(String),
}

pub struct Permission {
    pub tool: ToolUpdate,
    pub options: Vec<PermissionOption>,
    /// The option chosen, or `None` to turn it down without one.
    pub reply: oneshot::Sender<Option<PermissionOptionId>>,
}

/// What a tool call update says; fields left `None` didn't change.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolUpdate {
    pub id: String,
    pub kind: Option<ToolKind>,
    pub title: Option<String>,
    pub status: Option<ToolCallStatus>,
    /// The shell command, for a tool that runs one.
    pub command: Option<String>,
    pub paths: Vec<PathBuf>,
    /// All the output so far, replacing what came before.
    pub output: Option<String>,
    /// More output, after what came before.
    pub output_delta: Option<String>,
    pub exit_code: Option<i32>,
    pub diffs: Vec<FileDiff>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileDiff {
    pub path: PathBuf,
    pub added: usize,
    pub removed: usize,
}

impl ToolUpdate {
    fn from_call(call: &ToolCall) -> Self {
        let mut update = Self {
            id: call.tool_call_id.0.to_string(),
            kind: Some(call.kind),
            title: Some(call.title.clone()),
            status: Some(call.status),
            paths: call.locations.iter().map(|l| l.path.clone()).collect(),
            ..Self::default()
        };
        update.read_input(call.raw_input.as_ref());
        update.read_content(&call.content);
        update.read_meta(call.meta.as_ref());
        update
    }

    fn from_update(call: &ToolCallUpdate) -> Self {
        let fields = &call.fields;
        let mut update = Self {
            id: call.tool_call_id.0.to_string(),
            kind: fields.kind,
            title: fields.title.clone(),
            status: fields.status,
            paths: fields
                .locations
                .iter()
                .flatten()
                .map(|l| l.path.clone())
                .collect(),
            ..Self::default()
        };
        update.read_input(fields.raw_input.as_ref());
        if let Some(content) = &fields.content {
            update.read_content(content);
        }
        update.read_meta(call.meta.as_ref());
        update
    }

    fn read_input(&mut self, input: Option<&serde_json::Value>) {
        if let Some(command) = input
            .and_then(|input| input.get("command"))
            .and_then(command_text)
        {
            self.command = Some(command);
        }
    }

    fn read_content(&mut self, content: &[ToolCallContent]) {
        let mut text = String::new();
        for item in content {
            match item {
                ToolCallContent::Content(content) => {
                    if let ContentBlock::Text(block) = &content.content {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(&block.text);
                    }
                }
                ToolCallContent::Diff(diff) => {
                    let (added, removed) =
                        count_changes(diff.old_text.as_deref().unwrap_or(""), &diff.new_text);
                    self.diffs.push(FileDiff {
                        path: diff.path.clone(),
                        added,
                        removed,
                    });
                }
                _ => {}
            }
        }
        if !text.is_empty() {
            self.output = Some(strip_fence(&text));
        }
    }

    /// Codex streams a command's output and exit code in `_meta`, as Zed reads them.
    fn read_meta(&mut self, meta: Option<&serde_json::Map<String, serde_json::Value>>) {
        let Some(meta) = meta else {
            return;
        };
        if let Some(data) = meta
            .get("terminal_output_delta")
            .and_then(|delta| delta.get("data"))
            .and_then(|data| data.as_str())
        {
            self.output_delta = Some(data.to_string());
        }
        if let Some(code) = meta
            .get("terminal_exit")
            .and_then(|exit| exit.get("exit_code"))
            .and_then(|code| code.as_i64())
        {
            self.exit_code = i32::try_from(code).ok();
        }
    }
}

/// A command as the agent sends it: a string (Codex quotes it once more in permission
/// requests), or the program and its arguments (`["bash", "-lc", "…"]`).
fn command_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => {
            let text = text.trim();
            let unquoted = text
                .starts_with('"')
                .then(|| serde_json::from_str::<String>(text).ok())
                .flatten();
            Some(unquoted.unwrap_or_else(|| text.to_string()))
        }
        serde_json::Value::Array(items) => {
            let words: Vec<&str> = items.iter().filter_map(|item| item.as_str()).collect();
            match words.as_slice() {
                [shell, flag, script]
                    if matches!(*flag, "-c" | "-lc")
                        && ["sh", "bash", "zsh"]
                            .contains(&shell.rsplit('/').next().unwrap_or(shell)) =>
                {
                    Some((*script).to_string())
                }
                [] => None,
                words => Some(
                    words
                        .iter()
                        .map(|word| shell_quote(word))
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
            }
        }
        _ => None,
    }
}

fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c))
    {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// Output Claude wraps in a code fence, without it.
fn strip_fence(text: &str) -> String {
    let trimmed = text.trim();
    if let Some(inner) = trimmed.strip_prefix("```")
        && let Some(inner) = inner.strip_suffix("```")
    {
        // The language after the opening fence.
        return inner
            .split_once('\n')
            .map_or("", |(_, body)| body)
            .trim_end()
            .to_string();
    }
    text.to_string()
}

/// Lines added and removed, roughly: each line counted by how many more times it appears.
fn count_changes(old: &str, new: &str) -> (usize, usize) {
    let mut counts: std::collections::HashMap<&str, isize> = std::collections::HashMap::new();
    for line in old.lines() {
        *counts.entry(line).or_default() -= 1;
    }
    for line in new.lines() {
        *counts.entry(line).or_default() += 1;
    }
    let added = counts
        .values()
        .filter(|n| **n > 0)
        .map(|n| *n as usize)
        .sum();
    let removed = counts
        .values()
        .filter(|n| **n < 0)
        .map(|n| n.unsigned_abs())
        .sum();
    (added, removed)
}

/// A conversation: dropping it stops the agent.
pub struct Conversation {
    turns: UnboundedSender<Turn>,
}

struct Turn {
    text: String,
    /// Resolves when the panel cancels the turn (or drops its end).
    cancel: oneshot::Receiver<()>,
}

/// A turn in progress: dropping it cancels the turn.
pub struct TurnHandle {
    _cancel: oneshot::Sender<()>,
}

impl Conversation {
    /// Starts `command` in `cwd` on a thread of its own; what it says comes through the
    /// returned receiver.
    pub fn start(
        command: &AgentCommand,
        search_path: String,
        cwd: PathBuf,
    ) -> std::io::Result<(Self, UnboundedReceiver<Event>)> {
        let (turns, turns_rx) = unbounded();
        let (events, events_rx) = unbounded();
        let command = agent::quiet(command);
        thread::Builder::new()
            .name("agent-mode".into())
            .spawn(move || {
                futures::executor::block_on(run(command, search_path, cwd, turns_rx, events))
            })?;
        Ok((Self { turns }, events_rx))
    }

    /// Sends what the user wrote; `None` if the agent is gone.
    pub fn send(&self, text: String) -> Option<TurnHandle> {
        let (cancel_tx, cancel) = oneshot::channel();
        self.turns.unbounded_send(Turn { text, cancel }).ok()?;
        Some(TurnHandle { _cancel: cancel_tx })
    }
}

async fn run(
    command: AgentCommand,
    search_path: String,
    cwd: PathBuf,
    mut turns: UnboundedReceiver<Turn>,
    events: UnboundedSender<Event>,
) {
    let mut config = AcpAgentConfig::new(&command.program)
        .args(command.args.clone())
        .envs(command.env.clone());
    if !command.env.contains_key("PATH") {
        config = config.env("PATH", search_path);
    }
    let result = Client
        .builder()
        .name("moth")
        // Permission requests go to the session (`run_turn`): a handler here would take
        // them first.
        .connect_with(
            AcpAgent::new(config),
            async |connection: ConnectionTo<Agent>| {
                connection
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
                            .client_info(Implementation::new("moth", env!("CARGO_PKG_VERSION"))),
                    )
                    .block_task()
                    .await?;
                connection
                    .build_session(&cwd)
                    .block_task()
                    .run_until(async |mut session| {
                        let session_id = session.session_id().clone();
                        let mode = ask_first(
                            session.connection(),
                            &session_id,
                            session.config_options().unwrap_or_default(),
                            session.modes(),
                        )
                        .await;
                        events.unbounded_send(Event::Ready { mode }).ok();
                        while let Some(turn) = turns.next().await {
                            let ended = run_turn(&mut session, turn, &events)
                                .await
                                .map_err(|why| agent::explain(&command.name, why));
                            events.unbounded_send(Event::TurnEnded(ended)).ok();
                        }
                        Ok(())
                    })
                    .await
            },
        )
        .await;
    let why = match result {
        Ok(()) => "The conversation ended.".to_string(),
        Err(err) => {
            crate::log::error(format!("agent mode {}: {err}", command.program.display()));
            agent::explain(&command.name, agent::message(&err))
        }
    };
    events.unbounded_send(Event::Stopped(why)).ok();
}

/// Mode ids that make the agent ask before changing anything, most careful first: Codex's
/// read-only sandbox, then Claude's and Gemini's default.
const ASKING_MODES: &[&str] = &["read-only", "default", "manual", "ask"];

/// Puts the session in the agent's most careful mode, so changes come to Moth to approve.
/// Returns that mode's name.
async fn ask_first(
    connection: &ConnectionTo<Agent>,
    session: &SessionId,
    options: &[SessionConfigOption],
    modes: Option<&SessionModeState>,
) -> Option<String> {
    let rank = |id: &str| {
        ASKING_MODES
            .iter()
            .position(|mode| mode.eq_ignore_ascii_case(id))
    };
    // As a config option (both adapters announce it this way)…
    if let Some(option) = options
        .iter()
        .find(|o| o.category == Some(SessionConfigOptionCategory::Mode))
        && let Some(select) = agent::as_select(option)
    {
        let values = agent::values(select);
        if let Some(value) = values
            .iter()
            .filter_map(|v| rank(&v.value.0).map(|r| (r, v)))
            .min_by_key(|(r, _)| *r)
            .map(|(_, v)| v)
        {
            if select.current_value != value.value {
                let request = SetSessionConfigOptionRequest::new(
                    session.clone(),
                    option.id.clone(),
                    SessionConfigOptionValue::value_id(value.value.clone()),
                );
                if let Err(err) = connection.send_request(request).block_task().await {
                    crate::log::error(format!("agent mode: couldn't set the mode: {err}"));
                    return None;
                }
            }
            return Some(value.name.clone());
        }
    }
    // …or as a session mode.
    let modes = modes?;
    let mode = modes
        .available_modes
        .iter()
        .filter_map(|m| rank(&m.id.0).map(|r| (r, m)))
        .min_by_key(|(r, _)| *r)
        .map(|(_, m)| m)?;
    if modes.current_mode_id != mode.id {
        let request = SetSessionModeRequest::new(session.clone(), mode.id.clone());
        if let Err(err) = connection.send_request(request).block_task().await {
            crate::log::error(format!("agent mode: couldn't set the mode: {err}"));
            return None;
        }
    }
    Some(mode.name.clone())
}

type Session<'a> = agent_client_protocol::ActiveSession<'a, Agent>;

/// One turn: the prompt, then updates until the agent stops, with its permission requests
/// sent to the panel.
async fn run_turn(
    session: &mut Session<'_>,
    turn: Turn,
    events: &UnboundedSender<Event>,
) -> Result<(), String> {
    let Turn { text, mut cancel } = turn;
    let session_id = session.session_id().clone();
    let to_agent = session.connection().clone();
    session
        .send_prompt(text)
        .map_err(|err| agent::message(&err))?;
    let mut cancelled = false;
    loop {
        let update = if cancelled {
            session.read_update().await
        } else {
            let next = Box::pin(session.read_update());
            match future::select(next, &mut cancel).await {
                Either::Left((update, _)) => update,
                Either::Right(_) => {
                    // The agent still ends the turn, with `cancelled`.
                    to_agent
                        .send_notification(CancelNotification::new(session_id.clone()))
                        .map_err(|err| agent::message(&err))?;
                    cancelled = true;
                    continue;
                }
            }
        };
        match update.map_err(|err| agent::message(&err))? {
            SessionMessage::SessionMessage(dispatch) => {
                let result = MatchDispatch::new(dispatch)
                    .if_notification(async |notification: SessionNotification| {
                        forward(notification.update, events);
                        Ok(())
                    })
                    .await
                    .if_request(async |request: RequestPermissionRequest, responder| {
                        let (reply, answer) = oneshot::channel();
                        let sent = !cancelled
                            && events
                                .unbounded_send(Event::Permission(Permission {
                                    tool: ToolUpdate::from_update(&request.tool_call),
                                    options: request.options.clone(),
                                    reply,
                                }))
                                .is_ok();
                        let choice = if sent {
                            match future::select(answer, &mut cancel).await {
                                Either::Left((Ok(choice), _)) => choice,
                                Either::Left((Err(_), _)) => None,
                                Either::Right(_) => {
                                    to_agent
                                        .send_notification(CancelNotification::new(
                                            session_id.clone(),
                                        ))
                                        .ok();
                                    cancelled = true;
                                    None
                                }
                            }
                        } else {
                            None
                        };
                        responder.respond(RequestPermissionResponse::new(match choice {
                            Some(id) => RequestPermissionOutcome::Selected(
                                SelectedPermissionOutcome::new(id),
                            ),
                            None => RequestPermissionOutcome::Cancelled,
                        }))
                    })
                    .await
                    .otherwise(async |dispatch| match dispatch {
                        // Moth offers no file system or terminal of its own.
                        agent_client_protocol::Dispatch::Request(_, responder) => responder
                            .respond_with_error(agent_client_protocol::Error::method_not_found()),
                        _ => Ok(()),
                    })
                    .await;
                result.map_err(|err| agent::message(&err))?;
            }
            SessionMessage::StopReason(reason) => {
                return match reason {
                    StopReason::Cancelled if cancelled => Ok(()),
                    StopReason::Cancelled => Err("The agent stopped.".into()),
                    StopReason::Refusal => Err("The agent declined to go on.".into()),
                    StopReason::MaxTokens => {
                        Err("The agent ran out of room for its answer.".into())
                    }
                    StopReason::MaxTurnRequests => {
                        Err("The agent reached its limit of steps for one request.".into())
                    }
                    _ => Ok(()),
                };
            }
            _ => {}
        }
    }
}

fn forward(update: SessionUpdate, events: &UnboundedSender<Event>) {
    let event = match update {
        SessionUpdate::AgentMessageChunk(ContentChunk {
            content: ContentBlock::Text(text),
            ..
        }) => Event::Text(text.text),
        SessionUpdate::ToolCall(call) => Event::Tool(ToolUpdate::from_call(&call)),
        SessionUpdate::ToolCallUpdate(call) => Event::Tool(ToolUpdate::from_update(&call)),
        _ => return,
    };
    events.unbounded_send(event).ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_commands_the_way_each_agent_sends_them() {
        let text = |json: &str| command_text(&serde_json::from_str(json).unwrap());
        assert_eq!(text(r#""ls -la""#).as_deref(), Some("ls -la"));
        // Codex's permission requests quote the command once more.
        assert_eq!(
            text(r#""\"printf 'hi\\\\n' > hello.txt\"""#).as_deref(),
            Some(r"printf 'hi\n' > hello.txt")
        );
        assert_eq!(
            text(r#"["/bin/zsh", "-lc", "cargo test"]"#).as_deref(),
            Some("cargo test")
        );
        assert_eq!(
            text(r#"["git", "commit", "-m", "it's done"]"#).as_deref(),
            Some(r"git commit -m 'it'\''s done'")
        );
        assert_eq!(text("[]"), None);
        assert_eq!(text("3"), None);
    }

    #[test]
    fn reads_tool_calls_from_codex_and_claude() {
        let update: ToolCallUpdate = serde_json::from_str(
            r#"{"toolCallId":"exec-1","_meta":{"terminal_output_delta":{"data":"1 notes.txt\n","terminal_id":"exec-1"}}}"#,
        )
        .unwrap();
        let update = ToolUpdate::from_update(&update);
        assert_eq!(update.output_delta.as_deref(), Some("1 notes.txt\n"));
        let exit: ToolCallUpdate = serde_json::from_str(
            r#"{"toolCallId":"exec-1","status":"completed","_meta":{"terminal_exit":{"exit_code":2,"signal":null}}}"#,
        )
        .unwrap();
        let exit = ToolUpdate::from_update(&exit);
        assert_eq!(exit.exit_code, Some(2));
        assert_eq!(exit.status, Some(ToolCallStatus::Completed));

        let claude: ToolCallUpdate = serde_json::from_str(
            r#"{"toolCallId":"t2","status":"completed","content":[{"type":"content","content":{"type":"text","text":"```console\na\nb\n```"}}]}"#,
        )
        .unwrap();
        assert_eq!(
            ToolUpdate::from_update(&claude).output.as_deref(),
            Some("a\nb")
        );

        let edit: ToolCall = serde_json::from_str(
            r#"{"toolCallId":"e1","title":"Editing files","kind":"edit","status":"in_progress","content":[{"type":"diff","oldText":"first\n","newText":"first\nsecond\nthird\n","path":"/w/notes.txt"}]}"#,
        )
        .unwrap();
        let edit = ToolUpdate::from_call(&edit);
        assert_eq!(edit.kind, Some(ToolKind::Edit));
        assert_eq!(
            edit.diffs,
            [FileDiff {
                path: "/w/notes.txt".into(),
                added: 2,
                removed: 0
            }]
        );
    }
}
