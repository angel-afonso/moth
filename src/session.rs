//! Open windows, tabs and panes, so they come back the same way when the app is relaunched.
//! Folders and layout are saved, not processes or what was on screen.
//!
//! Each window registers its part in a global store and the file is rewritten whenever
//! something changes. Closing a window by hand removes it from the store; quitting with ⌘Q
//! doesn't, so everything that was open comes back next time.

use std::path::PathBuf;

use gpui::{App, EntityId, Global};
use serde::{Deserialize, Serialize};

use crate::panes::PaneTree;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub windows: Vec<WindowSession>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowSession {
    pub tabs: Vec<TabSession>,
    /// Active tab.
    pub active: usize,
    /// Window position and size: x, y, width, height.
    pub bounds: Option<[f32; 4]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabSession {
    /// Each pane's folder.
    pub layout: PaneTree<Option<PathBuf>>,
    /// Active pane, in reading order.
    pub active_pane: usize,
}

fn path() -> Option<PathBuf> {
    Some(crate::state::state_dir()?.join("session.json"))
}

/// The saved session, without empty windows.
pub fn load() -> Option<Session> {
    let text = std::fs::read_to_string(path()?).ok()?;
    let session = match serde_json::from_str::<Session>(&text) {
        Ok(session) => session,
        Err(err) => {
            crate::log::error(format!("invalid session.json: {err}"));
            return None;
        }
    };
    Some(clean(session))
}

fn clean(session: Session) -> Session {
    let windows = session
        .windows
        .into_iter()
        .filter(|window| !window.tabs.is_empty())
        .map(|mut window| {
            for tab in &mut window.tabs {
                tab.layout = tab.layout.clone().normalized();
            }
            window.active = window.active.min(window.tabs.len() - 1);
            window
        })
        .collect();
    Session { windows }
}

/// What each open window contributes to the session, in the order they were opened.
#[derive(Default)]
struct Store {
    windows: Vec<(EntityId, WindowSession)>,
    written: Option<Session>,
}

impl Global for Store {}

/// Updates a window's part and rewrites the file if anything changed.
pub fn update(workspace: EntityId, window: WindowSession, cx: &mut App) {
    let store = cx.default_global::<Store>();
    match store.windows.iter_mut().find(|(id, _)| *id == workspace) {
        Some((_, current)) if *current == window => return,
        Some((_, current)) => *current = window,
        None => store.windows.push((workspace, window)),
    }
    write(cx);
}

/// The window was closed by hand: it won't come back next time.
pub fn remove(workspace: EntityId, cx: &mut App) {
    let store = cx.default_global::<Store>();
    store.windows.retain(|(id, _)| *id != workspace);
    write(cx);
}

fn write(cx: &mut App) {
    let store = cx.default_global::<Store>();
    let session = Session {
        windows: store
            .windows
            .iter()
            .map(|(_, window)| window.clone())
            .collect(),
    };
    if store.written.as_ref() == Some(&session) {
        return;
    }
    store.written = Some(session.clone());
    cx.background_executor()
        .spawn(async move {
            if let Err(err) = save(&session) {
                crate::log::error(format!("couldn't save the session: {err}"));
            }
        })
        .detach();
}

fn save(session: &Session) -> anyhow::Result<()> {
    let path = path().ok_or_else(|| anyhow::anyhow!("HOME not set"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(session)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panes::Axis;

    #[test]
    fn round_trips_and_cleans_up() {
        let mut layout = PaneTree::Leaf(Some(PathBuf::from("/a")));
        // A tree with broken sizes (edited by hand) gets fixed on load.
        layout = PaneTree::Split {
            axis: Axis::Row,
            children: vec![layout, PaneTree::Leaf(None)],
            sizes: vec![0.],
        };
        let session = Session {
            windows: vec![
                WindowSession {
                    tabs: vec![TabSession {
                        layout,
                        active_pane: 1,
                    }],
                    active: 7,
                    bounds: Some([10., 20., 800., 600.]),
                },
                WindowSession {
                    tabs: vec![],
                    active: 0,
                    bounds: None,
                },
            ],
        };
        let text = serde_json::to_string(&session).unwrap();
        let loaded = clean(serde_json::from_str(&text).unwrap());
        assert_eq!(loaded.windows.len(), 1);
        assert_eq!(loaded.windows[0].active, 0);
        let PaneTree::Split { sizes, .. } = &loaded.windows[0].tabs[0].layout else {
            panic!("expected a split");
        };
        assert_eq!(sizes, &vec![0.5, 0.5]);
    }
}
