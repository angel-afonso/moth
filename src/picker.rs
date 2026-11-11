//! Filterable list over a text field: the basis of the command palette, history search
//! and the branch picker. Drawn as an overlay on top of the window.

use std::path::PathBuf;

use gpui::{
    Action, App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding,
    ScrollStrategy, SharedString, Subscription, UniformListScrollHandle, Window, div, prelude::*,
    px, uniform_list,
};
use ui::{
    Color, HighlightedLabel, Icon, IconName, IconSize, Label, LabelCommon, LabelSize, h_flex,
    v_flex,
};

use crate::{
    components::{HostChip, Kbd, KindBadge, SuggestionKind},
    fuzzy::fuzzy_match,
    text_input::{self, TextInput},
    theme::MONO_FONT_FAMILY,
    tokens::{TokensExt, metrics},
};

const ROW_HEIGHT: f32 = 34.;
const MAX_VISIBLE_ROWS: usize = 11;
const MAX_RESULTS: usize = 300;

gpui::actions!(picker, [NextFilter, PreviousFilter]);

pub fn init(cx: &mut App) {
    text_input::init(cx);
    let ctx = Some("Picker");
    cx.bind_keys([
        KeyBinding::new("up", menu::SelectPrevious, ctx),
        KeyBinding::new("down", menu::SelectNext, ctx),
        KeyBinding::new("ctrl-p", menu::SelectPrevious, ctx),
        KeyBinding::new("ctrl-n", menu::SelectNext, ctx),
        KeyBinding::new("pageup", menu::SelectFirst, ctx),
        KeyBinding::new("pagedown", menu::SelectLast, ctx),
        KeyBinding::new("enter", menu::Confirm, ctx),
        KeyBinding::new("secondary-enter", menu::SecondaryConfirm, ctx),
        KeyBinding::new("escape", menu::Cancel, ctx),
        KeyBinding::new("tab", NextFilter, ctx),
        KeyBinding::new("shift-tab", PreviousFilter, ctx),
    ]);
}

/// What happens when an item is chosen; the `Workspace` carries it out.
pub enum PickerAction {
    Dispatch(Box<dyn Action>),
    ActivateTab(usize),
    OpenDirInNewTab(PathBuf),
    InsertCommand(String),
    SwitchBranch(String),
    SetTheme(String),
    /// Runs a command in a new tab (with ⌘⏎, in the current one).
    RunCommand(String),
    ReconnectTab(usize),
}

impl Clone for PickerAction {
    fn clone(&self) -> Self {
        match self {
            Self::Dispatch(action) => Self::Dispatch(action.boxed_clone()),
            Self::ActivateTab(ix) => Self::ActivateTab(*ix),
            Self::OpenDirInNewTab(dir) => Self::OpenDirInNewTab(dir.clone()),
            Self::InsertCommand(text) => Self::InsertCommand(text.clone()),
            Self::SwitchBranch(branch) => Self::SwitchBranch(branch.clone()),
            Self::SetTheme(name) => Self::SetTheme(name.clone()),
            Self::RunCommand(command) => Self::RunCommand(command.clone()),
            Self::ReconnectTab(ix) => Self::ReconnectTab(*ix),
        }
    }
}

pub struct PickerItem {
    /// Section it's grouped under when the query is empty ("" = no header).
    pub section: SharedString,
    pub label: SharedString,
    pub detail: Option<SharedString>,
    pub icon: Option<IconName>,
    /// Suggestion kind; if set, shown instead of the icon.
    pub kind: Option<SuggestionKind>,
    /// Color dot instead of the icon (hosts).
    pub dot: Option<gpui::Hsla>,
    /// Chip after the text (host label).
    pub chip: Option<(SharedString, gpui::Hsla)>,
    /// Action whose shortcut is shown on the right.
    pub shortcut: Option<Box<dyn Action>>,
    pub mono: bool,
    /// The filters it shows under besides the first, which shows everything: bit N for
    /// the Nth of [`PickerConfig::filters`].
    pub filters: u32,
    pub action: PickerAction,
}

impl PickerItem {
    pub fn new(section: &str, label: impl Into<SharedString>, action: PickerAction) -> Self {
        Self {
            section: SharedString::from(section.to_string()),
            label: label.into(),
            detail: None,
            icon: None,
            kind: None,
            dot: None,
            chip: None,
            shortcut: None,
            mono: false,
            filters: 0,
            action,
        }
    }

    /// Also shows it under the Nth filter.
    pub fn in_filter(mut self, filter: usize) -> Self {
        self.filters |= 1 << filter;
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn dot(mut self, color: gpui::Hsla) -> Self {
        self.dot = Some(color);
        self
    }

    pub fn chip(mut self, label: impl Into<SharedString>, color: gpui::Hsla) -> Self {
        self.chip = Some((label.into(), color));
        self
    }

    pub fn kind(mut self, kind: SuggestionKind) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn shortcut(mut self, action: impl Action) -> Self {
        self.shortcut = Some(Box::new(action));
        self
    }

    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
}

pub struct PickerConfig {
    pub placeholder: SharedString,
    pub query: String,
    pub empty_text: SharedString,
    /// Shortcuts explained in the footer: (key, description).
    pub hints: Vec<(SharedString, SharedString)>,
    /// If set, choosing does nothing and this notice is shown.
    pub disabled_reason: Option<SharedString>,
    /// Item highlighted on open, with an empty query (index into the item list).
    pub selected_item: Option<usize>,
    /// Views of the list to switch between with Tab, shown as chips under the field. The
    /// first one shows every item; the others, the items tagged with them.
    pub filters: Vec<SharedString>,
}

pub enum PickerEvent {
    /// The highlighted item changed (for previews, like the theme picker).
    Selected(PickerAction),
    Confirm {
        action: PickerAction,
        secondary: bool,
    },
    Dismiss,
}

enum Row {
    Header(SharedString),
    Item { ix: usize, positions: Vec<usize> },
}

pub struct Picker {
    input: Entity<TextInput>,
    items: Vec<PickerItem>,
    rows: Vec<Row>,
    selected: Option<usize>,
    /// The filter in use (index into the config's).
    filter: usize,
    scroll: UniformListScrollHandle,
    config: PickerConfig,
    _input_changed: Subscription,
}

impl EventEmitter<PickerEvent> for Picker {}

impl Picker {
    pub fn new(items: Vec<PickerItem>, config: PickerConfig, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            let mut input = TextInput::new(config.placeholder.clone(), cx);
            input.set_text(config.query.clone(), cx);
            input
        });
        let input_changed = cx.subscribe(&input, |this, _, _: &text_input::Changed, cx| {
            this.update_matches(cx);
        });
        let mut this = Self {
            input,
            items,
            rows: Vec::new(),
            selected: None,
            filter: 0,
            scroll: UniformListScrollHandle::new(),
            config,
            _input_changed: input_changed,
        };
        this.update_matches(cx);
        this
    }

    fn update_matches(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).text().trim().to_string();
        self.rows.clear();
        let filter = self.filter;
        let shown = |item: &PickerItem| filter == 0 || item.filters & (1 << filter) != 0;
        if query.is_empty() {
            let mut section: Option<&SharedString> = None;
            for (ix, item) in self.items.iter().enumerate() {
                if !shown(item) {
                    continue;
                }
                if section != Some(&item.section) && !item.section.is_empty() {
                    self.rows.push(Row::Header(item.section.clone()));
                }
                section = Some(&item.section);
                self.rows.push(Row::Item {
                    ix,
                    positions: Vec::new(),
                });
            }
        } else {
            let mut matches: Vec<(i32, usize, Vec<usize>)> = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| shown(item))
                .filter_map(|(ix, item)| {
                    fuzzy_match(&query, &item.label).map(|m| (m.score, ix, m.positions))
                })
                .collect();
            // Best score first; ties keep the original order (e.g. most recent first).
            matches.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            self.rows.extend(
                matches
                    .into_iter()
                    .take(MAX_RESULTS)
                    .map(|(_, ix, positions)| Row::Item { ix, positions }),
            );
        }
        let preferred = query
            .is_empty()
            .then_some(self.config.selected_item)
            .flatten()
            .and_then(|item| {
                self.rows
                    .iter()
                    .position(|r| matches!(r, Row::Item { ix, .. } if *ix == item))
            });
        self.selected =
            preferred.or_else(|| self.rows.iter().position(|r| matches!(r, Row::Item { .. })));
        if let Some(selected) = self.selected {
            self.scroll.scroll_to_item(selected, ScrollStrategy::Center);
        }
        self.emit_selected(cx);
        cx.notify();
    }

    fn emit_selected(&self, cx: &mut Context<Self>) {
        if let Some(Row::Item { ix, .. }) = self.selected.and_then(|row| self.rows.get(row)) {
            cx.emit(PickerEvent::Selected(self.items[*ix].action.clone()));
        }
    }

    fn move_selection(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(current) = self.selected else {
            return;
        };
        let item_rows = |range: Box<dyn Iterator<Item = usize>>| {
            range
                .filter(|&i| matches!(self.rows[i], Row::Item { .. }))
                .collect::<Vec<_>>()
        };
        let candidates = if forward {
            item_rows(Box::new(current + 1..self.rows.len()))
        } else {
            item_rows(Box::new((0..current).rev()))
        };
        // Wrap around at either end.
        let next = candidates.first().copied().or_else(|| {
            let all = item_rows(Box::new(0..self.rows.len()));
            if forward {
                all.first().copied()
            } else {
                all.last().copied()
            }
        });
        if let Some(next) = next {
            self.select_row(next, cx);
        }
    }

    fn select_row(&mut self, row: usize, cx: &mut Context<Self>) {
        self.selected = Some(row);
        self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        self.emit_selected(cx);
        cx.notify();
    }

    fn select_next(&mut self, _: &menu::SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(true, cx);
    }

    fn select_previous(
        &mut self,
        _: &menu::SelectPrevious,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selection(false, cx);
    }

    fn select_first(&mut self, _: &menu::SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(first) = self.rows.iter().position(|r| matches!(r, Row::Item { .. })) {
            self.select_row(first, cx);
        }
    }

    fn select_last(&mut self, _: &menu::SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(last) = self
            .rows
            .iter()
            .rposition(|r| matches!(r, Row::Item { .. }))
        {
            self.select_row(last, cx);
        }
    }

    fn confirm_selected(&mut self, secondary: bool, cx: &mut Context<Self>) {
        if self.config.disabled_reason.is_some() {
            return;
        }
        if let Some(Row::Item { ix, .. }) = self.selected.and_then(|row| self.rows.get(row)) {
            let action = self.items[*ix].action.clone();
            cx.emit(PickerEvent::Confirm { action, secondary });
        }
    }

    fn confirm(&mut self, _: &menu::Confirm, _: &mut Window, cx: &mut Context<Self>) {
        self.confirm_selected(false, cx);
    }

    fn secondary_confirm(
        &mut self,
        _: &menu::SecondaryConfirm,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm_selected(true, cx);
    }

    fn cancel(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(PickerEvent::Dismiss);
    }

    fn set_filter(&mut self, filter: usize, cx: &mut Context<Self>) {
        if filter != self.filter && filter < self.config.filters.len() {
            self.filter = filter;
            self.update_matches(cx);
        }
    }

    fn next_filter(&mut self, _: &NextFilter, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.config.filters.len().max(1);
        self.set_filter((self.filter + 1) % count, cx);
    }

    fn previous_filter(&mut self, _: &PreviousFilter, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.config.filters.len().max(1);
        self.set_filter((self.filter + count - 1) % count, cx);
    }

    fn render_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let ui = cx.tokens().ui.clone();
        range
            .filter_map(|row_ix| {
                let row = self.rows.get(row_ix)?;
                Some(match row {
                    Row::Header(title) => div()
                        .w_full()
                        .h(px(ROW_HEIGHT))
                        .px(px(10.))
                        .pb(px(4.))
                        .flex()
                        .items_end()
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(ui.text_placeholder)
                        .child(title.to_uppercase())
                        .into_any_element(),
                    Row::Item { ix, positions } => {
                        let item = &self.items[*ix];
                        let selected = self.selected == Some(row_ix);
                        h_flex()
                            .id(("picker-row", row_ix))
                            .w_full()
                            .h(px(ROW_HEIGHT))
                            .px(px(10.))
                            .gap(px(10.))
                            .rounded(metrics::RADIUS_ROW)
                            .cursor_pointer()
                            .when(selected, |row| row.bg(ui.element_selected))
                            .when(!selected, |row| {
                                row.hover(|style| style.bg(ui.element_hover))
                            })
                            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                                this.selected = Some(row_ix);
                                this.confirm_selected(event.modifiers().platform, cx);
                            }))
                            .when_some(item.kind, |row, kind| row.child(KindBadge::new(kind)))
                            .when_some(item.dot.filter(|_| item.kind.is_none()), |row, color| {
                                row.child(
                                    div()
                                        .size(px(18.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(div().size(px(8.)).rounded_full().bg(color)),
                                )
                            })
                            .when(item.kind.is_none() && item.dot.is_none(), |row| {
                                row.children(item.icon.map(|icon| {
                                    Icon::new(icon).size(IconSize::Small).color(if selected {
                                        Color::Accent
                                    } else {
                                        Color::Muted
                                    })
                                }))
                            })
                            .child(
                                div()
                                    .overflow_hidden()
                                    .min_w_0()
                                    .when(item.mono, |el| el.font_family(MONO_FONT_FAMILY))
                                    .child(
                                        HighlightedLabel::new(
                                            item.label.clone(),
                                            positions.clone(),
                                        )
                                        .color(if selected {
                                            Color::Default
                                        } else {
                                            Color::Muted
                                        })
                                        .truncate(),
                                    ),
                            )
                            .when_some(item.chip.clone(), |row, (label, color)| {
                                row.child(HostChip::new(label, color))
                            })
                            .child(div().flex_1())
                            .when_some(item.detail.clone(), |row, detail| {
                                row.child(
                                    Label::new(detail)
                                        .size(LabelSize::Small)
                                        .color(Color::Placeholder)
                                        .truncate(),
                                )
                            })
                            .when_some(item.shortcut.as_ref(), |row, action| {
                                row.child(ui::KeyBinding::for_action(action.as_ref(), cx))
                            })
                            .into_any_element()
                    }
                })
            })
            .collect()
    }
}

impl Focusable for Picker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for Picker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visible_rows = self.rows.len().clamp(1, MAX_VISIBLE_ROWS);
        let ui = cx.tokens().ui.clone();

        v_flex()
            .key_context("Picker")
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::select_last))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::secondary_confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::next_filter))
            .on_action(cx.listener(Self::previous_filter))
            .w(px(620.))
            .max_w_full()
            .bg(ui.surface)
            .border_1()
            .border_color(ui.border_strong)
            .rounded(metrics::RADIUS_POPUP)
            .shadow_lg()
            .overflow_hidden()
            // A click inside the box must not close it (clicking the backdrop does).
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .h(px(48.))
                    .px(px(16.))
                    .gap(px(10.))
                    .border_b_1()
                    .border_color(ui.divider)
                    .child(
                        Icon::new(IconName::MagnifyingGlass)
                            .size(IconSize::Small)
                            .color(Color::Placeholder),
                    )
                    .child(self.input.clone())
                    .child(Kbd::new("esc")),
            )
            .when(!self.config.filters.is_empty(), |this| {
                this.child(
                    h_flex()
                        .px(px(12.))
                        .py(px(6.))
                        .gap(px(4.))
                        .border_b_1()
                        .border_color(ui.divider)
                        .children(self.config.filters.iter().enumerate().map(|(ix, name)| {
                            let active = ix == self.filter;
                            div()
                                .id(("picker-filter", ix))
                                .px(px(8.))
                                .py(px(2.))
                                .rounded(metrics::RADIUS_ROW)
                                .cursor_pointer()
                                .when(active, |chip| chip.bg(ui.element_selected))
                                .when(!active, |chip| {
                                    chip.hover(|style| style.bg(ui.element_hover))
                                })
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.set_filter(ix, cx);
                                }))
                                .child(
                                    Label::new(name.clone())
                                        .size(LabelSize::Small)
                                        .color(if active { Color::Default } else { Color::Muted }),
                                )
                        })),
                )
            })
            .child(if self.rows.is_empty() {
                div()
                    .h(px(ROW_HEIGHT * 2.))
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .child(
                        Label::new(self.config.empty_text.clone())
                            .size(LabelSize::Small)
                            .color(Color::Placeholder),
                    )
                    .into_any_element()
            } else {
                uniform_list(
                    "picker-rows",
                    self.rows.len(),
                    cx.processor(|this, range, _, cx| this.render_rows(range, cx)),
                )
                .track_scroll(&self.scroll)
                .h(px(ROW_HEIGHT * visible_rows as f32 + 12.))
                .p(px(6.))
                .into_any_element()
            })
            .when_some(self.config.disabled_reason.clone(), |this, reason| {
                this.child(
                    h_flex()
                        .px(px(16.))
                        .py(px(8.))
                        .gap_2()
                        .border_t_1()
                        .border_color(ui.divider)
                        .child(
                            Icon::new(IconName::Warning)
                                .size(IconSize::XSmall)
                                .color(Color::Warning),
                        )
                        .child(
                            Label::new(reason)
                                .size(LabelSize::Small)
                                .color(Color::Warning),
                        ),
                )
            })
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(px(14.))
                    .px(px(16.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(ui.divider)
                    .children(self.config.hints.iter().map(|(key, label)| {
                        h_flex().gap(px(5.)).child(Kbd::new(key.clone())).child(
                            Label::new(label.clone())
                                .size(LabelSize::Small)
                                .color(Color::Placeholder),
                        )
                    })),
            )
    }
}
