# terminalapp

Terminal para macOS hecha con [gpui](https://github.com/zed-industries/zed/tree/main/crates/gpui) y los componentes del crate `ui` de Zed. Usa el emulador de alacritty (el mismo fork que usa Zed).

```bash
cargo run
```

Requiere Rust 1.99 (fijado en `rust-toolchain.toml`). Los shaders de Metal se compilan al iniciar (`runtime_shaders`), así que no hace falta el Metal Toolchain de Xcode. Para builds de release conviene instalarlo (`xcodebuild -downloadComponent MetalToolchain`) y quitar esa feature.

La app abre en la última carpeta que usaste (se guarda en `~/Library/Application Support/terminalapp/last_cwd`) y cada pestaña nueva abre en la carpeta de la pestaña activa. La shell se lanza directa como shell de login (`$SHELL -l`), sin pasar por `/usr/bin/login`, así que no aparece el "Last login: …".

## Atajos

| Atajo | Acción |
| --- | --- |
| ⌘T | Nueva pestaña |
| ⌘W · clic central | Cerrar pestaña |
| ⌘1…⌘8, ⌘9 | Ir a la pestaña N / a la última |
| ⌘⇧] / ⌘⇧[ · ⌃Tab / ⌃⇧Tab | Pestaña siguiente / anterior |
| ⌘K · ⌘⇧P | Paleta de comandos |
| ⌘R | Buscar en el historial (⏎ lo lleva al prompt, ⌘⏎ lo ejecuta) |
| ⌘L | Limpiar pantalla y scrollback |
| ⌘⇧R | Reconectar la sesión SSH que se cortó |
| ⌘V | Pegar (respeta bracketed paste) |
| ⌘C | Copiar la pantalla visible (todavía no hay selección) |
| → / Fin | Aceptar la sugerencia del historial |

## Links

Las URLs de la salida (`http`, `https`, `ftp`, `file`, `mailto`), incluso las que se parten en dos filas, y los hipervínculos OSC 8 (`ls --hyperlink`, compiladores) se subrayan al pasar el mouse y se abren en el navegador predeterminado con un clic. Se abren al soltar el botón sobre el mismo link, y el clic que solo activa la ventana no abre nada. En la línea que estás escribiendo el clic sigue moviendo el cursor. Por seguridad no se abren otros esquemas, aunque lleguen por OSC 8.

## Paleta de comandos e historial

⌘K abre la paleta: acciones (con sus atajos), las pestañas abiertas y las carpetas recientes, que abren en una pestaña nueva. ⌘R abre la búsqueda en el historial de zsh: arranca con lo que ya estaba escrito en el prompt, filtra con coincidencia difusa y resalta los caracteres que coinciden. ⏎ lleva el comando al prompt para editarlo y ⌘⏎ lo ejecuta. Clic en la rama de la barra de estado (o "Cambiar de rama…" en la paleta) abre el mismo buscador con las ramas locales.

## Sesiones SSH

Cuando ejecutas `ssh` (con la integración de zsh), la app reconoce el destino (`user@host`, `-p`, `-J`, `-l`, `ssh://…`) y lo resuelve con `ssh -G`, que aplica tu `~/.ssh/config` sin conectarse. Mientras dure la sesión:

- la pestaña muestra el host con un ícono ⇄, su chip (`PROD`, `STG`…) y la línea superior en el color del host;
- la barra de estado cambia a `usuario@host`, `hostname:puerto`, la carpeta remota (si el servidor la pone en el título, como hacen Debian y Ubuntu) y el tiempo conectado;
- si el host es de producción, aparece una franja bajo las pestañas, un borde alrededor de la terminal, y pegar varias líneas pide confirmación (esc cancela, ⌘⏎ pega igual).

Si `ssh` termina con exit 255 (la conexión se cortó), la pestaña queda tachada con un botón para reconectar (también ⌘⇧R o desde la paleta). "Conectar a…" en ⌘K lista las sesiones cortadas, los hosts de `~/.ssh/config` (con sus `Include`) y los `ssh` del historial; ⏎ abre una pestaña nueva y ⌘⏎ conecta en la actual.

La identidad de cada host se define en `~/Library/Application Support/terminalapp/hosts.json`. Cada regla se compara (glob con `*` y `?`) contra el alias y contra el hostname real; gana la primera que coincide:

```json
{
  "hosts": [
    { "match": "prod-*", "label": "PROD", "color": "red", "production": true },
    { "match": "*.staging.internal", "label": "STG", "color": "cyan" }
  ]
}
```

`color` acepta colores ANSI del tema (`red`, `green`, `yellow`, `blue`, `magenta`, `cyan`) o un hex. Sin ese archivo se usan reglas por defecto: `*prod*`, `*production*` y `*live*` como producción, y `*staging*` y `*stg*` como STG. Si creas el archivo, reemplaza esas reglas. Los hosts sin regla reciben un color estable según su nombre.

## Temas

Los colores salen de tokens (los de la hoja "Tokens y componentes" del diseño): interfaz, estado, sintaxis de la línea de comandos y los 16 colores ANSI. Vienen dos temas, **Graphite** (oscuro) y **Graphite Light**. "Cambiar tema…" en la paleta (⌘K) abre el selector: con ↑↓ se ve una vista previa en vivo, ⏎ lo guarda y esc vuelve al anterior. El cambio recolorea también la salida que ya está en pantalla y el prompt, porque usan colores ANSI que se resuelven con el tema activo.

Para crear un tema, agrega un `.json` en `~/Library/Application Support/terminalapp/themes/`. Hereda de `base` (Graphite si no se indica) y basta con los colores que cambian; el selector relee la carpeta cada vez que se abre:

```json
{
  "name": "Graphite Naranja",
  "base": "Graphite",
  "ui": { "accent": "#FF9F43" },
  "syntax": { "command": "#FF9F43" }
}
```

Las claves disponibles están en [`assets/themes/graphite.json`](assets/themes/graphite.json). Un color inválido o una clave mal escrita invalida el tema, y el error aparece en la salida de la app.

## Integración con zsh

Al abrir una pestaña con zsh, la app instala `assets/shell/zsh/` en `~/Library/Application Support/terminalapp/shell/zsh` y arranca zsh con `ZDOTDIR` apuntando ahí. Ese `.zshenv` restaura tu `ZDOTDIR`, carga tu `.zshenv` y luego la integración; `.zprofile` y `.zshrc` siguen siendo los tuyos.

La integración emite OSC 133 (inicio/fin de prompt y de comando, con exit code), OSC 7 (directorio actual) y OSC 7777, propios: la línea en edición en cada tecla, el comando ejecutado, `HISTFILE`, `PATH` y los nombres de alias, funciones y builtins. La app los detecta en la salida del PTY antes de pasársela a alacritty y con eso:

- colorea la línea que estás escribiendo (comando, subcomando, flags, strings, rutas, operadores, variables; comando inexistente en rojo ondulado);
- sugiere en gris el resto del comando más reciente del historial que empieza igual; → o Fin lo acepta;
- un clic sobre la línea que estás escribiendo lleva el cursor ahí, como en un editor (también en líneas partidas y con caracteres anchos);
- muestra en la pestaña el comando en ejecución o la carpeta, y en la barra de estado la ruta y el resultado y la duración del último comando;
- muestra el estado de git en cada prompt (rama, `↓`/`↑` respecto del upstream, `+` en stage, `~` modificados, `?` sin seguimiento, `!` en conflicto). Elegir una rama en el buscador escribe y ejecuta `git switch` en la shell, así el resultado o cualquier error queda a la vista. Solo se puede cambiar con la shell en el prompt, no con un comando en curso.

Si usas el prompt de zsh por defecto de macOS, la integración lo reemplaza por uno con el estilo de la app: un encabezado tenue con la carpeta (las últimas tres si la ruta es larga), la rama, el resultado y la duración del comando anterior y la hora; debajo, `❯` en verde, o en rojo si el comando falló, con una línea en blanco entre bloques. Si tienes tu propio prompt (starship, p10k…) no se toca; para desactivarlo igual, pon `TERMINALAPP_PROMPT=0` en tu `.zshrc`.

Si la app no recibe locale (al abrirla desde el Finder), le pasa a la shell un `LANG` UTF-8 según el idioma de macOS, para que los acentos y la ñ funcionen.

Con otras shells la terminal funciona igual, sin estas funciones.

## Estructura

- `src/workspace.rs`: ventana, barra de pestañas, barra de estado con el selector de ramas, acciones.
- `src/terminal.rs`: PTY + `alacritty_terminal`, eventos hacia gpui.
- `src/terminal_view.rs`: pinta la grilla con el text system de gpui, cursor, IME.
- `src/shell.rs`: instalación del script de zsh, detección de OSC en el PTY, historial.
- `src/ssh.rs`: reconoce el comando `ssh`, lo resuelve con `ssh -G` y lee `~/.ssh/config`.
- `src/hosts.rs`: reglas de `hosts.json` (etiqueta, color, producción).
- `src/git.rs`: estado del repo (`git status --porcelain=v2`), lista de ramas, comando para cambiar.
- `src/highlight.rs`: lexer de la línea de comandos para el coloreado.
- `src/picker.rs`: buscador en overlay (lista virtualizada, secciones, teclado) que usan la paleta, el historial y las ramas.
- `src/text_input.rs`: campo de texto de una línea con IME (el crate `ui` no trae uno).
- `src/fuzzy.rs`: coincidencia difusa con puntaje y posiciones para resaltar.
- `src/state.rs`: última carpeta y carpetas recientes.
- `src/links.rs`: búsqueda de URLs en la salida y esquemas permitidos.
- `src/keys.rs`: teclas especiales → secuencias de escape.
- `src/tokens.rs`: tokens de diseño, carga de temas JSON con herencia, métricas de espaciado y radios.
- `src/theme.rs`: aplica el tema activo a la app y al sistema de temas de Zed (para los componentes de `ui`), cambio en caliente.
- `src/components.rs`: componentes del diseño: `TabItem`, `Kbd`, `KindBadge`, `StatusDot`, `BlockStatus`.
- `src/palette.rs`: colores de la grilla (ANSI, 256, truecolor) resueltos con los tokens.

## Licencias

El crate `ui` de Zed es GPL-3.0-or-later, por eso este proyecto también lo es. Las fuentes Geist (`assets/fonts`) son OFL; los íconos de `assets/icons` vienen del repo de Zed.
