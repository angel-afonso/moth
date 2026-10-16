# terminalapp

Terminal para macOS hecha con [gpui](https://github.com/zed-industries/zed/tree/main/crates/gpui) y los componentes del crate `ui` de Zed. Usa el emulador de alacritty (el mismo fork que usa Zed).

```bash
cargo run
```

Requiere Rust 1.99 (fijado en `rust-toolchain.toml`). Los shaders de Metal se compilan al iniciar (`runtime_shaders`), así que no hace falta el Metal Toolchain de Xcode. Para builds de release conviene instalarlo (`xcodebuild -downloadComponent MetalToolchain`) y quitar esa feature.

## Atajos

| Atajo | Acción |
| --- | --- |
| ⌘T | Nueva pestaña |
| ⌘W | Cerrar pestaña |
| ⌘1…⌘8, ⌘9 | Ir a la pestaña N / a la última |
| ⌘⇧] / ⌘⇧[ · ⌃Tab / ⌃⇧Tab | Pestaña siguiente / anterior |
| ⌘V | Pegar (respeta bracketed paste) |
| ⌘C | Copiar la pantalla visible (todavía no hay selección) |
| → / Fin | Aceptar la sugerencia del historial |

## Integración con zsh

Al abrir una pestaña con zsh, la app instala `assets/shell/zsh/` en `~/Library/Application Support/terminalapp/shell/zsh` y arranca zsh con `ZDOTDIR` apuntando ahí. Ese `.zshenv` restaura tu `ZDOTDIR`, carga tu `.zshenv` y luego la integración; `.zprofile` y `.zshrc` siguen siendo los tuyos.

La integración emite OSC 133 (inicio/fin de prompt y de comando, con exit code), OSC 7 (directorio actual) y OSC 7777, propios: la línea en edición en cada tecla, el comando ejecutado, `HISTFILE`, `PATH` y los nombres de alias, funciones y builtins. La app los detecta en la salida del PTY antes de pasársela a alacritty y con eso:

- colorea la línea que estás escribiendo (comando, subcomando, flags, strings, rutas, operadores, variables; comando inexistente en rojo ondulado);
- sugiere en gris el resto del comando más reciente del historial que empieza igual; → o Fin lo acepta;
- muestra en la pestaña el comando en ejecución o la carpeta, y en la barra de estado la ruta, la rama de git y el resultado y la duración del último comando.

Con otras shells la terminal funciona igual, sin estas funciones.

## Estructura

- `src/workspace.rs`: ventana, `TabBar`/`Tab` de Zed, barra de estado, acciones.
- `src/terminal.rs`: PTY + `alacritty_terminal`, eventos hacia gpui.
- `src/terminal_view.rs`: pinta la grilla con el text system de gpui, cursor, IME.
- `src/shell.rs`: instalación del script de zsh, detección de OSC en el PTY, historial, rama de git.
- `src/highlight.rs`: lexer de la línea de comandos para el coloreado.
- `src/keys.rs`: teclas especiales → secuencias de escape.
- `src/theme.rs`, `src/palette.rs`: tema "Graphite" sobre el sistema de temas de Zed.

## Licencias

El crate `ui` de Zed es GPL-3.0-or-later, por eso este proyecto también lo es. Las fuentes Geist (`assets/fonts`) son OFL; los íconos de `assets/icons` vienen del repo de Zed.
