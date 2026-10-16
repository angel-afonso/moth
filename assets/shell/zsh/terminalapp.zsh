# Integración de terminalapp con zsh.
#   OSC 133;A/B/C/D   inicio de prompt, fin de prompt, inicio de salida, fin de comando (+ exit code)
#   OSC 7             directorio actual
#   OSC 7777;...      propios: E comando ejecutado, L línea en edición, F fin de edición,
#                     H archivo de historial, P PATH, N nombres de alias/funciones/builtins
[[ -n "$_TAPP_LOADED" ]] && return
typeset -g _TAPP_LOADED=1 _tapp_running= _tapp_first=1
unset TERMINALAPP_SHELL_INTEGRATION
zmodload zsh/parameter 2>/dev/null
autoload -Uz add-zsh-hook add-zle-hook-widget

_tapp_osc() { builtin printf '\e]%s\a' "$1" }

# Reemplaza caracteres de control para que no corten la secuencia OSC.
_tapp_clean() { REPLY=${1//[$'\x00'-$'\x1f'$'\x7f']/ } }

_tapp_precmd() {
  local ret=$?
  if [[ -n "$_tapp_running" ]]; then
    _tapp_osc "133;D;$ret"
    _tapp_running=
  fi
  if [[ -n "$_tapp_first" ]]; then
    _tapp_first=
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
  _tapp_osc "133;A"
  # La marca de fin de prompt se pone ya y además al final de precmd, por si un tema
  # (starship, p10k) rearma PS1 después de nosotros.
  _tapp_mark_prompt
  precmd_functions=(${precmd_functions:#_tapp_mark_prompt} _tapp_mark_prompt)
}

_tapp_mark_prompt() {
  [[ "$PS1" == *$'\e]133;B\a'* ]] || PS1="${PS1}%{"$'\e]133;B\a'"%}"
}

_tapp_preexec() {
  _tapp_running=1
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
