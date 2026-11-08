//! The agent: a coding agent (Claude, Codex, Gemini…) reached over the Agent Client Protocol,
//! which suggests a command for what failed or for what the user asks.
//!
//! Moth is the ACP client. It launches the agent as a subprocess (`agent` in the settings, or
//! the first known adapter on the shell's PATH) and keeps it running on its own thread while
//! it's in use. Every request is a new session, so nothing carries over from one to the next.
//! The agent gets no file system or terminal from Moth, every permission it asks for is turned
//! down, and its sessions run in an empty folder of Moth's: what it sees is the context in the
//! prompt, with secrets taken out (see `redact`). Its answer goes on the prompt, never runs.
//!
//! The answer has to come in a second or two, so by default each session runs on the agent's
//! lightest model without reasoning (Claude's thinking is turned off), or at its lowest effort
//! where reasoning can't be turned off. `agent_options` in the settings chooses the model, the
//! reasoning, the effort and any other option the agent announces instead (`configure`). The
//! UI shows what's in use next to the AI label.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, Agent, Client, ConnectionTo, Dispatch, SessionMessage,
    on_receive_request,
    schema::{
        ProtocolVersion,
        v1::{
            CancelNotification, ContentBlock, ContentChunk, Implementation, InitializeRequest,
            RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
            SessionConfigId, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
            SessionConfigOptionValue, SessionConfigSelect, SessionConfigSelectOption,
            SessionConfigSelectOptions, SessionId, SessionNotification, SessionUpdate,
            SetSessionConfigOptionRequest, StopReason,
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
use gpui::{App, Global, Task};

use crate::{
    config::{AgentKind, AgentOptionValue, AgentOptions, AgentSetting},
    fixes::Failure,
    redact,
};

/// The ACP adapters of Claude and Codex, run with npx at a version known to work: nothing to
/// install but Node, and npm keeps the download for next time.
const CLAUDE_ADAPTER: &str = "@agentclientprotocol/claude-agent-acp@0.99.1";
const CODEX_ADAPTER: &str = "@agentclientprotocol/codex-acp@2.5.0";
/// Adapters installed by hand, which run as they are, in the order they're looked for.
const INSTALLED_ADAPTERS: &[(&str, AgentKind)] = &[
    ("claude-agent-acp", AgentKind::Claude),
    ("claude-code-acp", AgentKind::Claude),
    ("codex-acp", AgentKind::Codex),
];

/// The agent stops after this long without a request; the next one starts it again.
const IDLE: Duration = Duration::from_secs(10 * 60);
/// What's sent of a failed command's output: its last lines, up to a size.
const OUTPUT_LINES: usize = 60;
const OUTPUT_BYTES: usize = 6_000;

#[derive(Clone, Debug, PartialEq)]
pub struct AgentCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// What the UI calls it until the agent says its own name.
    pub name: String,
    /// Which agent it is, when Moth knows: it sets its environment (see `tuned`).
    pub kind: Option<AgentKind>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    Found(AgentCommand),
    /// `"agent": false` in the settings.
    Off,
    /// What's missing to run the configured agent, or that no agent was found.
    NotFound(String),
}

impl AgentKind {
    fn label(self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude",
            AgentKind::Codex => "Codex",
            AgentKind::Gemini => "Gemini",
        }
    }

    /// The CLI the user logs in with, whose presence says they use this agent.
    fn cli(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Gemini => "gemini",
        }
    }

    /// What to run in a terminal to log in.
    fn login(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex login",
            AgentKind::Gemini => "gemini",
        }
    }
}

/// Which agent to launch: the one in the settings, or else an adapter installed by hand, or else
/// the agent of the first CLI found (`claude`, `codex`, `gemini`), all on `search_path` (the
/// shell's PATH, which the app doesn't have when launched from Finder).
pub fn resolve(setting: Option<&AgentSetting>, search_path: &str) -> Resolution {
    let result = match setting {
        Some(AgentSetting::Enabled(false)) => return Resolution::Off,
        Some(AgentSetting::Kind(kind)) => launch(*kind, search_path),
        Some(AgentSetting::Command(command)) => {
            match crate::process::which(&command.command, search_path) {
                Some(program) => Ok(AgentCommand {
                    name: display_name(&program),
                    kind: known_kind(&program),
                    program,
                    args: command.args.clone(),
                    env: command.env.clone(),
                }),
                None => Err(format!("{} isn't on the PATH", command.command)),
            }
        }
        Some(AgentSetting::Enabled(true)) | None => detect(search_path).ok_or_else(|| {
            "No agent found: set \"agent\" to \"claude\", \"codex\" or \"gemini\" in the settings"
                .to_string()
        }),
    };
    result.map_or_else(Resolution::NotFound, Resolution::Found)
}

fn detect(search_path: &str) -> Option<AgentCommand> {
    let installed = INSTALLED_ADAPTERS.iter().find_map(|(program, kind)| {
        Some(AgentCommand {
            program: crate::process::which(program, search_path)?,
            args: Vec::new(),
            env: BTreeMap::new(),
            name: kind.label().into(),
            kind: Some(*kind),
        })
    });
    installed.or_else(|| {
        [AgentKind::Claude, AgentKind::Codex, AgentKind::Gemini]
            .into_iter()
            .filter(|kind| crate::process::which(kind.cli(), search_path).is_some())
            .find_map(|kind| launch(kind, search_path).ok())
    })
}

/// How to run `kind`: its adapter through npx, or Gemini CLI, which speaks ACP itself.
fn launch(kind: AgentKind, search_path: &str) -> Result<AgentCommand, String> {
    let which = |program: &str| crate::process::which(program, search_path);
    let (program, args) = match kind {
        AgentKind::Claude | AgentKind::Codex => {
            let adapter = match kind {
                AgentKind::Claude => CLAUDE_ADAPTER,
                _ => CODEX_ADAPTER,
            };
            let npx = which("npx")
                .filter(|_| which("node").is_some())
                .ok_or_else(|| format!("{} needs Node.js (npx) on the PATH", kind.label()))?;
            (npx, vec!["-y".to_string(), adapter.to_string()])
        }
        AgentKind::Gemini => (
            which("gemini").ok_or("gemini isn't on the PATH: install Gemini CLI")?,
            vec!["--acp".to_string()],
        ),
    };
    Ok(AgentCommand {
        program,
        args,
        env: BTreeMap::new(),
        name: kind.label().into(),
        kind: Some(kind),
    })
}

fn display_name(program: &Path) -> String {
    known_kind(program).map_or_else(
        || crate::process::command_name(program).unwrap_or_default(),
        |kind| kind.label().to_string(),
    )
}

/// The agent a program is, from its name: an adapter or Gemini CLI.
fn known_kind(program: &Path) -> Option<AgentKind> {
    let file = crate::process::command_name(program)?;
    INSTALLED_ADAPTERS
        .iter()
        .map(|(adapter, kind)| (*adapter, *kind))
        .chain([("gemini", AgentKind::Gemini)])
        .find(|(known, _)| *known == file)
        .map(|(_, kind)| kind)
}

/// `command` with the environment that keeps the agent quick: for Claude, no thinking unless
/// `reasoning` is on; for Claude and Codex, none of the follow-up suggestions they'd write
/// after each answer. What the settings' own `env` says wins.
pub fn tuned(command: &AgentCommand, options: &AgentOptions) -> AgentCommand {
    let mut tuned = command.clone();
    let mut set = |key: &str, value: &str| {
        tuned
            .env
            .entry(key.to_string())
            .or_insert_with(|| value.to_string());
    };
    match command.kind {
        Some(AgentKind::Claude) => {
            set("CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION", "false");
            if !options.reasoning() {
                set("MAX_THINKING_TOKENS", "0");
            }
        }
        Some(AgentKind::Codex) => set("CODEX_ACP_PROMPT_SUGGESTIONS", "false"),
        _ => {}
    }
    tuned
}

/// The agent runs without thinking (`MAX_THINKING_TOKENS=0`, which Claude's adapter reads).
fn thinking_off(command: &AgentCommand) -> bool {
    command
        .env
        .get("MAX_THINKING_TOKENS")
        .is_some_and(|tokens| tokens.trim() == "0")
}

/// `text`, from a failed request, with how to log in when it's about that.
fn explain(name: &str, text: String) -> String {
    let lower = text.to_lowercase();
    let auth = [
        "auth",
        "not logged in",
        "/login",
        "log in",
        "sign in",
        "api key",
        "unauthorized",
        "credentials",
    ]
    .iter()
    .any(|signal| lower.contains(signal));
    let kind = [AgentKind::Claude, AgentKind::Codex, AgentKind::Gemini]
        .into_iter()
        .find(|kind| kind.label() == name);
    match kind {
        Some(kind) if auth => format!(
            "Log in once in a terminal with `{}`. ({text})",
            kind.login()
        ),
        _ => text,
    }
}

/// Where the agent's sessions run: an empty folder, so its own tools find nothing of the user's.
fn session_dir() -> PathBuf {
    let dir = crate::state::state_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("agent");
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// What arrives from a request, in order: the model answering (when Moth chose it), text as
/// it's written, then `Done` or `Failed`.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    /// The model and settings in use, like "Haiku 5.5 · no reasoning".
    Model(String),
    /// An option from the settings that couldn't be set, and why.
    Warning(String),
    Text(String),
    Done,
    Failed(String),
}

struct Request {
    prompt: String,
    cwd: PathBuf,
    /// `agent_options` from the settings.
    options: AgentOptions,
    updates: UnboundedSender<Update>,
    /// Resolves when the asker drops its end: the prompt turn is cancelled.
    cancel: oneshot::Receiver<()>,
}

/// The agent's process and connection, on a thread of their own.
struct Worker {
    command: AgentCommand,
    requests: UnboundedSender<Request>,
}

impl Worker {
    fn start(command: AgentCommand, search_path: String) -> std::io::Result<Self> {
        let (requests, receiver) = unbounded();
        let launch = command.clone();
        thread::Builder::new()
            .name("agent".into())
            .spawn(move || futures::executor::block_on(run(launch, search_path, receiver)))?;
        Ok(Self { command, requests })
    }
}

async fn run(command: AgentCommand, search_path: String, mut requests: UnboundedReceiver<Request>) {
    let mut config = AcpAgentConfig::new(&command.program)
        .args(command.args.clone())
        .envs(command.env.clone());
    // Agents written in Node start with `#!/usr/bin/env node`: they need the shell's PATH.
    if !command.env.contains_key("PATH") {
        config = config.env("PATH", search_path);
    }
    // The request being answered, told why if the connection fails during it.
    let current = std::sync::Mutex::new(None::<UnboundedSender<Update>>);
    let result = Client
        .builder()
        .name("moth")
        // Permission requests outside a session's prompt turn: turned down as well.
        .on_receive_request(
            async |_: RequestPermissionRequest, responder, _| {
                responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                ))
            },
            on_receive_request!(),
        )
        .connect_with(
            AcpAgent::new(config),
            async |connection: ConnectionTo<Agent>| {
                // The default capabilities: no file system and no terminal.
                connection
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
                            .client_info(Implementation::new("moth", env!("CARGO_PKG_VERSION"))),
                    )
                    .block_task()
                    .await?;
                while let Some(request) = requests.next().await {
                    let updates = request.updates.clone();
                    *current.lock().unwrap() = Some(updates.clone());
                    let thinking_off = thinking_off(&command);
                    if let Err(err) = prompt(&connection, request, thinking_off).await {
                        updates
                            .unbounded_send(Update::Failed(explain(&command.name, message(&err))))
                            .ok();
                    }
                    *current.lock().unwrap() = None;
                }
                Ok(())
            },
        )
        .await;
    if let Err(err) = result {
        crate::log::error(format!("agent {}: {err}", command.program.display()));
        // Whatever was waiting won't get an answer from this process.
        let failed = Update::Failed(explain(&command.name, message(&err)));
        if let Some(updates) = current.lock().unwrap().take() {
            updates.unbounded_send(failed).ok();
        }
        requests.close();
        while let Ok(request) = requests.try_recv() {
            request
                .updates
                .unbounded_send(Update::Failed(explain(&command.name, message(&err))))
                .ok();
        }
    }
}

/// One request: a new session, the prompt, and its answer streamed back.
async fn prompt(
    connection: &ConnectionTo<Agent>,
    request: Request,
    thinking_off: bool,
) -> agent_client_protocol::Result<()> {
    let Request {
        prompt,
        cwd,
        options: preferences,
        updates,
        mut cancel,
    } = request;
    connection
        .build_session(&cwd)
        .block_task()
        .run_until(async |mut session| {
            let session_id = session.session_id().clone();
            let to_agent = session.connection().clone();
            let options = session.config_options().unwrap_or_default().to_vec();
            let setup =
                configure(&to_agent, &session_id, options, &preferences, thinking_off).await;
            for warning in setup.warnings {
                updates.unbounded_send(Update::Warning(warning)).ok();
            }
            if !setup.labels.is_empty() {
                updates
                    .unbounded_send(Update::Model(setup.labels.join(" · ")))
                    .ok();
            }
            session.send_prompt(prompt)?;
            let mut cancelled = false;
            loop {
                let update = if cancelled {
                    session.read_update().await?
                } else {
                    let next = Box::pin(session.read_update());
                    match future::select(next, &mut cancel).await {
                        Either::Left((update, _)) => update?,
                        Either::Right(_) => {
                            // The agent still ends the turn, with `cancelled`, and the session
                            // has to be there to take it: without it the connection fails.
                            to_agent
                                .send_notification(CancelNotification::new(session_id.clone()))?;
                            cancelled = true;
                            continue;
                        }
                    }
                };
                match update {
                    SessionMessage::SessionMessage(dispatch) => {
                        MatchDispatch::new(dispatch)
                            .if_notification(async |notification: SessionNotification| {
                                if let SessionUpdate::AgentMessageChunk(ContentChunk {
                                    content: ContentBlock::Text(text),
                                    ..
                                }) = notification.update
                                {
                                    updates.unbounded_send(Update::Text(text.text)).ok();
                                }
                                Ok(())
                            })
                            .await
                            .if_request(async |_: RequestPermissionRequest, responder| {
                                responder.respond(RequestPermissionResponse::new(
                                    RequestPermissionOutcome::Cancelled,
                                ))
                            })
                            .await
                            .otherwise(async |dispatch| match dispatch {
                                // Moth offers no other methods (files, terminals).
                                Dispatch::Request(_, responder) => responder.respond_with_error(
                                    agent_client_protocol::Error::method_not_found(),
                                ),
                                _ => Ok(()),
                            })
                            .await?;
                    }
                    SessionMessage::StopReason(reason) => {
                        let update = match reason {
                            // Moth cancelled it: nobody is waiting for the answer.
                            StopReason::Cancelled if cancelled => Update::Done,
                            StopReason::Cancelled => {
                                Update::Failed("The agent stopped before answering.".into())
                            }
                            StopReason::Refusal => {
                                Update::Failed("The agent declined to answer.".into())
                            }
                            _ => Update::Done,
                        };
                        updates.unbounded_send(update).ok();
                        return Ok(());
                    }
                    _ => {}
                }
            }
        })
        .await
}

/// Words that describe a light, quick model, and a large, slow one.
const LIGHT: &[&str] = &[
    "haiku",
    "flash",
    "lite",
    "mini",
    "nano",
    "small",
    "fast",
    "faster",
    "fastest",
    "quick",
    "quickest",
    "instant",
    "luna",
    "affordable",
    "efficient",
    "cheap",
    "cheapest",
    "light",
    "lighter",
    "lightweight",
];
const HEAVY: &[&str] = &[
    "opus",
    "frontier",
    "capable",
    "hardest",
    "complex",
    "demanding",
    "pro",
    "max",
    "ultra",
    "deep",
];
const OLD: &[&str] = &["older", "previous", "legacy", "deprecated"];

/// What `configure` did: what's in use, for the UI, and what couldn't be set.
#[derive(Debug, Default, PartialEq)]
struct Setup {
    labels: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Clone, Debug)]
enum Step {
    Model,
    Effort,
    /// Another option, by its id.
    Other(String),
}

/// Sets up a session as `agent_options` says, one option at a time, since each can change the
/// others (a model has its own effort levels): the model, then the effort, then the rest. What
/// the settings leave to Moth is chosen for speed: the lightest model, and no reasoning or the
/// lowest effort. An option that can't be set is left as it was and reported.
async fn configure(
    connection: &ConnectionTo<Agent>,
    session: &SessionId,
    mut options: Vec<SessionConfigOption>,
    preferences: &AgentOptions,
    thinking_off: bool,
) -> Setup {
    let mut setup = Setup::default();
    let steps = [Step::Model, Step::Effort]
        .into_iter()
        .chain(preferences.other.keys().cloned().map(Step::Other));
    for step in steps {
        if matches!(step, Step::Effort) && thinking_off {
            setup.labels.push("no reasoning".into());
        }
        let choice = match choose(&step, &options, preferences, thinking_off) {
            Ok(Some(choice)) => choice,
            Ok(None) => continue,
            Err(warning) => {
                crate::log::error(format!("agent: {warning}"));
                setup.warnings.push(warning);
                continue;
            }
        };
        if !choice.current {
            let request =
                SetSessionConfigOptionRequest::new(session.clone(), choice.config, choice.value);
            match connection.send_request(request).block_task().await {
                Ok(response) => options = response.config_options,
                Err(err) => {
                    let warning = format!("Couldn't set {}: {}", choice.name, message(&err));
                    crate::log::error(format!("agent: {warning}"));
                    setup.warnings.push(warning);
                    continue;
                }
            }
        }
        setup.labels.push(choice.label);
    }
    setup
}

/// What to set for `step`: what the settings ask for, or else the quickest; `Err` says why
/// what they ask for can't be set.
fn choose(
    step: &Step,
    options: &[SessionConfigOption],
    preferences: &AgentOptions,
    thinking_off: bool,
) -> Result<Option<Choice>, String> {
    let wanted = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty() && !v.eq_ignore_ascii_case("auto"))
            .map(|v| AgentOptionValue::Text(v.to_string()))
    };
    match step {
        Step::Model => match wanted(&preferences.model) {
            None => Ok(lightest_model(options)),
            Some(want) => {
                let option = model_option(options)
                    .ok_or("The agent has no models to choose from, so \"model\" is ignored")?;
                set_to(option, &want).map(Some)
            }
        },
        Step::Effort => match wanted(&preferences.effort) {
            None if thinking_off => Ok(None),
            None => Ok(lowest_effort(options, preferences.reasoning())),
            Some(want) => {
                let option = effort_option(options).ok_or(
                    "The model has no reasoning effort to choose, so \"effort\" is ignored",
                )?;
                set_to(option, &want).map(Some)
            }
        },
        Step::Other(id) => {
            let option = options
                .iter()
                .find(|o| o.id.0.as_ref() == id)
                .ok_or_else(|| format!("The agent has no option \"{id}\""))?;
            if option.category == Some(SessionConfigOptionCategory::Mode) {
                return Err(format!(
                    "\"{id}\" sets what the agent may do, which Moth keeps to nothing"
                ));
            }
            set_to(option, &preferences.other[id]).map(Some)
        }
    }
}

fn model_option(options: &[SessionConfigOption]) -> Option<&SessionConfigOption> {
    options.iter().find(|o| {
        o.category == Some(SessionConfigOptionCategory::Model)
            || (o.category.is_none() && o.id.0.as_ref() == "model")
    })
}

fn effort_option(options: &[SessionConfigOption]) -> Option<&SessionConfigOption> {
    options.iter().find(|o| {
        o.category == Some(SessionConfigOptionCategory::ThoughtLevel)
            || (o.category.is_none()
                && words(&o.id.0)
                    .iter()
                    .any(|w| matches!(w.as_str(), "effort" | "reasoning" | "thinking")))
    })
}

/// `option` set to `want`: one of its values, by id or name (any case), or on and off.
fn set_to(option: &SessionConfigOption, want: &AgentOptionValue) -> Result<Choice, String> {
    let switch = |value: &str| match value.trim().to_lowercase().as_str() {
        "on" | "true" | "enabled" | "yes" => Some(true),
        "off" | "false" | "disabled" | "no" => Some(false),
        _ => None,
    };
    let want_switch = match want {
        AgentOptionValue::Bool(on) => Some(*on),
        AgentOptionValue::Text(text) => switch(text),
    };
    let shown = match want {
        AgentOptionValue::Bool(on) => on.to_string(),
        AgentOptionValue::Text(text) => text.clone(),
    };
    match &option.kind {
        SessionConfigKind::Boolean(boolean) => {
            let on = want_switch
                .ok_or_else(|| format!("{} is on or off, not \"{shown}\"", option.name))?;
            Ok(Choice {
                config: option.id.clone(),
                value: SessionConfigOptionValue::boolean(on),
                name: option.name.clone(),
                label: format!(
                    "{} {}",
                    option.name.to_lowercase(),
                    if on { "on" } else { "off" }
                ),
                current: boolean.current_value == on,
            })
        }
        SessionConfigKind::Select(select) => {
            let values = values(select);
            let found = values.iter().find(|value| match want {
                AgentOptionValue::Text(text) => {
                    value.value.0.eq_ignore_ascii_case(text.trim())
                        || value.name.eq_ignore_ascii_case(text.trim())
                }
                AgentOptionValue::Bool(_) => false,
            });
            let found = found.or_else(|| {
                let on = want_switch?;
                values
                    .iter()
                    .find(|value| switch(&value.value.0) == Some(on))
            });
            let value = found.ok_or_else(|| {
                let offered: Vec<&str> = values.iter().map(|v| v.value.0.as_ref()).collect();
                format!(
                    "{} \"{shown}\" isn't offered; there's {}",
                    option.name,
                    offered.join(", ")
                )
            })?;
            Ok(choice(option, select, value))
        }
        _ => Err(format!("Moth can't set {}", option.name)),
    }
}

/// `value` of a select option, with how the UI says it.
fn choice(
    option: &SessionConfigOption,
    select: &SessionConfigSelect,
    value: &SessionConfigSelectOption,
) -> Choice {
    let label = if option.category == Some(SessionConfigOptionCategory::Model) {
        value.name.clone()
    } else if effort_option(std::slice::from_ref(option)).is_some() {
        if effort_rank(value) == Some(0) {
            "no reasoning".into()
        } else {
            format!("{} effort", value.name.to_lowercase())
        }
    } else {
        format!(
            "{} {}",
            option.name.to_lowercase(),
            value.name.to_lowercase()
        )
    };
    Choice {
        config: option.id.clone(),
        value: SessionConfigOptionValue::value_id(value.value.clone()),
        name: option.name.clone(),
        label,
        current: select.current_value == value.value,
    }
}

/// A value to set: which option, to what, and how the UI says it.
#[derive(Debug, PartialEq)]
struct Choice {
    config: SessionConfigId,
    value: SessionConfigOptionValue,
    /// The option's name, for the log.
    name: String,
    label: String,
    /// It's already the value.
    current: bool,
}

/// The model the agent describes as lightest and quickest (Haiku, Flash Lite, mini…), if one
/// reads that way.
fn lightest_model(options: &[SessionConfigOption]) -> Option<Choice> {
    let option = model_option(options)?;
    let select = as_select(option)?;
    let mut best: Option<(i32, &SessionConfigSelectOption)> = None;
    for value in values(select) {
        if matches!(value.value.0.as_ref(), "default" | "auto") {
            continue;
        }
        let score = lightness(value);
        if best.is_none_or(|(top, _)| score > top) {
            best = Some((score, value));
        }
    }
    let (_, value) = best.filter(|(score, _)| *score > 0)?;
    Some(choice(option, select, value))
}

/// How light a model reads from its id, name and description: quick and small words count for
/// it, large and old ones against it.
fn lightness(value: &SessionConfigSelectOption) -> i32 {
    let text = format!(
        "{} {} {}",
        value.value.0,
        value.name,
        value.description.as_deref().unwrap_or_default()
    );
    words(&text)
        .iter()
        .map(|word| {
            if LIGHT.contains(&word.as_str()) {
                2
            } else if HEAVY.contains(&word.as_str()) {
                -2
            } else if OLD.contains(&word.as_str()) {
                -1
            } else {
                0
            }
        })
        .sum()
}

/// The lowest reasoning effort: none (when `reasoning` is off), then minimal, then low.
fn lowest_effort(options: &[SessionConfigOption], reasoning: bool) -> Option<Choice> {
    let option = effort_option(options)?;
    let select = as_select(option)?;
    let value = values(select)
        .into_iter()
        .filter_map(|value| Some((effort_rank(value)?, value)))
        .filter(|(rank, _)| !reasoning || *rank > 0)
        .min_by_key(|(rank, _)| *rank)?
        .1;
    Some(choice(option, select, value))
}

/// How little reasoning an effort level means, for the levels that mean little.
fn effort_rank(value: &SessionConfigSelectOption) -> Option<u8> {
    match value.value.0.to_lowercase().as_str() {
        "none" | "off" | "disabled" | "instant" => Some(0),
        "minimal" | "min" | "lowest" => Some(1),
        "low" => Some(2),
        _ => None,
    }
}

fn as_select(option: &SessionConfigOption) -> Option<&SessionConfigSelect> {
    match &option.kind {
        SessionConfigKind::Select(select) => Some(select),
        _ => None,
    }
}

fn values(select: &SessionConfigSelect) -> Vec<&SessionConfigSelectOption> {
    match &select.options {
        SessionConfigSelectOptions::Ungrouped(values) => values.iter().collect(),
        SessionConfigSelectOptions::Grouped(groups) => {
            groups.iter().flat_map(|group| &group.options).collect()
        }
        _ => Vec::new(),
    }
}

/// The distinct words of `text`, lowercase.
fn words(text: &str) -> std::collections::BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The error in a line or two, with what the agent printed on stderr if that's all there is.
fn message(err: &agent_client_protocol::Error) -> String {
    let text = err.to_string();
    let text = text.trim();
    let text = text
        .strip_prefix("Internal error: ")
        .unwrap_or(text)
        .trim_matches('"');
    let mut lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines.truncate(3);
    let joined = lines.join(" · ");
    if joined.chars().count() > 300 {
        joined.chars().take(300).collect::<String>() + "…"
    } else {
        joined
    }
}

/// The running agent, shared by every window.
#[derive(Default)]
struct Agents {
    worker: Option<Worker>,
    idle: Option<Task<()>>,
    /// The last `resolve`, for the setting and PATH it was made with.
    resolved: Option<Resolved>,
}

struct Resolved {
    setting: Option<AgentSetting>,
    search_path: String,
    resolution: Resolution,
    at: Instant,
}

/// `resolve` for the current settings, remembered: the UI asks on every frame. An agent that
/// wasn't found is looked for again after a while, in case it was installed since.
pub fn find(search_path: &str, cx: &mut App) -> Resolution {
    let setting = crate::config::get(cx).agent.clone();
    let agents = cx.default_global::<Agents>();
    if let Some(resolved) = &agents.resolved
        && resolved.setting == setting
        && resolved.search_path == search_path
        && (matches!(resolved.resolution, Resolution::Found(_) | Resolution::Off)
            || resolved.at.elapsed() < Duration::from_secs(30))
    {
        return resolved.resolution.clone();
    }
    let resolution = resolve(setting.as_ref(), search_path);
    agents.resolved = Some(Resolved {
        setting,
        search_path: search_path.to_string(),
        resolution: resolution.clone(),
        at: Instant::now(),
    });
    resolution
}

impl Global for Agents {}

/// A request in flight. Dropping it cancels the prompt turn.
///
/// Its fields are private so it's only read through [`Pending::next`]: a task that awaited
/// `pending.updates` directly would capture just that field (closures capture the fields they
/// use), and the cancel sender, dropped with the rest right away, would cancel every turn
/// before the agent answered.
pub struct Pending {
    updates: UnboundedReceiver<Update>,
    _cancel: oneshot::Sender<()>,
}

impl Pending {
    /// The next update, `None` once the request is over.
    pub async fn next(&mut self) -> Option<Update> {
        self.updates.next().await
    }
}

/// Sends `prompt` to the agent (starting it if needed) and returns its answer as it arrives,
/// with the model and settings of `agent_options`.
pub fn ask(command: &AgentCommand, search_path: &str, prompt: String, cx: &mut App) -> Pending {
    let options = crate::config::get(cx).agent_options.clone();
    // A change of `reasoning` changes the environment: the agent is started again.
    let command = &tuned(command, &options);
    let (updates_tx, updates) = unbounded();
    let (cancel_tx, cancel) = oneshot::channel();
    let pending = Pending {
        updates,
        _cancel: cancel_tx,
    };
    let agents = cx.default_global::<Agents>();
    let reusable = agents
        .worker
        .as_ref()
        .is_some_and(|w| w.command == *command && !w.requests.is_closed());
    if !reusable {
        agents.worker = match Worker::start(command.clone(), search_path.to_string()) {
            Ok(worker) => Some(worker),
            Err(err) => {
                updates_tx
                    .unbounded_send(Update::Failed(format!("Couldn't start the agent: {err}")))
                    .ok();
                return pending;
            }
        };
    }
    let request = Request {
        prompt,
        cwd: session_dir(),
        options,
        updates: updates_tx,
        cancel,
    };
    if let Some(worker) = &agents.worker
        && let Err(err) = worker.requests.unbounded_send(request)
    {
        err.into_inner()
            .updates
            .unbounded_send(Update::Failed("The agent stopped".into()))
            .ok();
    }
    // Dropping the worker closes its queue: the thread finishes and the process is stopped.
    let idle = cx.background_executor().timer(IDLE);
    let task = cx.spawn(async move |cx| {
        idle.await;
        cx.update(|cx| {
            let agents = cx.default_global::<Agents>();
            agents.worker = None;
            agents.idle = None;
        });
    });
    cx.default_global::<Agents>().idle = Some(task);
    pending
}

/// What the prompt says about where the command runs.
#[derive(Clone, Debug, Default)]
pub struct Environment {
    /// The current folder, with the home folder as `~`.
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub shell: String,
    /// `user@host` when the command runs over SSH.
    pub remote: Option<String>,
}

impl Environment {
    fn describe(&self) -> String {
        let mut lines = Vec::new();
        match &self.remote {
            Some(host) => lines.push(format!(
                "The shell is on the remote host {host}, reached over SSH (the OS there is unknown)."
            )),
            None => lines.push(format!("Shell: {} on {}", self.shell, os_name())),
        }
        if let Some(cwd) = &self.cwd {
            lines.push(format!("Current folder: {cwd}"));
        }
        if let Some(branch) = &self.branch {
            lines.push(format!("Git branch: {branch}"));
        }
        lines.join("\n")
    }
}

fn os_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(windows) {
        "Windows"
    } else {
        "Linux"
    }
}

/// `path` with the home folder written as `~`.
pub fn tilde(path: &Path) -> String {
    if let Some(home) = crate::state::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".into()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.display().to_string()
}

/// A prompt ready to send: the instructions plus the context, with secrets taken out.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub text: String,
    /// The context part, as sent (shown under "What was sent").
    pub context: String,
    pub redactions: usize,
    /// The command prints secrets, so its output was left out.
    pub withheld_output: bool,
}

const RULES: &str = "Rules:
- Reply with at most two short sentences, then the command alone in a ```sh code block.
- The command must be one line, for the shell described below. The user reviews it and runs it themselves.
- Don't use tools, read files or run commands: everything you need is below.
- Values shown as [REDACTED] were removed for privacy. Never guess them; if the command needs one, keep the placeholder.
- If no command would do it, say why in one sentence and leave out the code block.";

/// A fix for the command that failed.
pub fn fix_prompt(environment: &Environment, failure: &Failure) -> Prompt {
    let withheld_output = redact::sensitive_command(&failure.command);
    let mut output_redactions = 0;
    let output = if withheld_output {
        "(left out: this command prints secrets)".to_string()
    } else if failure.output.trim().is_empty() {
        "(no output)".to_string()
    } else {
        // Redacted before it's cut: a cut through a private key would leave its body without
        // the BEGIN line that identifies it.
        let redacted = redact::redact(&failure.output);
        output_redactions = redacted.count;
        tail(&redacted.text)
    };
    let exit = failure
        .exit_code
        .map_or("unknown".to_string(), |code| code.to_string());
    let context = format!(
        "{}\nCommand: {}\nExit code: {exit}\nOutput (last lines):\n<output>\n{output}\n</output>",
        environment.describe(),
        failure.command,
    );
    let instructions = "You are the command assistant in Moth, a terminal app. A shell command just failed. Suggest the command the user should run instead to do what they meant.";
    build(instructions, &context, withheld_output, output_redactions)
}

/// A command for what the user describes.
pub fn ask_prompt(
    environment: &Environment,
    request: &str,
    files: &[String],
    last: Option<&Failure>,
) -> Prompt {
    let mut context = environment.describe();
    if !files.is_empty() {
        context.push_str(&format!("\nIn this folder: {}", files.join(", ")));
    }
    if let Some(last) = last {
        let exit = last
            .exit_code
            .map_or(String::new(), |code| format!(" (exit code {code})"));
        context.push_str(&format!("\nPrevious command: {}{exit}", last.command));
    }
    context.push_str(&format!("\nThe user asks for: {}", request.trim()));
    let instructions = "You are the command assistant in Moth, a terminal app. Write the shell command that does what the user asks.";
    build(instructions, &context, false, 0)
}

/// `redactions`: secrets already taken out of what's in `context`.
fn build(instructions: &str, context: &str, withheld_output: bool, redactions: usize) -> Prompt {
    let redacted = redact::redact(context);
    Prompt {
        text: format!("{instructions}\n\n{RULES}\n\n{}", redacted.text),
        context: redacted.text,
        redactions: redactions + redacted.count,
        withheld_output,
    }
}

/// The last lines of the output, within the size limit.
fn tail(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let start = lines.len().saturating_sub(OUTPUT_LINES);
    let mut text = lines[start..].join("\n");
    if text.len() > OUTPUT_BYTES {
        let mut cut = text.len() - OUTPUT_BYTES;
        while !text.is_char_boundary(cut) {
            cut += 1;
        }
        text = format!("…{}", &text[cut..]);
    }
    text
}

/// The agent's answer: what it says and the command in its code block.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reply {
    pub explanation: String,
    pub command: Option<String>,
}

pub fn parse_reply(text: &str) -> Reply {
    let mut explanation = Vec::new();
    let mut block: Option<Vec<&str>> = None;
    let mut command_lines: Option<Vec<&str>> = None;
    // A lone `\r` ends a line too: typed at the prompt, it would run what comes before it.
    for line in text.split(['\n', '\r']) {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            match block.take() {
                Some(lines) => {
                    command_lines.get_or_insert(lines);
                }
                None => block = Some(Vec::new()),
            }
            continue;
        }
        match &mut block {
            Some(lines) => lines.push(line),
            None => explanation.push(trimmed),
        }
    }
    // An unclosed block (the answer was cut off) still counts.
    if let Some(lines) = block {
        command_lines.get_or_insert(lines);
    }
    let command = command_lines.and_then(|lines| join_command(&lines));
    let explanation = explanation
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Reply {
        explanation,
        command,
    }
}

/// The lines of a code block as one command: prompts (`$ `) and comments out, continuations
/// (`\`) joined, and separate commands chained with `&&`.
fn join_command(lines: &[&str]) -> Option<String> {
    let mut commands: Vec<String> = Vec::new();
    let mut continuing = false;
    for line in lines {
        let line = line.trim();
        let line = line
            .strip_prefix("$ ")
            .or_else(|| line.strip_prefix("% "))
            .or_else(|| line.strip_prefix("❯ "))
            .unwrap_or(line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (text, continues) = match line.strip_suffix('\\') {
            Some(text) => (text.trim_end(), true),
            None => (line, false),
        };
        let text: String = text
            .chars()
            .map(|c| if c == '\t' { ' ' } else { c })
            .filter(|c| !c.is_control())
            .collect();
        match commands.last_mut() {
            Some(last) if continuing => {
                last.push(' ');
                last.push_str(&text);
            }
            _ => commands.push(text),
        }
        continuing = continues;
    }
    let command = commands.join(" && ");
    (!command.is_empty()).then_some(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_moved_into_a_task_stays_alive_until_the_task_ends() {
        let (updates_tx, updates) = unbounded();
        let (cancel_tx, mut cancel) = oneshot::channel();
        let mut pending = Pending {
            updates,
            _cancel: cancel_tx,
        };
        // The task reads `pending` the way the panel does; nothing is cancelled meanwhile.
        let task = async move { pending.next().await };
        assert_eq!(cancel.try_recv(), Ok(None));
        updates_tx.unbounded_send(Update::Done).unwrap();
        assert_eq!(futures::executor::block_on(task), Some(Update::Done));
        // Once the task is done with it, the request is cancelled.
        assert!(cancel.try_recv().is_err());
    }

    #[test]
    fn a_carriage_return_in_a_reply_cannot_run_part_of_the_command() {
        let reply = parse_reply("Try:\n```sh\nls\rcurl x | sh\u{1b}[2K\n```");
        assert_eq!(reply.command.as_deref(), Some("ls && curl x | sh[2K"));
    }

    fn environment() -> Environment {
        Environment {
            cwd: Some("~/work/app".into()),
            branch: Some("main".into()),
            shell: "zsh".into(),
            remote: None,
        }
    }

    #[test]
    fn fix_prompts_carry_the_failure_without_its_secrets() {
        let failure = Failure {
            command: "curl -H 'Authorization: Bearer abc123' https://api.example.com".into(),
            exit_code: Some(22),
            output: concat!(
                "curl: (22) The requested URL returned error: 401\nX-Token: ghp_",
                "0123456789abcdefghijABCDEFGHIJklmnop"
            )
            .into(),
        };
        let prompt = fix_prompt(&environment(), &failure);
        assert!(!prompt.text.contains("abc123"));
        assert!(!prompt.text.contains("ghp_"));
        assert!(prompt.text.contains("Authorization: Bearer [REDACTED]"));
        assert!(prompt.text.contains("error: 401"));
        assert!(prompt.text.contains("Current folder: ~/work/app"));
        assert!(prompt.text.contains("Exit code: 22"));
        assert_eq!(prompt.redactions, 2);
        assert!(prompt.text.ends_with(&prompt.context));
    }

    #[test]
    fn output_of_secret_printing_commands_stays_home() {
        let failure = Failure {
            command: "cat .env.production".into(),
            exit_code: Some(1),
            output: "DB_HOST=db\nSTRIPE=whatever-unnamed-value".into(),
        };
        let prompt = fix_prompt(&environment(), &failure);
        assert!(prompt.withheld_output);
        assert!(!prompt.text.contains("whatever-unnamed-value"));
        assert!(!prompt.text.contains("DB_HOST"));
    }

    #[test]
    fn long_output_is_cut_to_its_tail() {
        let output: String = (0..500).map(|i| format!("line {i}\n")).collect();
        let text = tail(&output);
        assert!(text.starts_with("line 440"));
        assert!(text.ends_with("line 499"));
        let wide = "é".repeat(OUTPUT_BYTES);
        assert!(tail(&wide).len() <= OUTPUT_BYTES + "…".len());
    }

    #[test]
    fn ask_prompts_redact_what_the_user_typed() {
        let prompt = ask_prompt(
            &environment(),
            concat!(
                "log in with token sk-ant-",
                "api03-abcdefghijklmnopqrstuvwxyz012345"
            ),
            &["Cargo.toml".into(), "src".into()],
            None,
        );
        assert!(!prompt.text.contains("sk-ant"));
        assert!(prompt.text.contains("In this folder: Cargo.toml, src"));
        assert_eq!(prompt.redactions, 1);
    }

    #[test]
    fn replies_give_the_explanation_and_one_command() {
        let reply = parse_reply(
            "The branch has no upstream.\n\n```sh\n$ git push --set-upstream origin main\n```\n",
        );
        assert_eq!(reply.explanation, "The branch has no upstream.");
        assert_eq!(
            reply.command.as_deref(),
            Some("git push --set-upstream origin main")
        );

        let multi = parse_reply(
            "Two steps:\n```bash\n# build first\ncargo build \\\n  --release\ncargo test\n```",
        );
        assert_eq!(
            multi.command.as_deref(),
            Some("cargo build --release && cargo test")
        );

        let none = parse_reply("That needs root access, which can't be fixed from here.");
        assert_eq!(none.command, None);

        // Cut off before the closing fence.
        assert_eq!(
            parse_reply("Try:\n```\nls -la").command.as_deref(),
            Some("ls -la")
        );
    }

    fn options(json: &str) -> Vec<SessionConfigOption> {
        serde_json::from_str(json).unwrap()
    }

    /// What claude-agent-acp offers (trimmed), with Opus as its default.
    const CLAUDE_OPTIONS: &str = r#"[
        {"id":"mode","name":"Mode","category":"mode","type":"select","currentValue":"default",
         "options":[{"value":"default","name":"Manual"},{"value":"plan","name":"Plan"}]},
        {"id":"model","name":"Model","category":"model","type":"select","currentValue":"default",
         "options":[
            {"value":"default","name":"Default (recommended)","description":"Opus"},
            {"value":"opus","name":"Opus 5.5","description":"Opus 5.5 · Best for everyday, complex tasks"},
            {"value":"claude-fable-5-1[1m]","name":"Fable 5.1","description":"Most capable for your hardest and longest-running tasks"},
            {"value":"sonnet","name":"Sonnet 5.5","description":"Sonnet 5.5 · Efficient for routine tasks"},
            {"value":"haiku","name":"Haiku 5.5","description":"Haiku 5.5 · Fastest for quick answers"}]},
        {"id":"effort","name":"Effort","category":"thought_level","type":"select","currentValue":"default",
         "options":[{"value":"default","name":"Default"},{"value":"low","name":"Low"},{"value":"high","name":"High"},{"value":"max","name":"Max"}]},
        {"id":"fast","name":"Fast mode","category":"model_config","type":"select","currentValue":"off",
         "options":[{"value":"on","name":"On"},{"value":"off","name":"Off"}]}
    ]"#;

    /// What codex-acp offers (trimmed).
    const CODEX_OPTIONS: &str = r#"[
        {"id":"mode","name":"Mode","category":"mode","type":"select","currentValue":"agent",
         "options":[{"value":"read-only","name":"Read Only"},{"value":"agent","name":"Agent"}]},
        {"id":"model","name":"Model","category":"model","type":"select","currentValue":"gpt-6.1-sol",
         "options":[
            {"value":"gpt-6.1-sol","name":"6.1 Sol","description":"Latest workhorse model for coding and everyday work."},
            {"value":"gpt-6-astra","name":"6 Astra","description":"Frontier intelligence for the most demanding work."},
            {"value":"gpt-6-luna","name":"6 Luna","description":"Fast and affordable model for easier tasks."},
            {"value":"gpt-5.6-luna","name":"5.6 Luna","description":"Older fast and efficient model."}]},
        {"id":"reasoning_effort","name":"Reasoning effort","category":"thought_level","type":"select","currentValue":"low",
         "options":[{"value":"low","name":"Low"},{"value":"medium","name":"Medium"},{"value":"ultra","name":"Ultra"}]},
        {"id":"fast-mode","name":"Fast mode","category":"model_config","type":"select","currentValue":"off",
         "options":[{"value":"off","name":"Off"},{"value":"on","name":"On"}]}
    ]"#;

    fn chosen(choice: Option<Choice>) -> Option<(String, String, bool)> {
        choice.map(|c| {
            let value = match c.value {
                SessionConfigOptionValue::ValueId { value } => value.0.to_string(),
                other => format!("{other:?}"),
            };
            (c.config.0.to_string(), value, c.current)
        })
    }

    #[test]
    fn picks_the_lightest_model_and_lowest_effort() {
        let claude = options(CLAUDE_OPTIONS);
        assert_eq!(
            chosen(lightest_model(&claude)),
            Some(("model".into(), "haiku".into(), false))
        );
        assert_eq!(lightest_model(&claude).unwrap().label, "Haiku 5.5");
        assert_eq!(
            chosen(lowest_effort(&claude, false)),
            Some(("effort".into(), "low".into(), false))
        );
        assert_eq!(lowest_effort(&claude, false).unwrap().label, "low effort");

        let codex = options(CODEX_OPTIONS);
        assert_eq!(
            chosen(lightest_model(&codex)),
            Some(("model".into(), "gpt-6-luna".into(), false))
        );
        // Already at its lowest.
        assert_eq!(
            chosen(lowest_effort(&codex, false)),
            Some(("reasoning_effort".into(), "low".into(), true))
        );

        // An effort of none is no reasoning, skipped when reasoning is on.
        let none = options(
            r#"[{"id":"effort","name":"Effort","category":"thought_level","type":"select","currentValue":"medium",
                 "options":[{"value":"none","name":"None"},{"value":"low","name":"Low"},{"value":"medium","name":"Medium"}]}]"#,
        );
        let off = lowest_effort(&none, false).unwrap();
        assert_eq!(off.label, "no reasoning");
        assert_eq!(chosen(Some(off)).unwrap().1, "none");
        assert_eq!(
            chosen(lowest_effort(&none, true)).unwrap().1,
            "low".to_string()
        );
    }

    #[test]
    fn leaves_models_alone_when_none_reads_as_light() {
        let opaque = options(
            r#"[{"id":"model","name":"Model","category":"model","type":"select","currentValue":"a",
                 "options":[{"value":"a","name":"Model A"},{"value":"b","name":"Model B"}]},
                {"id":"thinking","name":"Thinking","type":"boolean","currentValue":true}]"#,
        );
        assert_eq!(lightest_model(&opaque), None);
        assert_eq!(lowest_effort(&opaque, false), None);
        // Grouped values, and the flash-lite of a family beating its flash.
        let gemini = options(
            r#"[{"id":"model","name":"Model","category":"model","type":"select","currentValue":"gemini-pro",
                 "options":[{"group":"g","name":"Gemini","options":[
                    {"value":"gemini-pro","name":"Gemini Pro"},
                    {"value":"gemini-flash","name":"Gemini Flash"},
                    {"value":"gemini-flash-lite","name":"Gemini Flash Lite"}]}]}]"#,
        );
        assert_eq!(
            chosen(lightest_model(&gemini)),
            Some(("model".into(), "gemini-flash-lite".into(), false))
        );
    }

    fn preferences(json: &str) -> AgentOptions {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn the_settings_choose_the_model_and_its_options() {
        let claude = options(CLAUDE_OPTIONS);
        let codex = options(CODEX_OPTIONS);
        let auto = AgentOptions::default();
        // Nothing set: the lightest model; Claude without thinking needs no effort.
        assert_eq!(
            chosen(choose(&Step::Model, &claude, &auto, true).unwrap())
                .unwrap()
                .1,
            "haiku"
        );
        assert_eq!(choose(&Step::Effort, &claude, &auto, true), Ok(None));
        assert_eq!(
            chosen(choose(&Step::Effort, &codex, &auto, false).unwrap())
                .unwrap()
                .1,
            "low"
        );

        // By id or by name, in any case.
        let chosen_by = |json: &str, step: Step, options: &[SessionConfigOption]| {
            choose(&step, options, &preferences(json), false).map(|c| {
                let label = c.as_ref().map(|c| c.label.clone());
                (chosen(c).map(|c| c.1), label)
            })
        };
        assert_eq!(
            chosen_by(r#"{"model": "Sonnet"}"#, Step::Model, &claude),
            Ok((Some("sonnet".into()), Some("Sonnet 5.5".into())))
        );
        assert_eq!(
            chosen_by(r#"{"model": "6.1 sol"}"#, Step::Model, &codex),
            Ok((Some("gpt-6.1-sol".into()), Some("6.1 Sol".into())))
        );
        assert_eq!(
            chosen_by(r#"{"model": "auto"}"#, Step::Model, &codex)
                .unwrap()
                .0,
            Some("gpt-6-luna".into())
        );
        assert_eq!(
            chosen_by(r#"{"effort": "high"}"#, Step::Effort, &claude),
            Ok((Some("high".into()), Some("high effort".into())))
        );
        // Other options by id, true and false meaning on and off.
        assert_eq!(
            chosen_by(
                r#"{"fast-mode": true}"#,
                Step::Other("fast-mode".into()),
                &codex
            ),
            Ok((Some("on".into()), Some("fast mode on".into())))
        );

        // What can't be set says why.
        let unknown = chosen_by(r#"{"model": "gpt-9"}"#, Step::Model, &codex).unwrap_err();
        assert!(unknown.contains("\"gpt-9\" isn't offered"), "{unknown}");
        assert!(unknown.contains("gpt-6-luna"), "{unknown}");
        assert!(
            chosen_by(r#"{"turbo": "on"}"#, Step::Other("turbo".into()), &claude)
                .unwrap_err()
                .contains("no option \"turbo\"")
        );
        // Never the permissions mode.
        assert!(
            chosen_by(
                r#"{"mode": "bypassPermissions"}"#,
                Step::Other("mode".into()),
                &claude
            )
            .unwrap_err()
            .contains("keeps to nothing")
        );
    }

    #[test]
    fn claude_runs_without_thinking_unless_reasoning_is_on() {
        let claude = AgentCommand {
            program: "npx".into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            name: "Claude".into(),
            kind: Some(AgentKind::Claude),
        };
        let quick = tuned(&claude, &AgentOptions::default());
        assert!(thinking_off(&quick));
        assert_eq!(quick.env["CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION"], "false");
        let reasoning = tuned(&claude, &preferences(r#"{"reasoning": true}"#));
        assert!(!thinking_off(&reasoning));
        // The settings' own env wins.
        let mut own = claude.clone();
        own.env.insert("MAX_THINKING_TOKENS".into(), "4000".into());
        assert!(!thinking_off(&tuned(&own, &AgentOptions::default())));
        let codex = AgentCommand {
            kind: Some(AgentKind::Codex),
            ..claude
        };
        let codex = tuned(&codex, &AgentOptions::default());
        assert!(!thinking_off(&codex));
        assert_eq!(codex.env["CODEX_ACP_PROMPT_SUGGESTIONS"], "false");
    }

    #[test]
    fn resolves_the_configured_or_a_found_agent() {
        let dir = std::env::temp_dir().join(format!("moth-agent-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.display().to_string();
        assert!(matches!(resolve(None, &path), Resolution::NotFound(_)));
        let claude: AgentSetting = serde_json::from_str(r#""claude""#).unwrap();
        assert!(matches!(
            resolve(Some(&claude), &path),
            Resolution::NotFound(_)
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let program = |name: &str| {
                let program = dir.join(name);
                std::fs::write(&program, "#!/bin/sh\n").unwrap();
                std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
                program
            };
            // A CLI says which agent; its adapter runs through npx.
            program("codex");
            let npx = program("npx");
            program("node");
            let Resolution::Found(found) = resolve(None, &path) else {
                panic!("codex should be found");
            };
            assert_eq!(found.program, npx);
            assert_eq!(found.args, ["-y", CODEX_ADAPTER]);
            assert_eq!(found.name, "Codex");
            let Resolution::Found(found) = resolve(Some(&claude), &path) else {
                panic!("claude runs through npx");
            };
            assert_eq!(found.args, ["-y", CLAUDE_ADAPTER]);
            // An adapter installed by hand comes first.
            let adapter = program("claude-agent-acp");
            let Resolution::Found(found) = resolve(None, &path) else {
                panic!("claude-agent-acp should be found");
            };
            assert_eq!((found.program, found.name.as_str()), (adapter, "Claude"));
        }
        let off: AgentSetting = serde_json::from_str("false").unwrap();
        assert_eq!(resolve(Some(&off), &path), Resolution::Off);
        let custom: AgentSetting =
            serde_json::from_str(r#"{"command": "my-agent", "args": ["--acp"]}"#).unwrap();
        assert!(matches!(
            resolve(Some(&custom), &path),
            Resolution::NotFound(_)
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn says_how_to_log_in() {
        assert_eq!(
            explain("Codex", "Authentication required".into()),
            "Log in once in a terminal with `codex login`. (Authentication required)"
        );
        assert_eq!(explain("Codex", "Rate limited".into()), "Rate limited");
        assert_eq!(explain("my-agent", "Not logged in".into()), "Not logged in");
    }

    /// An ACP agent that takes a second to answer each prompt, or ends the turn as `cancelled`
    /// if `session/cancel` comes first.
    #[cfg(unix)]
    const SLOW_AGENT: &str = r#"#!/usr/bin/env python3
# A slow ACP agent: each prompt waits up to 1 s for session/cancel, then answers.
import json, sys, select
def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n"); sys.stdout.flush()
n = 0
current = {}
def options():
    return [
        {"id":"model","name":"Model","category":"model","type":"select","currentValue":current["model"],
         "options":[{"value":"big","name":"Big","description":"Most capable"},{"value":"small","name":"Small","description":"Fastest"}]},
        {"id":"effort","name":"Effort","category":"thought_level","type":"select","currentValue":current["effort"],
         "options":[{"value":"low","name":"Low"},{"value":"high","name":"High"}]},
    ]
while True:
    line = sys.stdin.readline()
    if not line: sys.exit(0)
    msg = json.loads(line); method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":msg["id"],"result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[]}})
    elif method == "session/new":
        n += 1
        current = {"model": "big", "effort": "high"}
        send({"jsonrpc":"2.0","id":msg["id"],"result":{"sessionId":"s%d" % n,"configOptions":options()}})
    elif method == "session/set_config_option":
        current[msg["params"]["configId"]] = msg["params"]["value"]
        send({"jsonrpc":"2.0","id":msg["id"],"result":{"configOptions":options()}})
    elif method == "session/prompt":
        sid = msg["params"]["sessionId"]; cancelled = False
        ready, _, _ = select.select([sys.stdin], [], [], 1.0)
        if ready:
            other = json.loads(sys.stdin.readline())
            cancelled = other.get("method") == "session/cancel"
        if not cancelled:
            send({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"%s/%s" % (current["model"], current["effort"])}}}})
        send({"jsonrpc":"2.0","id":msg["id"],"result":{"stopReason":"cancelled" if cancelled else "end_turn"}})
"#;

    #[cfg(unix)]
    #[test]
    fn a_cancelled_request_leaves_the_agent_working() {
        use std::os::unix::fs::PermissionsExt as _;
        if crate::process::which("python3", &std::env::var("PATH").unwrap()).is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("moth-slow-agent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("slow-agent");
        std::fs::write(&program, SLOW_AGENT).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let command = AgentCommand {
            program,
            args: Vec::new(),
            env: BTreeMap::new(),
            name: "Slow".into(),
            kind: None,
        };
        let worker = Worker::start(command, std::env::var("PATH").unwrap()).unwrap();
        let request = |prompt: &str| {
            let (updates_tx, updates) = unbounded();
            let (cancel_tx, cancel) = oneshot::channel();
            let request = Request {
                prompt: prompt.into(),
                cwd: dir.clone(),
                options: AgentOptions::default(),
                updates: updates_tx,
                cancel,
            };
            worker.requests.unbounded_send(request).unwrap();
            (updates, cancel_tx)
        };
        // Closing the panel while the agent works, then asking again.
        let (_, first) = request("first");
        thread::sleep(Duration::from_millis(300));
        drop(first);
        let (second, _second) = request("second");
        let updates: Vec<Update> = futures::executor::block_on(second.collect());
        // Switched to the small model at low effort before the prompt.
        assert_eq!(
            updates,
            [
                Update::Model("Small · low effort".into()),
                Update::Text("small/low".into()),
                Update::Done
            ]
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
