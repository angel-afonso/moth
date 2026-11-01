//! Keeps Moth up to date: looks for a newer release shortly after startup and every few hours
//! (unless `"check_updates": false` in config.json), downloads it in the background, and swaps
//! it in when the user restarts. Checking, verifying and installing live in `update`; this is
//! the app's side: the schedule, the status bar, the notice and the restart.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use gpui::{AnyElement, App, AsyncApp, Global, actions, div, prelude::*};
use ui::{
    Button, ButtonCommon, ButtonSize, ButtonStyle, Clickable, Color, Label, LabelCommon, LabelSize,
};

use crate::{
    tokens::TokensExt,
    update::{Installation, Update, Updater},
};

actions!(moth, [CheckForUpdates, RestartToUpdate]);

/// The key the release workflow signs updates with.
const PUBLIC_KEY: &str = include_str!("../assets/update.pub");
/// Long enough after startup not to slow it down.
const FIRST_CHECK: Duration = Duration::from_secs(10);
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

enum Status {
    Idle,
    Checking,
    /// A newer release was found; its download is about to start.
    Available,
    Downloading {
        percent: Option<u8>,
    },
    Ready {
        update: Update,
        archive: PathBuf,
    },
    Installing,
}

/// What the notice above the tabs says, if anything.
#[derive(Clone)]
pub enum Notice {
    Info(String),
    Error(String),
    /// An update is downloaded and verified: the notice offers the restart.
    Ready(String),
    /// Restarting would end these processes: the notice asks first.
    ConfirmRestart(Vec<String>),
}

struct Updates {
    status: Status,
    notice: Option<Notice>,
    /// A restart is installing, so another one must not start.
    restart_pending: bool,
    /// Why updates are unavailable, when they are (a development build, a build without a
    /// release repository, an app not installed from a release).
    updater: Result<(Arc<Updater>, Installation), String>,
}

impl Global for Updates {}

/// Whether a restart into the downloaded update can start.
enum Restart {
    /// The archive to install, and where.
    Begin(PathBuf, Installation),
    /// One is already under way.
    Busy,
    NotReady,
}

impl Updates {
    /// Starts a restart unless one is under way; [`Self::end_restart`] allows the next one
    /// when it fails.
    fn begin_restart(&mut self) -> Restart {
        if self.restart_pending || matches!(self.status, Status::Installing) {
            return Restart::Busy;
        }
        let (Status::Ready { archive, .. }, Ok((_, installation))) = (&self.status, &self.updater)
        else {
            return Restart::NotReady;
        };
        let restart = Restart::Begin(archive.clone(), installation.clone());
        self.restart_pending = true;
        restart
    }

    fn end_restart(&mut self) {
        self.restart_pending = false;
    }
}

/// Registers the update actions and, in a release build, starts checking.
pub fn init(cx: &mut App) {
    let updater = updater();
    if let Ok((_, installation)) = &updater {
        let installation = installation.clone();
        cx.background_executor()
            .spawn(async move { installation.clean_up() })
            .detach();
    }
    let available = updater.is_ok();
    cx.set_global(Updates {
        status: Status::Idle,
        notice: None,
        restart_pending: false,
        updater,
    });
    cx.on_action(|_: &CheckForUpdates, cx| check(true, cx));
    cx.on_action(|_: &RestartToUpdate, cx| restart_to_update(false, cx));
    if available {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(FIRST_CHECK).await;
            loop {
                if cx.update(|cx| crate::config::get(cx).check_updates != Some(false)) {
                    cx.update(|cx| check(false, cx));
                }
                cx.background_executor().timer(EVERY).await;
            }
        })
        .detach();
    }
}

fn updater() -> Result<(Arc<Updater>, Installation), String> {
    if cfg!(debug_assertions) {
        return Err("Updates are off in development builds.".into());
    }
    let manifest_url = crate::update::manifest_url()
        .ok_or("This build doesn't update itself: it wasn't made by the Release workflow.")?;
    let platform =
        crate::update::platform().ok_or("Updates are not published for this platform.")?;
    let installation =
        Installation::current().map_err(|error| format!("Moth cannot update itself: {error}"))?;
    let updater = Updater::new(
        &manifest_url,
        PUBLIC_KEY,
        env!("CARGO_PKG_VERSION"),
        platform,
    )
    .map_err(|error| error.to_string())?;
    Ok((Arc::new(updater), installation))
}

/// Looks for a newer release and downloads it. `manual` checks (the command) say what they
/// found; automatic ones stay quiet unless an update is ready.
fn check(manual: bool, cx: &mut App) {
    let updates = cx.global::<Updates>();
    let updater = match &updates.updater {
        Ok((updater, _)) => updater.clone(),
        Err(why) => {
            if manual {
                notify(Notice::Info(why.clone()), cx);
            }
            return;
        }
    };
    match &updates.status {
        Status::Idle => {}
        Status::Ready { update, .. } => {
            if manual {
                let version = update.version.to_string();
                notify(Notice::Ready(version), cx);
            }
            return;
        }
        Status::Checking | Status::Available | Status::Downloading { .. } | Status::Installing => {
            if manual {
                notify(
                    Notice::Info("Moth is already updating; this takes a moment.".into()),
                    cx,
                );
            }
            return;
        }
    }
    set_status(Status::Checking, cx);
    cx.spawn(async move |cx| {
        let found = {
            let updater = updater.clone();
            cx.background_executor()
                .spawn(async move { updater.check() })
                .await
        };
        match found {
            Ok(Some(update)) => download(updater, update, manual, cx).await,
            Ok(None) => cx.update(|cx| {
                set_status(Status::Idle, cx);
                if manual {
                    notify(
                        Notice::Info(format!(
                            "Moth {} is the latest version.",
                            env!("CARGO_PKG_VERSION")
                        )),
                        cx,
                    );
                }
            }),
            Err(error) => cx.update(|cx| {
                set_status(Status::Idle, cx);
                report(manual, format!("Couldn't check for updates: {error}"), cx);
            }),
        }
    })
    .detach();
}

/// Downloads and verifies `update` into the cache, showing progress in the status bar, then
/// offers the restart.
async fn download(updater: Arc<Updater>, update: Update, manual: bool, cx: &mut AsyncApp) {
    let version = update.version.to_string();
    cx.update(|cx| set_status(Status::Available, cx));
    let Some(dir) = crate::state::cache_dir().map(|cache| cache.join("updates")) else {
        cx.update(|cx| {
            set_status(Status::Idle, cx);
            report(
                manual,
                format!("Couldn't download Moth {version}: no folder to save it in."),
                cx,
            );
        });
        return;
    };
    let (progress, mut received) = futures::channel::mpsc::unbounded();
    let task = cx.background_executor().spawn(async move {
        // Only the latest download is kept.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let mut last = None;
        let archive = updater.download(&update, &dir, |download| {
            let now = percent(download.received, download.total);
            if now != last {
                last = now;
                let _ = progress.unbounded_send(now);
            }
        })?;
        Ok::<_, crate::update::Error>((update, archive))
    });
    cx.update(|cx| set_status(Status::Downloading { percent: None }, cx));
    while let Some(percent) = received.next().await {
        cx.update(|cx| set_status(Status::Downloading { percent }, cx));
    }
    match task.await {
        Ok((update, archive)) => cx.update(|cx| {
            set_status(Status::Ready { update, archive }, cx);
            notify(Notice::Ready(version), cx);
        }),
        Err(error) => cx.update(|cx| {
            set_status(Status::Idle, cx);
            report(
                manual,
                format!("Couldn't download Moth {version}: {error}"),
                cx,
            );
        }),
    }
}

/// Installs the downloaded update and restarts. If a tab is running something (a command, an
/// SSH session), asks first, unless `force`.
pub fn restart_to_update(force: bool, cx: &mut App) {
    let updates = cx.global::<Updates>();
    let ready = matches!(updates.status, Status::Ready { .. }) && updates.updater.is_ok();
    if ready && !updates.restart_pending && !force {
        let busy = crate::workspace::busy_everywhere(cx);
        if !busy.is_empty() {
            notify(Notice::ConfirmRestart(busy), cx);
            return;
        }
    }
    let (archive, installation) = match cx.global_mut::<Updates>().begin_restart() {
        Restart::Begin(archive, installation) => (archive, installation),
        // The install is already under way.
        Restart::Busy => return,
        Restart::NotReady => {
            notify(
                Notice::Info("No update is ready yet. Check for Updates… looks for one.".into()),
                cx,
            );
            return;
        }
    };
    let ready = std::mem::replace(&mut cx.global_mut::<Updates>().status, Status::Installing);
    cx.global_mut::<Updates>().notice = None;
    cx.refresh_windows();
    cx.spawn(async move |cx| {
        let installed = cx
            .background_executor()
            .spawn(async move { installation.install(&archive) })
            .await;
        cx.update(|cx| match installed {
            // The session is saved as it changes, so the restart reopens the same tabs.
            Ok(path) => {
                cx.set_restart_path(path);
                cx.restart();
            }
            Err(error) => {
                cx.global_mut::<Updates>().end_restart();
                set_status(ready, cx);
                notify(
                    Notice::Error(format!(
                        "Couldn't install the update: {error}. You can download it from the \
                         releases page instead."
                    )),
                    cx,
                );
            }
        });
    })
    .detach();
}

/// The status bar's right corner: the running version, or how far an update has got.
pub fn status_item(cx: &App) -> AnyElement {
    let status = cx
        .try_global::<Updates>()
        .map_or(&Status::Idle, |updates| &updates.status);
    let text = Label::new(status_text(status)).size(LabelSize::XSmall);
    match status {
        Status::Ready { .. } => div()
            .id("restart-to-update")
            .cursor_pointer()
            .child(text.color(Color::Accent))
            .on_click(|_, _, cx| restart_to_update(false, cx))
            .into_any_element(),
        _ => text.color(Color::Placeholder).into_any_element(),
    }
}

fn status_text(status: &Status) -> String {
    match status {
        Status::Idle => format!("v{}", env!("CARGO_PKG_VERSION")),
        Status::Checking => "Checking for updates".into(),
        Status::Available => "New update available".into(),
        Status::Downloading {
            percent: Some(percent),
        } => format!("Downloading update {percent}%"),
        Status::Downloading { .. } => "Downloading update".into(),
        Status::Ready { .. } => "Restart to update".into(),
        Status::Installing => "Installing the update…".into(),
    }
}

/// Whether an update is downloaded and waiting for the restart (for the command palette).
pub fn ready(cx: &App) -> bool {
    cx.try_global::<Updates>()
        .is_some_and(|updates| matches!(updates.status, Status::Ready { .. }))
}

/// The notice above the tabs, in every window, until it's dismissed or acted on.
pub fn render_notice(cx: &App) -> Option<AnyElement> {
    let notice = cx.try_global::<Updates>()?.notice.clone()?;
    let ui = cx.tokens().ui.clone();
    let (text, color, action) = match &notice {
        Notice::Info(text) => (text.clone(), Color::Default, None),
        Notice::Error(text) => (text.clone(), Color::Error, None),
        Notice::Ready(version) => (
            format!("Moth {version} is ready. Restart to finish updating; your tabs reopen."),
            Color::Default,
            Some(("Restart", false)),
        ),
        Notice::ConfirmRestart(busy) => (
            format!("Restarting ends {}. Restart anyway?", busy.join(", ")),
            Color::Warning,
            Some(("Restart Anyway", true)),
        ),
    };
    Some(
        div()
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .py_1p5()
            .bg(ui.surface)
            .border_b_1()
            .border_color(ui.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Label::new(text).color(color)),
            )
            .children(action.map(|(label, force)| {
                Button::new("update-restart", label)
                    .style(ButtonStyle::Filled)
                    .size(ButtonSize::Compact)
                    .on_click(move |_, _, cx| restart_to_update(force, cx))
            }))
            .child(
                Button::new("update-dismiss", "Dismiss")
                    .style(ButtonStyle::Subtle)
                    .size(ButtonSize::Compact)
                    .on_click(|_, _, cx| {
                        cx.global_mut::<Updates>().notice = None;
                        cx.refresh_windows();
                    }),
            )
            .into_any_element(),
    )
}

/// How much of a download has arrived, when its size is known.
fn percent(received: u64, total: Option<u64>) -> Option<u8> {
    let total = total.filter(|total| *total > 0)?;
    Some((received.min(total) * 100 / total) as u8)
}

fn set_status(status: Status, cx: &mut App) {
    cx.global_mut::<Updates>().status = status;
    cx.refresh_windows();
}

fn notify(notice: Notice, cx: &mut App) {
    cx.global_mut::<Updates>().notice = Some(notice);
    cx.refresh_windows();
}

/// A failure the user asked about is shown; one from a background check only goes to the
/// log, since nothing is wrong with what they are doing.
fn report(manual: bool, message: String, cx: &mut App) {
    if manual {
        notify(Notice::Error(message), cx);
    } else {
        crate::log::error(message);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn percent_of_a_download() {
        assert_eq!(percent(0, Some(200)), Some(0));
        assert_eq!(percent(50, Some(200)), Some(25));
        assert_eq!(percent(200, Some(200)), Some(100));
        assert_eq!(percent(300, Some(200)), Some(100));
        assert_eq!(percent(50, None), None);
        assert_eq!(percent(50, Some(0)), None);
    }

    #[test]
    fn the_status_bar_shows_the_version_and_each_update_step() {
        let downloading = |percent| Status::Downloading { percent };
        let current = format!("v{}", env!("CARGO_PKG_VERSION"));
        assert_eq!(status_text(&Status::Idle), current);
        assert_eq!(status_text(&Status::Checking), "Checking for updates");
        assert_eq!(status_text(&Status::Available), "New update available");
        assert_eq!(status_text(&downloading(None)), "Downloading update");
        assert_eq!(
            status_text(&downloading(Some(42))),
            "Downloading update 42%"
        );
        assert_eq!(status_text(&ready()), "Restart to update");
        assert_eq!(status_text(&Status::Installing), "Installing the update…");
    }

    fn ready() -> Status {
        Status::Ready {
            update: Update {
                version: "0.3.0".parse().unwrap(),
                notes_url: String::new(),
                asset: crate::update::Asset {
                    url: String::new(),
                    sha256: String::new(),
                    signature: String::new(),
                },
                file_name: "moth.zip".into(),
            },
            archive: PathBuf::from("/cache/updates/moth.zip"),
        }
    }

    fn updates(status: Status) -> Updates {
        let updater = Updater::new(
            "https://github.com/you/moth/releases/latest/download/latest.json",
            PUBLIC_KEY,
            "0.2.1",
            "linux-x86_64",
        )
        .unwrap();
        let installation = Installation::Binary {
            exe: "/opt/moth/moth".into(),
        };
        Updates {
            status,
            notice: None,
            restart_pending: false,
            updater: Ok((Arc::new(updater), installation)),
        }
    }

    #[test]
    fn a_restart_begins_once_until_it_ends() {
        // Restart clicked twice before the install finishes must not install twice.
        let mut updates = updates(ready());
        assert!(matches!(
            updates.begin_restart(),
            Restart::Begin(archive, _) if archive == Path::new("/cache/updates/moth.zip")
        ));
        assert!(matches!(updates.begin_restart(), Restart::Busy));
        // The install failed: Restart works again.
        updates.end_restart();
        assert!(matches!(updates.begin_restart(), Restart::Begin(..)));
    }

    #[test]
    fn no_restart_without_a_ready_update() {
        assert!(matches!(
            updates(Status::Idle).begin_restart(),
            Restart::NotReady
        ));
        assert!(matches!(
            updates(Status::Installing).begin_restart(),
            Restart::Busy
        ));
        let mut unavailable = updates(ready());
        unavailable.updater = Err("Updates are off in development builds.".into());
        assert!(matches!(unavailable.begin_restart(), Restart::NotReady));
    }

    #[test]
    fn the_public_key_is_a_minisign_key() {
        assert!(PUBLIC_KEY.starts_with("untrusted comment: minisign public key"));
        assert_eq!(PUBLIC_KEY.lines().count(), 2);
    }
}
