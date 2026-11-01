mod assets;
mod blocks;
mod completions;
mod components;
mod config;
mod editor;
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
mod session;
mod shell;
mod ssh;
mod state;
mod terminal;
mod terminal_view;
mod text_input;
mod theme;
mod tokens;
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
            config::init(cx);
            theme::init(cx);
            workspace::init(cx);
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
            name: "terminalapp".into(),
            items: vec![
                MenuItem::action("Abrir configuración", ws::OpenConfig),
                MenuItem::separator(),
                MenuItem::action("Salir de terminalapp", ws::Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "Shell".into(),
            items: vec![
                MenuItem::action("Nueva ventana", ws::NewWindow),
                MenuItem::action("Nueva pestaña", ws::NewTab),
                MenuItem::action("Reabrir pestaña cerrada", ws::ReopenTab),
                MenuItem::separator(),
                MenuItem::action("Dividir a la derecha", ws::SplitRight),
                MenuItem::action("Dividir hacia abajo", ws::SplitDown),
                MenuItem::separator(),
                MenuItem::action("Cerrar", ws::CloseTab),
            ],
            disabled: false,
        },
        Menu {
            name: "Edición".into(),
            items: vec![
                MenuItem::action("Copiar", view::Copy),
                MenuItem::action("Pegar", view::Paste),
                MenuItem::action("Copiar la salida del comando", view::CopyCommandOutput),
                MenuItem::separator(),
                MenuItem::action("Buscar…", view::FindOutput),
                MenuItem::action("Limpiar pantalla", ws::ClearScreen),
            ],
            disabled: false,
        },
        Menu {
            name: "Ventana".into(),
            items: vec![
                MenuItem::action("Minimizar", ws::Minimize),
                MenuItem::action("Zoom", ws::ZoomWindow),
                MenuItem::separator(),
                MenuItem::action("Pestaña siguiente", ws::NextTab),
                MenuItem::action("Pestaña anterior", ws::PreviousTab),
                MenuItem::action("Panel siguiente", ws::NextPane),
                MenuItem::action("Panel anterior", ws::PreviousPane),
                MenuItem::action("Maximizar o restaurar el panel", ws::TogglePaneZoom),
            ],
            disabled: false,
        },
    ]
}
