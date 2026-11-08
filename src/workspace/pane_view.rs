//! Drawing a tab's pane tree, and the dividers that resize it.

use super::*;

/// Payload carried while dragging a divider; also serves as the (empty) drag view.
#[derive(Clone)]
struct DividerDrag {
    tab: usize,
    path: Vec<usize>,
    divider: usize,
}

impl Render for DividerDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// No pane shrinks below this when dragging.
const MIN_PANE_SIZE: Pixels = px(80.);

impl Workspace {
    /// The active tab's panes: a single one (or the maximized one) fills everything; otherwise
    /// the tree with one-pixel dividers and unfocused panes dimmed.
    pub(super) fn render_panes(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tab = self.tabs.get(self.active)?;
        if tab.zoomed || tab.panes.len() == 1 {
            return Some(tab.view().clone().into_any_element());
        }
        Some(self.render_pane_node(tab, &tab.layout, Vec::new(), cx))
    }

    fn render_pane_node(
        &self,
        tab: &TabEntry,
        node: &PaneTree<EntityId>,
        path: Vec<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = cx.tokens().clone();
        match node {
            PaneTree::Leaf(id) => {
                let Some(pane) = tab.pane(*id) else {
                    return div().into_any_element();
                };
                div()
                    .relative()
                    .size_full()
                    .child(pane.view.clone())
                    .when(*id != tab.active_pane, |el| {
                        el.child(
                            div()
                                .absolute()
                                .inset_0()
                                .bg(tint(tokens.terminal.background, 0.35)),
                        )
                    })
                    .into_any_element()
            }
            PaneTree::Split {
                axis,
                children,
                sizes,
            } => {
                let axis = *axis;
                let tab_id = tab.id;
                let mut container = div().flex().size_full().on_drag_move(cx.listener({
                    let path = path.clone();
                    move |this, event: &DragMoveEvent<DividerDrag>, _, cx| {
                        let drag = event.drag(cx);
                        if drag.tab != tab_id || drag.path != path {
                            return;
                        }
                        let divider = drag.divider;
                        let (offset, length) = match axis {
                            Axis::Row => (
                                event.event.position.x - event.bounds.origin.x,
                                event.bounds.size.width,
                            ),
                            Axis::Column => (
                                event.event.position.y - event.bounds.origin.y,
                                event.bounds.size.height,
                            ),
                        };
                        if length <= px(0.) {
                            return;
                        }
                        let min = MIN_PANE_SIZE / length;
                        if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                            tab.layout.resize(&path, divider, offset / length, min);
                            cx.notify();
                        }
                    }
                }));
                container = match axis {
                    Axis::Row => container.flex_row(),
                    Axis::Column => container.flex_col(),
                };
                for (ix, (child, size)) in children.iter().zip(sizes).enumerate() {
                    if ix > 0 {
                        container =
                            container.child(self.render_divider(tab_id, &path, ix, axis, cx));
                    }
                    let mut child_path = path.clone();
                    child_path.push(ix);
                    container = container.child(
                        div()
                            .flex_basis(relative(*size))
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .child(self.render_pane_node(tab, child, child_path, cx)),
                    );
                }
                container.into_any_element()
            }
        }
    }

    /// One-pixel line between panes, with a wider hit area for grabbing and dragging it.
    fn render_divider(
        &self,
        tab: usize,
        path: &[usize],
        divider: usize,
        axis: Axis,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let drag = DividerDrag {
            tab,
            path: path.to_vec(),
            divider,
        };
        let id = SharedString::from(format!("divider-{tab}-{path:?}-{divider}"));
        let handle = div()
            .id(id)
            .absolute()
            .occlude()
            .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()));
        let handle = match axis {
            Axis::Row => handle
                .top_0()
                .bottom_0()
                .left(px(-3.))
                .w(px(7.))
                .cursor(CursorStyle::ResizeLeftRight),
            Axis::Column => handle
                .left_0()
                .right_0()
                .top(px(-3.))
                .h(px(7.))
                .cursor(CursorStyle::ResizeUpDown),
        };
        let line = div()
            .relative()
            .flex_none()
            .bg(cx.tokens().ui.border_strong);
        let line = match axis {
            Axis::Row => line.w(px(1.)).h_full(),
            Axis::Column => line.h(px(1.)).w_full(),
        };
        // `deferred` paints it above the panes, so the pane to the right (or below) doesn't
        // cover half of the hit area.
        line.child(deferred(handle)).into_any_element()
    }
}
