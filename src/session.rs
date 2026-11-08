//! Open windows, tabs and panes, so they come back the same way when the app is relaunched.
//! Folders and layout are saved, not processes or what was on screen.
//!
//! Each window registers its part in a global store and the file is rewritten shortly after
//! something changes (and on quit). Closing a window by hand removes it from the store; quitting with ⌘Q
//! doesn't, so everything that was open comes back next time.

use std::{
    path::PathBuf,
    sync::{Mutex, MutexGuard, PoisonError},
    time::Duration,
};

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
        .filter_map(|mut window| {
            window.tabs = std::mem::take(&mut window.tabs)
                .into_iter()
                .filter_map(|mut tab| {
                    tab.layout = tab.layout.normalized()?;
                    Some(tab)
                })
                .collect();
            let last = window.tabs.len().checked_sub(1)?;
            window.active = window.active.min(last);
            Some(window)
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
    // Moving or resizing a window changes the session on every frame: the file is written
    // once things settle, with whatever is latest by then.
    let mut pending = lock(&PENDING);
    pending.session = Some(session);
    if pending.scheduled {
        return;
    }
    pending.scheduled = true;
    drop(pending);
    let executor = cx.background_executor().clone();
    cx.background_executor()
        .spawn(async move {
            executor.timer(SAVE_DELAY).await;
            save_pending(true);
        })
        .detach();
}

/// The session waiting to be written, and whether a write is on its way.
struct Pending {
    session: Option<Session>,
    scheduled: bool,
}

static PENDING: Mutex<Pending> = Mutex::new(Pending {
    session: None,
    scheduled: false,
});

/// Held while the file is written, so writes never overlap.
static WRITING: Mutex<()> = Mutex::new(());

const SAVE_DELAY: Duration = Duration::from_millis(300);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Writes the session waiting to be written, if there is one. `scheduled`: called by the
/// write `write` scheduled, which is then done.
fn save_pending(scheduled: bool) {
    let _writing = lock(&WRITING);
    let session = {
        let mut pending = lock(&PENDING);
        if scheduled {
            pending.scheduled = false;
        }
        pending.session.take()
    };
    if let Some(session) = session
        && let Err(err) = save(&session)
    {
        crate::log::error(format!("couldn't save the session: {err}"));
    }
}

/// Writes what's still waiting before the app quits.
pub fn init(cx: &mut App) {
    cx.on_app_quit(|_| {
        save_pending(false);
        async {}
    })
    .detach();
}

fn save(session: &Session) -> anyhow::Result<()> {
    let path = path().ok_or_else(|| anyhow::anyhow!("HOME not set"))?;
    crate::state::write_atomic(&path, &serde_json::to_vec_pretty(session)?)?;
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
