//! The agent's panel at the prompt: ⌘I asks for a command in plain words, ⌘⇧I asks for a fix
//! for the command that just failed. The answer streams in, ⏎ puts the command on the prompt
//! and ⌘⏎ runs it; nothing runs without one of those. The footer says how many secrets were
//! taken out, and "What was sent" shows the context exactly as the agent got it.

use futures::StreamExt as _;
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    Subscription, Task, Window, div, prelude::*, px,
};
use ui::{
    Button, ButtonCommon, ButtonSize, ButtonStyle, Clickable, Color, Icon, IconName, IconSize,
    Label, LabelCommon, LabelSize, h_flex, v_flex,
};

use crate::{
    agent::{self, AgentCommand, Environment, Prompt, Reply, Resolution, Update},
    components::Kbd,
    fixes::Failure,
    shortcuts,
    terminal::Terminal,
    text_input::{Changed, TextInput},
    theme::MONO_FONT_FAMILY,
    tokens::{TokensExt, metrics, tint},
};

pub enum AgentPanelEvent {
    /// Put the command on the prompt; with `run`, also run it.
    Insert {
        command: String,
        run: bool,
    },
    Dismissed,
}

#[derive(Clone, Copy, PartialEq)]
pub enum AgentTask {
    /// A command for what the user describes.
    Ask,
    /// A fix for the command that just failed.
    Fix,
}

enum State {
    /// Ask: waiting for the request to be typed.
    Editing,
    Waiting,
    Answered(Reply),
    Failed(String),
}

pub struct AgentPanel {
    task: AgentTask,
    terminal: Entity<Terminal>,
    focus_handle: FocusHandle,
    input: Option<Entity<TextInput>>,
    /// What was asked last, to tell an edited request (⏎ asks again) from the same one.
    asked: Option<String>,
    agent: Option<AgentCommand>,
    failure: Option<Failure>,
    state: State,
    /// The answer as it arrives.
    reply: String,
    sent: Option<Prompt>,
    show_sent: bool,
    _pending: Option<Task<()>>,
    _input_changed: Option<Subscription>,
}

impl EventEmitter<AgentPanelEvent> for AgentPanel {}

impl AgentPanel {
    pub fn new(
        task: AgentTask,
        terminal: Entity<Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search_path = search_path(&terminal, cx);
        let (agent, unavailable) = match agent::find(&search_path, cx) {
            Resolution::Found(command) => (Some(command), None),
            Resolution::Off => (
                None,
                Some("The agent is off (\"agent\": false in the settings).".to_string()),
            ),
            Resolution::NotFound(why) => (
                None,
                Some(format!(
                    "{why}. Install an ACP adapter or set \"agent\" in the settings."
                )),
            ),
        };
        let failure = terminal
            .read(cx)
            .shell()
            .failure
            .as_ref()
            .map(|last| last.failure.clone());
        let production = terminal
            .read(cx)
            .shell()
            .remote
            .as_ref()
            .is_some_and(|remote| remote.identity.production);

        let (input, input_changed) = if task == AgentTask::Ask {
            let input = cx.new(|cx| {
                let mut input =
                    TextInput::new("Describe a command, like “find files over 100 MB”", cx);
                // Words already typed at the prompt are probably the request.
                let typed = terminal
                    .read(cx)
                    .shell()
                    .line
                    .as_ref()
                    .map(|line| line.buffer.trim().to_string());
                if let Some(typed) = typed {
                    input.set_text(typed, cx);
                }
                input
            });
            let changed = cx.subscribe(&input, |_, _, _: &Changed, cx| cx.notify());
            input.focus_handle(cx).focus(window, cx);
            (Some(input), Some(changed))
        } else {
            (None, None)
        };

        let mut panel = Self {
            task,
            terminal,
            focus_handle: cx.focus_handle(),
            input,
            asked: None,
            agent,
            failure,
            state: State::Editing,
            reply: String::new(),
            sent: None,
            show_sent: false,
            _pending: None,
            _input_changed: input_changed,
        };
        if task == AgentTask::Fix {
            panel.focus_handle.focus(window, cx);
        }
        if let Some(why) = unavailable {
            panel.state = State::Failed(why);
        } else if production {
            panel.state = State::Failed(
                "The agent is off on production hosts, so nothing from them leaves the machine."
                    .into(),
            );
        } else if task == AgentTask::Fix {
            match panel.failure.clone() {
                Some(failure) => {
                    let prompt = agent::fix_prompt(&environment(&panel.terminal, cx), &failure);
                    panel.send(prompt, cx);
                }
                None => panel.state = State::Failed("The last command didn't fail.".into()),
            }
        }
        panel
    }

    fn send(&mut self, prompt: Prompt, cx: &mut Context<Self>) {
        let Some(command) = self.agent.clone() else {
            return;
        };
        let search_path = search_path(&self.terminal, cx);
        let mut pending = agent::ask(&command, &search_path, prompt.text.clone(), cx);
        self.sent = Some(prompt);
        self.reply.clear();
        self.state = State::Waiting;
        self._pending = Some(cx.spawn(async move |this, cx| {
            // `pending` lives in this task: closing the panel drops it and cancels the turn.
            while let Some(update) = pending.updates.next().await {
                let finished = !matches!(update, Update::Text(_));
                let alive = this.update(cx, |this, cx| {
                    match update {
                        Update::Text(text) => this.reply.push_str(&text),
                        Update::Done => {
                            this.state = State::Answered(agent::parse_reply(&this.reply))
                        }
                        Update::Failed(why) => this.state = State::Failed(why),
                    }
                    cx.notify();
                });
                if alive.is_err() || finished {
                    return;
                }
            }
            this.update(cx, |this, cx| {
                this.state = State::Failed("The agent stopped without answering.".into());
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn ask(&mut self, cx: &mut Context<Self>) {
        let Some(input) = &self.input else {
            return;
        };
        let request = input.read(cx).text().trim().to_string();
        if request.is_empty() || self.agent.is_none() {
            return;
        }
        let files = folder_entries(&self.terminal, cx);
        let prompt = agent::ask_prompt(
            &environment(&self.terminal, cx),
            &request,
            &files,
            self.failure.as_ref(),
        );
        self.asked = Some(request);
        self.send(prompt, cx);
    }

    fn command(&self) -> Option<&str> {
        match &self.state {
            State::Answered(reply) => reply.command.as_deref(),
            _ => None,
        }
    }

    /// ⏎: asks (or asks again, if the request changed) or puts the answer on the prompt.
    fn confirm(&mut self, run: bool, cx: &mut Context<Self>) {
        let edited = self
            .input
            .as_ref()
            .is_some_and(|input| Some(input.read(cx).text().trim()) != self.asked.as_deref());
        if edited && !run {
            self.ask(cx);
            return;
        }
        if let Some(command) = self.command() {
            cx.emit(AgentPanelEvent::Insert {
                command: command.to_string(),
                run,
            });
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let m = &keystroke.modifiers;
        match keystroke.key.as_str() {
            "escape" => cx.emit(AgentPanelEvent::Dismissed),
            "enter" if m.secondary() => self.confirm(true, cx),
            "enter" if !(m.control || m.alt || m.shift || m.platform) => self.confirm(false, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn agent_name(&self) -> String {
        self.agent
            .as_ref()
            .map_or_else(|| "the agent".to_string(), |agent| agent.name.clone())
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

impl Focusable for AgentPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.input {
            Some(input) => input.focus_handle(cx),
            None => self.focus_handle.clone(),
        }
    }
}

/// The shell's PATH (the app's own when it hasn't said), where the agent is looked for.
fn search_path(terminal: &Entity<Terminal>, cx: &App) -> String {
    terminal
        .read(cx)
        .shell()
        .path_env
        .clone()
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default()
}

fn environment(terminal: &Entity<Terminal>, cx: &App) -> Environment {
    let shell = terminal.read(cx).shell();
    let configured = crate::config::get(cx).shell.clone();
    let (program, _) = crate::shell::shell_command(configured.as_deref());
    let shell_name =
        crate::process::command_name(std::path::Path::new(&program)).unwrap_or(program);
    match &shell.remote {
        Some(remote) => Environment {
            cwd: remote.remote_cwd.clone(),
            branch: None,
            shell: shell_name,
            remote: Some(remote.user_at_host()),
        },
        None => Environment {
            cwd: shell.cwd.as_deref().map(agent::tilde),
            branch: shell.git.as_ref().and_then(|git| git.branch.clone()),
            shell: shell_name,
            remote: None,
        },
    }
}

/// Names (never contents) of what's in the current folder, to say what kind of project it is.
fn folder_entries(terminal: &Entity<Terminal>, cx: &App) -> Vec<String> {
    let shell = terminal.read(cx).shell();
    if shell.remote.is_some() {
        return Vec::new();
    }
    let Some(cwd) = shell.cwd.as_ref() else {
        return Vec::new();
    };
    let mut names: Vec<String> = std::fs::read_dir(cwd)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if name.starts_with('.') {
                return None;
            }
            let dir = entry.file_type().is_ok_and(|t| t.is_dir());
            Some(if dir { format!("{name}/") } else { name })
        })
        .collect();
    names.sort();
    names.truncate(40);
    names
}

impl Render for AgentPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = cx.tokens();
        let ui = tokens.ui.clone();
        let error = tokens.terminal.ansi.red;
        let accent = ui.accent;
        let agent = self.agent_name();
        let title = match self.task {
            AgentTask::Ask => format!("Ask {agent}"),
            AgentTask::Fix => format!("Fix with {agent}"),
        };
        let hint = |key: String, label: &'static str| {
            h_flex().gap(px(5.)).child(Kbd::new(key)).child(
                Label::new(label)
                    .size(LabelSize::XSmall)
                    .color(Color::Placeholder),
            )
        };

        let header = h_flex()
            .gap(px(8.))
            .child(
                Icon::new(self.icon())
                    .size(IconSize::Small)
                    .color(Color::Muted),
            )
            .child(
                Label::new(title)
                    .size(LabelSize::Small)
                    .weight(gpui::FontWeight::SEMIBOLD),
            )
            .child(div().flex_1())
            .child(hint("esc".into(), "close"));

        let failed_command = (self.task == AgentTask::Fix)
            .then(|| self.failure.clone())
            .flatten()
            .map(|failure| {
                let exit = failure
                    .exit_code
                    .map_or(String::new(), |code| format!("exit {code}"));
                h_flex()
                    .gap(px(8.))
                    .child(
                        Icon::new(IconName::XCircle)
                            .size(IconSize::XSmall)
                            .color(Color::Custom(error)),
                    )
                    .child(
                        div().min_w_0().font_family(MONO_FONT_FAMILY).child(
                            Label::new(failure.command)
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .truncate(),
                        ),
                    )
                    .child(
                        Label::new(exit)
                            .size(LabelSize::XSmall)
                            .color(Color::Placeholder),
                    )
            });

        let input = self.input.clone().map(|input| {
            h_flex()
                .px(px(10.))
                .py(px(6.))
                .rounded(metrics::RADIUS_ROW)
                .bg(ui.background)
                .border_1()
                .border_color(ui.border)
                .child(input)
        });

        let body = match &self.state {
            State::Editing => None,
            State::Waiting => Some(
                v_flex()
                    .gap(px(4.))
                    .child(
                        Label::new(format!("Asking {agent}…"))
                            .size(LabelSize::Small)
                            .color(Color::Placeholder),
                    )
                    .when(!self.reply.trim().is_empty(), |el| {
                        let partial = agent::parse_reply(&self.reply);
                        el.child(
                            Label::new(partial.explanation)
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    })
                    .into_any_element(),
            ),
            State::Answered(reply) => Some(
                v_flex()
                    .gap(px(8.))
                    .when(!reply.explanation.is_empty(), |el| {
                        el.child(
                            Label::new(reply.explanation.clone())
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    })
                    .when_some(reply.command.clone(), |el, command| {
                        el.child(
                            div()
                                .px(px(10.))
                                .py(px(7.))
                                .rounded(metrics::RADIUS_ROW)
                                .bg(ui.background)
                                .border_1()
                                .border_color(tint(accent, 0.5))
                                .font_family(MONO_FONT_FAMILY)
                                .text_size(px(12.5))
                                .text_color(ui.text)
                                .child(command),
                        )
                        .child(
                            h_flex()
                                .gap(px(6.))
                                .child(
                                    Button::new("agent-insert", "Put on Prompt")
                                        .style(ButtonStyle::Filled)
                                        .size(ButtonSize::Medium)
                                        .label_size(LabelSize::Small)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.confirm(false, cx)),
                                        ),
                                )
                                .child(hint("⏎".into(), "put on prompt"))
                                .child(div().w(px(8.)))
                                .child(
                                    Button::new("agent-run", "Run")
                                        .style(ButtonStyle::Outlined)
                                        .size(ButtonSize::Medium)
                                        .label_size(LabelSize::Small)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.confirm(true, cx)),
                                        ),
                                )
                                .child(hint(shortcuts::label("cmd-enter"), "run")),
                        )
                    })
                    .into_any_element(),
            ),
            State::Failed(why) => Some(
                h_flex()
                    .gap(px(8.))
                    .child(
                        Icon::new(IconName::Warning)
                            .size(IconSize::XSmall)
                            .color(Color::Custom(error)),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Label::new(why.clone())
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    )
                    .into_any_element(),
            ),
        };

        let footer = self.sent.as_ref().map(|sent| {
            let summary = match (sent.redactions, sent.withheld_output) {
                (_, true) => "Output left out: this command prints secrets".to_string(),
                (0, false) => "No secrets found in what was sent".to_string(),
                (1, false) => "1 secret removed before sending".to_string(),
                (n, false) => format!("{n} secrets removed before sending"),
            };
            let show = self.show_sent;
            v_flex()
                .gap(px(6.))
                .pt(px(8.))
                .border_t_1()
                .border_color(ui.divider)
                .child(
                    h_flex()
                        .gap(px(6.))
                        .child(
                            Icon::new(IconName::Lock)
                                .size(IconSize::XSmall)
                                .color(Color::Placeholder),
                        )
                        .child(
                            Label::new(summary)
                                .size(LabelSize::XSmall)
                                .color(Color::Placeholder),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new(
                                "agent-show-sent",
                                if show {
                                    "Hide what was sent"
                                } else {
                                    "What was sent"
                                },
                            )
                            .style(ButtonStyle::Subtle)
                            .size(ButtonSize::Compact)
                            .label_size(LabelSize::XSmall)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_sent = !this.show_sent;
                                cx.notify();
                            })),
                        ),
                )
                .when(show, |el| {
                    el.child(
                        div()
                            .id("agent-sent")
                            .max_h(px(180.))
                            .overflow_y_scroll()
                            .px(px(10.))
                            .py(px(7.))
                            .rounded(metrics::RADIUS_ROW)
                            .bg(ui.background)
                            .font_family(MONO_FONT_FAMILY)
                            .text_size(px(11.))
                            .text_color(ui.text_muted)
                            .child(sent.context.clone()),
                    )
                })
        });

        let ask_hint =
            (self.task == AgentTask::Ask && matches!(self.state, State::Editing)).then(|| {
                h_flex().gap(px(12.)).child(hint("⏎".into(), "ask")).child(
                    Label::new("The command goes on the prompt; nothing runs on its own.")
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder),
                )
            });

        v_flex()
            .key_context("AgentPanel")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .w(px(540.))
            .p(px(12.))
            .gap(px(10.))
            .bg(ui.surface)
            .border_1()
            .border_color(ui.border_strong)
            .rounded(px(10.))
            .shadow_lg()
            .child(header)
            .children(failed_command)
            .children(input)
            .children(ask_hint)
            .children(body)
            .children(footer)
    }
}
