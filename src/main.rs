mod agent;
mod agent_mode;
mod agent_panel;
mod assets;
mod blocks;
mod completions;
mod components;
mod config;
mod editor;
mod fixes;
mod fuzzy;
mod git;
mod glyphs;
mod highlight;
mod hosts;
mod keys;
mod links;
mod log;
mod mouse;
mod notify;
mod palette;
mod panes;
mod picker;
mod process;
mod redact;
mod session;
mod shell;
mod shortcuts;
mod ssh;
mod state;
mod terminal;
mod terminal_view;
mod text_input;
mod theme;
mod tokens;
mod update;
mod updates;
mod workspace;

use gpui::{App, Menu, MenuItem};

use crate::assets::Assets;

fn main() {
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            if let Err(err) = Assets.load_fonts(cx) {
                crate::log::error(format!("{err:#}"));
            }
            log::init();
            state::migrate_legacy_dir();
            config::init(cx);
            theme::init(cx);
            session::init(cx);
            workspace::init(cx);
            updates::init(cx);
            cx.set_menus(menus());
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            workspace::open_initial_windows(cx);
            cx.activate(true);
        });
}

fn menus() -> Vec<Menu> {
    use crate::{terminal_view as view, workspace as ws};
    vec![
        Menu {
            name: "Moth".into(),
            items: vec![
                MenuItem::action("Check for Updates…", crate::updates::CheckForUpdates),
                MenuItem::action("Open Settings", ws::OpenConfig),
                MenuItem::separator(),
                MenuItem::action("Quit Moth", ws::Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "Shell".into(),
            items: vec![
                MenuItem::action("New Window", ws::NewWindow),
                MenuItem::action("New Tab", ws::NewTab),
                MenuItem::action("Reopen Closed Tab", ws::ReopenTab),
                MenuItem::separator(),
                MenuItem::action("Split Right", ws::SplitRight),
                MenuItem::action("Split Down", ws::SplitDown),
                MenuItem::separator(),
                MenuItem::action("Close", ws::CloseTab),
            ],
            disabled: false,
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Copy", view::Copy),
                MenuItem::action("Paste", view::Paste),
                MenuItem::action("Copy Command Output", view::CopyCommandOutput),
                MenuItem::separator(),
                MenuItem::action("Ask Agent…", view::AskAgent),
                MenuItem::action("Fix Last Command with Agent", view::FixWithAgent),
                MenuItem::action("Agent Mode", view::ToggleAgentMode),
                MenuItem::separator(),
                MenuItem::action("Find…", view::FindOutput),
                MenuItem::action("Clear Screen", ws::ClearScreen),
            ],
            disabled: false,
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", ws::Minimize),
                MenuItem::action("Zoom", ws::ZoomWindow),
                MenuItem::separator(),
                MenuItem::action("Next Tab", ws::NextTab),
                MenuItem::action("Previous Tab", ws::PreviousTab),
                MenuItem::action("Next Pane", ws::NextPane),
                MenuItem::action("Previous Pane", ws::PreviousPane),
                MenuItem::action("Maximize or Restore Pane", ws::TogglePaneZoom),
            ],
            disabled: false,
        },
    ]
}
