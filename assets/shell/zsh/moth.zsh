# Integración de Moth con zsh.
#   OSC 133;A/B/C/D   inicio de prompt, fin de prompt, inicio de salida, fin de comando (+ exit code)
#                     B lleva rows=N (filas del prompt) y prev_err=0|1 (falló el comando anterior)
#   OSC 7             directorio actual
#   OSC 7777;...      propios: E comando ejecutado, L línea en edición, F fin de edición,
#                     H archivo de historial, P PATH, N nombres de alias/funciones/builtins
#
# Si el prompt es el de macOS por defecto, se reemplaza por uno con el estilo de la app
# (encabezado con carpeta, rama y resultado del comando anterior; `❯` debajo). Con un tema
# propio (starship, p10k…) o con MOTH_PROMPT=0 en tu .zshrc, no se toca.
[[ -n "$_MOTH_LOADED" ]] && return
typeset -g _MOTH_LOADED=1 _moth_running= _moth_first=1 _moth_own_prompt= _moth_started= _moth_prev_err=0
unset MOTH_SHELL_INTEGRATION
zmodload zsh/parameter 2>/dev/null
zmodload zsh/datetime 2>/dev/null
autoload -Uz add-zsh-hook add-zle-hook-widget

_moth_osc() { builtin printf '\e]%s\a' "$1" }

# Reemplaza caracteres de control para que no corten la secuencia OSC.
_moth_clean() { REPLY=${1//[$'\x00'-$'\x1f'$'\x7f']/ } }

_moth_precmd() {
  local ret=$? ran=$_moth_running
  _moth_prev_err=0
  [[ -n "$ran" ]] && (( ret != 0 )) && _moth_prev_err=1
  if [[ -n "$_moth_running" ]]; then
    _moth_osc "133;D;$ret"
    _moth_running=
  fi
  if [[ -n "$_moth_first" ]]; then
    _moth_first=
    # Para entonces ya se cargó el .zshrc: se ve si el usuario tiene su propio prompt.
    if [[ "$MOTH_PROMPT" != 0 && ( "$PS1" == "%n@%m %1~ %# " || "$PS1" == "%m%# " ) ]]; then
      _moth_own_prompt=1
    fi
    # Los hooks de ZLE se registran recién aquí: desde .zshenv ZLE todavía no está listo.
    add-zle-hook-widget line-init _moth_report_line
    add-zle-hook-widget line-pre-redraw _moth_report_line
    add-zle-hook-widget line-finish _moth_line_finish
    _moth_osc "7777;H;${HISTFILE:-$HOME/.zsh_history}"
    _moth_osc "7777;P;$PATH"
    _moth_osc "7777;N;${(k)aliases} ${(k)galiases} ${(k)functions} ${(k)builtins} ${(k)reswords}"
  fi
  local p=${PWD//\%/%25}
  p=${p// /%20}
  _moth_osc "7;file://${HOST:-localhost}$p"
  [[ -n "$_moth_own_prompt" ]] && _moth_build_prompt "$ret" "$ran"
  _moth_osc "133;A"
  # La marca de fin de prompt se pone ya y además al final de precmd, por si un tema
  # (starship, p10k) rearma PS1 después de nosotros.
  _moth_mark_prompt
  precmd_functions=(${precmd_functions:#_moth_mark_prompt} _moth_mark_prompt)
}

_moth_format_duration() {
  local -F seconds=$1
  local -i ms=$(( seconds * 1000 ))
  if (( ms < 1000 )); then
    REPLY="${ms}ms"
  elif (( ms < 60000 )); then
    builtin printf -v REPLY '%.1fs' $seconds
  else
    REPLY="$(( ms / 60000 ))m $(( (ms / 1000) % 60 ))s"
  fi
}

# Encabezado tenue y `❯` debajo. Usa colores ANSI, que la app resuelve con el tema activo:
# white = texto secundario, bright black = metadatos, magenta = rama, green/red = estado.
#   ~/work/moth  main  ✓ 41.2s · 14:02
#   ❯
_moth_build_prompt() {
  local ret=$1 ran=$2 branch
  # Rutas largas: solo las últimas tres carpetas.
  local header="%F{7}%(5~|…/%3~|%~)%f"
  branch=$(command git symbolic-ref --short -q HEAD 2>/dev/null) \
    || branch=$(command git rev-parse --short HEAD 2>/dev/null)
  if [[ -n "$branch" ]]; then
    header+="  %F{5}${branch//\%/%%}%f"
  fi
  local meta="%D{%H:%M}"
  if [[ -n "$ran" && -n "$_moth_started" ]]; then
    _moth_format_duration $(( EPOCHREALTIME - _moth_started ))
    if (( ret == 0 )); then
      meta="%F{2}✓ $REPLY%f%F{8} · $meta"
    else
      meta="%F{1}✕ exit $ret · $REPLY%f%F{8} · $meta"
    fi
  fi
  header+="  %F{8}$meta%f"
  local mark=2
  [[ -n "$ran" ]] && (( ret != 0 )) && mark=1
  # Tras un comando, una línea en blanco lo separa del siguiente bloque.
  local gap=""
  [[ -n "$ran" ]] && gap=$'\n'
  PS1="$gap$header"$'\n'"%F{$mark}❯%f "
}

# Agrega al final de PS1 la marca de fin de prompt (reemplaza la del prompt anterior, que puede
# seguir ahí si PS1 no se rearma). La app la usa para delimitar los bloques de comandos, así
# que lleva las filas que ocupa el prompt y si falló el comando anterior.
_moth_mark_prompt() {
  local base=${PS1%"%{"$'\e]133;B'*}
  local expanded=${(%)base}
  local newlines=${expanded//[^$'\n']/}
  PS1="${base}%{"$'\e]133;B;rows='"$(( ${#newlines} + 1 ));prev_err=$_moth_prev_err"$'\a'"%}"
}

_moth_preexec() {
  _moth_running=1
  _moth_started=$EPOCHREALTIME
  _moth_clean "$1"
  _moth_osc "7777;E;$REPLY"
  _moth_osc "133;C"
}

_moth_report_line() {
  _moth_clean "$BUFFER"
  _moth_osc "7777;L;$CURSOR;$REPLY"
}

_moth_line_finish() { _moth_osc "7777;F" }

precmd_functions=(_moth_precmd ${precmd_functions:#_moth_precmd})
add-zsh-hook preexec _moth_preexec
