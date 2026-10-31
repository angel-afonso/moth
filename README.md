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
| ⌘⇧T | Reabrir la última pestaña cerrada en su carpeta |
| ⌘+ / ⌘− / ⌘0 | Aumentar / reducir / restablecer el tamaño del texto |
| ⌘F | Buscar en la salida y el scrollback |
| ⌘↑ / ⌘↓ | Ir al comando anterior / siguiente |
| ⌘⇧C | Copiar la salida del comando elegido, o del último |
| ⌘1…⌘8, ⌘9 | Ir a la pestaña N / a la última |
| ⌘⇧] / ⌘⇧[ · ⌃Tab / ⌃⇧Tab | Pestaña siguiente / anterior |
| ⌘K · ⌘⇧P | Paleta de comandos |
| ⌘R | Buscar en el historial (⏎ lo lleva al prompt, ⌘⏎ lo ejecuta) |
| ⌘L | Limpiar pantalla y scrollback |
| ⌘⇧R | Reconectar la sesión SSH que se cortó |
| ⌘V | Pegar (respeta bracketed paste) |
| ⌘C | Copiar la selección (sin selección, la pantalla visible) |
| Tab | Abrir el popup de completions (↑↓ elegir, Tab completar, ⏎ ejecutar, esc cerrar) |
| → / Fin | Aceptar la sugerencia del historial |
| ⌥← / ⌥→ · ⌥⌦ | Palabra anterior / siguiente · borrar la palabra siguiente (en el prompt) |
| ⌘← / ⌘→ · ⌘⌫ | Inicio / fin de la línea · borrar la línea (en el prompt) |

## Teclado

En el prompt, ⌥ y ⌘ editan como en el resto de macOS (mandan `ESC b`/`ESC f` y `⌃A`/`⌃E`/`⌃U`, que entienden zsh, bash y readline). En programas de pantalla completa (vim, less, htop) esas teclas mandan las secuencias estándar de xterm.

Por defecto ⌥+letra escribe el carácter de macOS, porque en teclados como el español ⌥ es la forma de escribir `@`, `#` o `|`. "Usar ⌥ como Meta" en la paleta (⌘K) hace que ⌥+tecla mande `ESC` + tecla (⌥B, ⌥F, ⌥. en zsh); la opción se guarda entre ejecuciones.

## Cerrar pestañas y salir

Si una pestaña tiene un comando en curso o una sesión SSH, ⌘W, ⌘Q, el botón de cerrar de la pestaña o el de la ventana piden confirmación con la lista de lo que se cortaría (⏎ confirma, esc cancela; repetir el atajo también confirma). Requiere la integración de zsh para saber qué está corriendo.

Cuando la shell termina con `exit` o ⌃D, la pestaña se cierra. Si la mata una señal o sale con error apenas abre (un `.zshrc` roto, por ejemplo), la pestaña queda abierta con la salida y un aviso: ⏎ abre una shell nueva en la misma carpeta y ⌘W cierra.

## Avisos, zoom y recuperación de pestañas

Cuando un comando dura al menos 10 segundos y termina en una pestaña inactiva o con la ventana en segundo plano, la app marca la pestaña y muestra un aviso con el comando, su resultado y duración. El aviso se cierra con su botón; son avisos dentro de la app, sin notificaciones del sistema. Requiere la integración de zsh.

El zoom cambia el texto de todas las pestañas, ajusta el tamaño de las celdas y se guarda entre ejecuciones. El rango es de 8 a 32 puntos y ⌘0 restaura 13,5 puntos.

⌘⇧T recupera hasta las últimas 20 carpetas de pestañas cerradas, incluso tras reiniciar la app. Abre una shell nueva: no restaura procesos ni ejecuta de nuevo comandos SSH. Si la carpeta ya no existe, se omite.

## Bloques de comandos

Con la integración de zsh, la salida queda dividida en bloques: un prompt, el comando y lo que imprimió.

- ⌘↑ y ⌘↓ recorren los comandos: el elegido se resalta con una barra de acento y queda arriba de la vista. Pasado el último, ⌘↓ vuelve al final; escribir cualquier cosa quita la selección.
- ⌘⇧C copia la salida del comando elegido o, sin elegir, la del último comando. Copia solo la salida, sin el prompt ni el comando.
- Los comandos que terminaron con error (incluido ⌃C) llevan una barra roja en el margen izquierdo.

Para delimitar los bloques, el escáner del PTY inserta después de cada fin de prompt (`OSC 133;B`) unos caracteres invisibles de ancho cero que alacritty guarda en la celda. Así los bloques siguen al texto al hacer scroll, al cambiar el tamaño de la ventana o cuando el scrollback descarta líneas viejas. Esos caracteres se quitan al copiar y no se dibujan. La integración informa además cuántas filas ocupa el prompt; con temas que arman el prompt en tiempo de ejecución (p10k), ese número puede quedar corto y el bloque incluir la primera fila del prompt siguiente.

## Buscar en la salida

⌘F abre un campo sobre la terminal. Busca texto literal en la pantalla y el scrollback, incluidas las líneas partidas; resalta la coincidencia y desplaza la vista hasta ella. ⏎ busca la siguiente, ⇧⏎ la anterior y Esc cierra el buscador. Los botones ↑ y ↓ también navegan y la búsqueda vuelve al principio o al final al llegar al extremo. Las consultas en minúsculas ignoran mayúsculas; incluir una mayúscula hace que la búsqueda distinga el caso.

## Selección y mouse

Arrastrar selecciona, doble clic selecciona una palabra, triple clic una línea y Shift+clic extiende la selección; ⌘C la copia y escribir la descarta. Cuando un programa pide el mouse (vim, htop, lazygit, Claude Code…), los clics, el arrastre y la rueda le llegan a él en formato SGR o X10; con Shift se selecciona igual.

## Completions

Tab en el prompt abre un popup con lo que se puede escribir donde está el cursor: comandos (PATH, alias, funciones, builtins), subcomandos y flags de herramientas comunes con su descripción, ramas de git, scripts de `package.json`, hosts de `~/.ssh/config`, rutas e historial. El resto del candidato elegido se ve en gris en la línea y un panel muestra su documentación (uso y flags). Si hay un solo candidato se completa directo; si no hay ninguno, el Tab sigue a la completion de zsh. Las especificaciones están en [`assets/completions/specs.json`](assets/completions/specs.json) y se pueden ampliar.

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

Las claves disponibles están en [`assets/themes/graphite.json`](assets/themes/graphite.json). Un color inválido o una clave mal escrita invalida el tema, y el error queda en el log de la app.

## Log y seguridad

Los errores de la app (un tema o `hosts.json` inválidos, una pestaña que no se pudo abrir) se guardan en `~/Library/Logs/terminalapp/terminalapp.log`, además de la salida estándar.

Los programas pueden copiar al portapapeles con OSC 52 (útil para copiar desde vim o tmux en un servidor remoto), pero no leerlo: así un servidor SSH o un archivo con secuencias de escape no pueden llevarse lo que copiaste.

## Integración con zsh

Al abrir una pestaña con zsh, la app instala `assets/shell/zsh/` en `~/Library/Application Support/terminalapp/shell/zsh` y arranca zsh con `ZDOTDIR` apuntando ahí. Ese `.zshenv` restaura tu `ZDOTDIR`, carga tu `.zshenv` y luego la integración; `.zprofile` y `.zshrc` siguen siendo los tuyos.

La integración emite OSC 133 (inicio/fin de prompt y de comando, con exit code; el fin de prompt lleva también las filas del prompt y si falló el comando anterior), OSC 7 (directorio actual) y OSC 7777, propios: la línea en edición en cada tecla, el comando ejecutado, `HISTFILE`, `PATH` y los nombres de alias, funciones y builtins. La app los detecta en la salida del PTY antes de pasársela a alacritty y con eso:

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
- `src/state.rs`: última carpeta, carpetas recientes, tema, zoom y ⌥ como Meta.
- `src/log.rs`: errores a stderr y a `~/Library/Logs/terminalapp/`.
- `src/glyphs.rs`: bloques (`▀▄█▐▛▜`…) y líneas de caja (`─│╭╮`…) dibujados como rectángulos que llenan la celda, para que logos como el de Claude Code y los bordes de TUIs no queden con huecos.
- `src/blocks.rs`: bloques de comandos: marcas invisibles en la grilla, salida de cada comando y resultado.
- `src/completions.rs`: motor de completions (interpreta la línea, elige la fuente, rankea y aplica).
- `src/mouse.rs`: reporte de mouse a los programas que lo piden (SGR y X10).
- `src/links.rs`: búsqueda de URLs en la salida y esquemas permitidos.
- `src/keys.rs`: teclas especiales → secuencias de escape.
- `src/tokens.rs`: tokens de diseño, carga de temas JSON con herencia, métricas de espaciado y radios.
- `src/theme.rs`: aplica el tema activo a la app y al sistema de temas de Zed (para los componentes de `ui`), cambio en caliente.
- `src/components.rs`: componentes del diseño: `TabItem`, `Kbd`, `KindBadge`, `StatusDot`, `BlockStatus`.
- `src/palette.rs`: colores de la grilla (ANSI, 256, truecolor) resueltos con los tokens.

## Licencias

El crate `ui` de Zed es GPL-3.0-or-later, por eso este proyecto también lo es. Las fuentes Geist (`assets/fonts`) son OFL; los íconos de `assets/icons` vienen del repo de Zed.
