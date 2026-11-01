//! Components from the "Tokens and components" sheet: tab with its states, key, suggestion
//! kind and status indicators. Built with the `ui` crate and the theme tokens.

use std::time::Duration;

use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, Hsla, MouseButton, MouseDownEvent,
    SharedString, Window, div, prelude::*, px,
};
use ui::{
    ButtonCommon, Clickable, Color, Icon, IconButton, IconName, IconSize, Label, LabelCommon,
    Tooltip, h_flex,
};

use crate::{
    theme::MONO_FONT_FAMILY,
    tokens::{TokensExt, metrics, tint},
};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type MouseDownHandler = Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>;

/// Human-readable duration: `120ms`, `3.4s`, `2m 5s`.
pub fn format_duration(duration: Duration) -> String {
    let ms = duration.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", duration.as_secs_f32())
    } else {
        format!("{}m {}s", ms / 60_000, (ms / 1000) % 60)
    }
}

// ---------------------------------------------------------------------------------------

/// A key or shortcut drawn as a keycap (`⌘K`, `esc`, `↑↓`).
#[derive(IntoElement)]
pub struct Kbd {
    keys: SharedString,
}

impl Kbd {
    pub fn new(keys: impl Into<SharedString>) -> Self {
        Self { keys: keys.into() }
    }
}

impl RenderOnce for Kbd {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui = &cx.tokens().ui;
        div()
            .px(px(5.))
            .rounded(metrics::RADIUS_KBD)
            .bg(ui.element_background)
            .border_1()
            .border_color(ui.border_strong)
            .font_family(MONO_FONT_FAMILY)
            .text_size(px(10.5))
            .line_height(px(16.))
            .text_color(ui.text_muted)
            .child(self.keys)
    }
}

// ---------------------------------------------------------------------------------------

/// Kind of a suggestion, colored by its syntax category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuggestionKind {
    Command,
    Subcommand,
    Flag,
    Path,
    Branch,
    History,
}

#[derive(IntoElement)]
pub struct KindBadge {
    kind: SuggestionKind,
}

impl KindBadge {
    pub fn new(kind: SuggestionKind) -> Self {
        Self { kind }
    }
}

impl RenderOnce for KindBadge {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let syntax = &cx.tokens().syntax;
        let (color, glyph) = match self.kind {
            SuggestionKind::Command => (syntax.command, Some("C")),
            SuggestionKind::Subcommand => (syntax.subcommand, Some("S")),
            SuggestionKind::Flag => (syntax.flag, Some("−")),
            SuggestionKind::Path => (syntax.path, Some("/")),
            SuggestionKind::Branch => (syntax.command, Some("B")),
            SuggestionKind::History => (syntax.flag, None),
        };
        div()
            .size(px(18.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(metrics::RADIUS_KBD)
            .bg(tint(color, 0.16))
            .text_color(color)
            .font_family(MONO_FONT_FAMILY)
            .text_size(px(10.))
            .font_weight(FontWeight::SEMIBOLD)
            .map(|badge| match glyph {
                Some(glyph) => badge.child(glyph),
                None => badge.child(
                    Icon::new(IconName::HistoryRerun)
                        .size(IconSize::XSmall)
                        .color(Color::Custom(color)),
                ),
            })
    }
}

// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DotState {
    /// Process running: green with a halo.
    Running,
    /// Needs attention (bell in the background).
    Attention,
    Success,
    Error,
}

#[derive(IntoElement)]
pub struct StatusDot {
    state: DotState,
    size: f32,
}

impl StatusDot {
    pub fn new(state: DotState) -> Self {
        Self { state, size: 7. }
    }

    pub fn small(mut self) -> Self {
        self.size = 6.;
        self
    }
}

impl RenderOnce for StatusDot {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let status = &cx.tokens().status;
        let color = match self.state {
            DotState::Running | DotState::Success => status.success,
            DotState::Attention => status.warning,
            DotState::Error => status.error,
        };
        let dot = div().size(px(self.size)).rounded_full().bg(color);
        if self.state == DotState::Running {
            div()
                .size(px(self.size + 6.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(tint(color, 0.15))
                .child(dot)
                .into_any_element()
        } else {
            dot.into_any_element()
        }
    }
}

// ---------------------------------------------------------------------------------------

/// Result of a block: `✓ 41.2s`, `✕ exit 1 · 1.3s`, or `● 12s` while running.
#[derive(IntoElement)]
pub enum BlockStatus {
    Running(Duration),
    Finished { exit_code: i32, duration: Duration },
}

impl RenderOnce for BlockStatus {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let status = &cx.tokens().status;
        let (text, color) = match self {
            BlockStatus::Running(elapsed) => {
                (format!("● {}", format_duration(elapsed)), status.warning)
            }
            BlockStatus::Finished {
                exit_code: 0,
                duration,
            } => (format!("✓ {}", format_duration(duration)), status.success),
            BlockStatus::Finished {
                exit_code,
                duration,
            } => (
                format!("✕ exit {exit_code} · {}", format_duration(duration)),
                status.error,
            ),
        };
        div()
            .font_family(MONO_FONT_FAMILY)
            .text_size(metrics::TEXT_META)
            .text_color(color)
            .child(text)
    }
}

// ---------------------------------------------------------------------------------------

/// Short label for a remote host (`PROD`, `STG`) in its color. Dashed if the session dropped.
#[derive(IntoElement)]
pub struct HostChip {
    label: SharedString,
    color: Hsla,
    dashed: bool,
}

impl HostChip {
    pub fn new(label: impl Into<SharedString>, color: Hsla) -> Self {
        Self {
            label: label.into(),
            color,
            dashed: false,
        }
    }

    pub fn dashed(mut self, dashed: bool) -> Self {
        self.dashed = dashed;
        self
    }
}

impl RenderOnce for HostChip {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex_none()
            .px(px(6.))
            .rounded(metrics::RADIUS_KBD)
            .font_family(MONO_FONT_FAMILY)
            .text_size(px(10.))
            .line_height(px(16.))
            .font_weight(FontWeight::SEMIBOLD)
            .map(|chip| {
                if self.dashed {
                    chip.border_1()
                        .border_dashed()
                        .border_color(tint(self.color, 0.5))
                        .text_color(self.color.opacity(0.7))
                } else {
                    chip.bg(tint(self.color, 0.16)).text_color(self.color)
                }
            })
            .child(self.label)
    }
}

// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabState {
    Idle,
    Running,
    Attention,
}

/// Tab bar tab: active (terminal background and accent line), inactive, hover,
/// running (green dot) or needing attention (orange dot).
#[derive(IntoElement)]
pub struct TabItem {
    id: ElementId,
    title: SharedString,
    /// Remote session: host color and its label, if any.
    remote: Option<(Hsla, Option<SharedString>)>,
    disconnected: bool,
    on_reconnect: Option<ClickHandler>,
    shortcut: Option<SharedString>,
    selected: bool,
    state: TabState,
    close_tooltip: SharedString,
    on_click: Option<ClickHandler>,
    on_close: Option<ClickHandler>,
    on_middle_click: Option<MouseDownHandler>,
}

impl TabItem {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            remote: None,
            disconnected: false,
            on_reconnect: None,
            shortcut: None,
            selected: false,
            state: TabState::Idle,
            close_tooltip: "Close Tab".into(),
            on_click: None,
            on_close: None,
            on_middle_click: None,
        }
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn state(mut self, state: TabState) -> Self {
        self.state = state;
        self
    }

    pub fn remote(mut self, color: Hsla, label: Option<SharedString>) -> Self {
        self.remote = Some((color, label));
        self
    }

    /// The remote session dropped: struck-through title and a reconnect button instead of close.
    pub fn disconnected(
        mut self,
        on_reconnect: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.disconnected = true;
        self.on_reconnect = Some(Box::new(on_reconnect));
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    pub fn on_close(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_close = Some(Box::new(handler));
        self
    }

    pub fn on_middle_click(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_middle_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for TabItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui = cx.tokens().ui.clone();
        let warning = cx.tokens().status.warning;
        let selected = self.selected;
        let disconnected = self.disconnected;
        let close_id = ElementId::from(SharedString::from(format!("{:?}-close", self.id)));
        let host_color = self.remote.as_ref().map(|(color, _)| *color);

        let indicator: AnyElement = match (self.state, host_color, disconnected) {
            (TabState::Attention, _, _) => StatusDot::new(DotState::Attention).into_any_element(),
            (_, Some(_), true) => Icon::new(IconName::LockOff)
                .size(IconSize::Small)
                .color(Color::Placeholder)
                .into_any_element(),
            (_, Some(color), false) => Icon::new(IconName::ArrowRightLeft)
                .size(IconSize::Small)
                .color(Color::Custom(color))
                .into_any_element(),
            (TabState::Running, None, _) => StatusDot::new(DotState::Running).into_any_element(),
            _ => Icon::new(IconName::Terminal)
                .size(IconSize::Small)
                .color(if selected {
                    Color::Accent
                } else {
                    Color::Placeholder
                })
                .into_any_element(),
        };
        let accent = host_color.filter(|_| !disconnected).unwrap_or(ui.accent);

        h_flex()
            .id(self.id)
            .relative()
            .h_full()
            .flex_none()
            .pl(px(14.))
            .pr(px(6.))
            .gap(metrics::SPACE_2)
            .border_r_1()
            .border_color(ui.border)
            .cursor_pointer()
            .map(|tab| {
                if selected {
                    // Blends into the terminal: same background, no bottom border.
                    tab.bg(ui.background).child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h(px(2.))
                            .bg(accent),
                    )
                } else {
                    tab.border_b_1().hover(|style| style.bg(ui.element_hover))
                }
            })
            .when_some(self.on_click, |tab, handler| tab.on_click(handler))
            .when_some(self.on_middle_click, |tab, handler| {
                tab.on_mouse_down(MouseButton::Middle, handler)
            })
            .child(
                div()
                    .size(px(14.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(indicator),
            )
            .child(
                div().max_w(px(200.)).child(
                    Label::new(self.title)
                        .color(if disconnected {
                            Color::Placeholder
                        } else if selected {
                            Color::Default
                        } else {
                            Color::Muted
                        })
                        .when(selected && !disconnected, |label| {
                            label.weight(FontWeight::MEDIUM)
                        })
                        .when(disconnected, |label| label.strikethrough())
                        .truncate(),
                ),
            )
            .when_some(
                self.remote
                    .and_then(|(color, label)| label.map(|l| (color, l))),
                |tab, (color, label)| tab.child(HostChip::new(label, color).dashed(disconnected)),
            )
            .when_some(self.shortcut, |tab, shortcut| {
                tab.child(
                    div()
                        .font_family(MONO_FONT_FAMILY)
                        .text_size(metrics::TEXT_META)
                        .text_color(ui.text_placeholder)
                        .child(shortcut),
                )
            })
            .when_some(self.on_reconnect, |tab, handler| {
                let reconnect_id =
                    ElementId::from(SharedString::from(format!("{close_id:?}-reconnect")));
                tab.child(
                    div()
                        .rounded(metrics::RADIUS_ROW)
                        .bg(tint(warning, 0.14))
                        .child(
                            IconButton::new(reconnect_id, IconName::RotateCw)
                                .icon_size(IconSize::XSmall)
                                .icon_color(Color::Warning)
                                .size(ui::ButtonSize::Medium)
                                .tooltip(Tooltip::text(format!(
                                    "Reconnect ({})",
                                    crate::shortcuts::label("cmd-shift-r")
                                )))
                                .on_click(move |event, window, cx| {
                                    cx.stop_propagation();
                                    handler(event, window, cx)
                                }),
                        ),
                )
            })
            .when(!disconnected, |tab| {
                tab.child(
                    IconButton::new(close_id, IconName::Close)
                        .icon_size(IconSize::XSmall)
                        .icon_color(if selected {
                            Color::Muted
                        } else {
                            Color::Placeholder
                        })
                        .size(ui::ButtonSize::Medium)
                        .tooltip(Tooltip::text(self.close_tooltip))
                        .when_some(self.on_close, |button, handler| {
                            button.on_click(move |event, window, cx| {
                                cx.stop_propagation();
                                handler(event, window, cx)
                            })
                        }),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(Duration::from_millis(120)), "120ms");
        assert_eq!(format_duration(Duration::from_millis(41_180)), "41.2s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 5s");
    }
}
