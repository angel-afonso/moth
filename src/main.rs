mod assets;
mod highlight;
mod keys;
mod palette;
mod shell;
mod terminal;
mod terminal_view;
mod theme;
mod workspace;

use gpui::{
    App, AppContext as _, Bounds, Menu, MenuItem, TitlebarOptions, WindowBounds, WindowOptions,
    point, px, size,
};

use crate::{assets::Assets, workspace::Workspace};

fn main() {
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            if let Err(err) = Assets.load_fonts(cx) {
                eprintln!("{err:#}");
            }
            theme::init(cx);
            workspace::init(cx);
            cx.set_menus([
                Menu {
                    name: "terminalapp".into(),
                    items: vec![MenuItem::action("Salir de terminalapp", workspace::Quit)],
                    disabled: false,
                },
                Menu {
                    name: "Shell".into(),
                    items: vec![
                        MenuItem::action("Nueva pestaña", workspace::NewTab),
                        MenuItem::separator(),
                        MenuItem::action("Cerrar pestaña", workspace::CloseTab),
                    ],
                    disabled: false,
                },
            ]);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("terminalapp".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(12.), px(10.))),
                    }),
                    window_min_size: Some(size(px(480.), px(280.))),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Workspace::new(window, cx)),
            )
            .expect("no se pudo abrir la ventana");
            cx.activate(true);
        });
}
