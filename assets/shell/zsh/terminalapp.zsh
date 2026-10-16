# Integración de terminalapp con zsh.
#   OSC 133;A/B/C/D   inicio de prompt, fin de prompt, inicio de salida, fin de comando (+ exit code)
#   OSC 7             directorio actual
#   OSC 7777;...      propios: E comando ejecutado, L línea en edición, F fin de edición,
#                     H archivo de historial, P PATH, N nombres de alias/funciones/builtins
#
# Si el prompt es el de macOS por defecto, se reemplaza por uno con el estilo de la app
# (encabezado con carpeta, rama y resultado del comando anterior; `❯` debajo). Con un tema
# propio (starship, p10k…) o con TERMINALAPP_PROMPT=0 en tu .zshrc, no se toca.
[[ -n "$_TAPP_LOADED" ]] && return
typeset -g _TAPP_LOADED=1 _tapp_running= _tapp_first=1 _tapp_own_prompt= _tapp_started=
unset TERMINALAPP_SHELL_INTEGRATION
zmodload zsh/parameter 2>/dev/null
zmodload zsh/datetime 2>/dev/null
autoload -Uz add-zsh-hook add-zle-hook-widget

_tapp_osc() { builtin printf '\e]%s\a' "$1" }

# Reemplaza caracteres de control para que no corten la secuencia OSC.
_tapp_clean() { REPLY=${1//[$'\x00'-$'\x1f'$'\x7f']/ } }

_tapp_precmd() {
  local ret=$? ran=$_tapp_running
  if [[ -n "$_tapp_running" ]]; then
    _tapp_osc "133;D;$ret"
    _tapp_running=
  fi
  if [[ -n "$_tapp_first" ]]; then
    _tapp_first=
    # Para entonces ya se cargó el .zshrc: se ve si el usuario tiene su propio prompt.
    if [[ "$TERMINALAPP_PROMPT" != 0 && ( "$PS1" == "%n@%m %1~ %# " || "$PS1" == "%m%# " ) ]]; then
      _tapp_own_prompt=1
    fi
    # Los hooks de ZLE se registran recién aquí: desde .zshenv ZLE todavía no está listo.
    add-zle-hook-widget line-init _tapp_report_line
    add-zle-hook-widget line-pre-redraw _tapp_report_line
    add-zle-hook-widget line-finish _tapp_line_finish
    _tapp_osc "7777;H;${HISTFILE:-$HOME/.zsh_history}"
    _tapp_osc "7777;P;$PATH"
    _tapp_osc "7777;N;${(k)aliases} ${(k)galiases} ${(k)functions} ${(k)builtins} ${(k)reswords}"
  fi
  local p=${PWD//\%/%25}
  p=${p// /%20}
  _tapp_osc "7;file://${HOST:-localhost}$p"
  [[ -n "$_tapp_own_prompt" ]] && _tapp_build_prompt "$ret" "$ran"
  _tapp_osc "133;A"
  # La marca de fin de prompt se pone ya y además al final de precmd, por si un tema
  # (starship, p10k) rearma PS1 después de nosotros.
  _tapp_mark_prompt
  precmd_functions=(${precmd_functions:#_tapp_mark_prompt} _tapp_mark_prompt)
}

_tapp_format_duration() {
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

# Encabezado tenue y `❯` debajo, con los colores de la paleta Graphite (truecolor).
#   ~/work/terminalapp  main  ✓ 41.2s · 14:02
#   ❯
_tapp_build_prompt() {
  local ret=$1 ran=$2 branch
  # Rutas largas: solo las últimas tres carpetas.
  local header="%F{#A3AAB4}%(5~|…/%3~|%~)%f"
  branch=$(command git symbolic-ref --short -q HEAD 2>/dev/null) \
    || branch=$(command git rev-parse --short HEAD 2>/dev/null)
  if [[ -n "$branch" ]]; then
    header+="  %F{#C3A6FF}${branch//\%/%%}%f"
  fi
  local meta="%D{%H:%M}"
  if [[ -n "$ran" && -n "$_tapp_started" ]]; then
    _tapp_format_duration $(( EPOCHREALTIME - _tapp_started ))
    if (( ret == 0 )); then
      meta="%F{#7EE0A1}✓ $REPLY%f%F{#7D8590} · $meta"
    else
      meta="%F{#FF7B72}✕ exit $ret · $REPLY%f%F{#7D8590} · $meta"
    fi
  fi
  header+="  %F{#7D8590}$meta%f"
  local mark="#7EE0A1"
  [[ -n "$ran" ]] && (( ret != 0 )) && mark="#FF7B72"
  # Tras un comando, una línea en blanco lo separa del siguiente bloque.
  local gap=""
  [[ -n "$ran" ]] && gap=$'\n'
  PS1="$gap$header"$'\n'"%F{$mark}❯%f "
}

_tapp_mark_prompt() {
  [[ "$PS1" == *$'\e]133;B\a'* ]] || PS1="${PS1}%{"$'\e]133;B\a'"%}"
}

_tapp_preexec() {
  _tapp_running=1
  _tapp_started=$EPOCHREALTIME
  _tapp_clean "$1"
  _tapp_osc "7777;E;$REPLY"
  _tapp_osc "133;C"
}

_tapp_report_line() {
  _tapp_clean "$BUFFER"
  _tapp_osc "7777;L;$CURSOR;$REPLY"
}

_tapp_line_finish() { _tapp_osc "7777;F" }

precmd_functions=(_tapp_precmd ${precmd_functions:#_tapp_precmd})
add-zsh-hook preexec _tapp_preexec
