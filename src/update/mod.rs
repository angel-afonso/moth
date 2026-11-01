//! Moth's updater, with no UI dependencies: reads the release's
//! `latest.json`, decides whether it is newer than the running app, and
//! downloads the archive for this platform, accepting it only when its
//! SHA-256 and its minisign signature (made by the release workflow, see
//! `.github/scripts/update-manifest.sh`) both match, and replaces the
//! installed app with it.
//!
//! Everything here blocks; the app calls it from a background thread. `updates.rs` is the
//! app's side: the schedule, the status bar and the restart.

mod client;
mod error;
mod install;
mod manifest;
mod verify;

pub use client::Updater;
pub use error::Error;
pub use install::Installation;
#[cfg(test)]
pub use manifest::Asset;
pub use manifest::{Manifest, Update, platform};
pub use verify::verify_file;

/// The manifest the app reads: always the newest release's. The repository (`owner/name`)
/// comes from `MOTH_REPOSITORY` at build time, which the Release workflow sets; builds without
/// it, such as local ones, don't update themselves.
pub fn manifest_url() -> Option<String> {
    let repository = option_env!("MOTH_REPOSITORY").filter(|repo| !repo.is_empty())?;
    Some(format!(
        "https://github.com/{repository}/releases/latest/download/latest.json"
    ))
}
