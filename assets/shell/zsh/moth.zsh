# Moth's zsh integration.
#   OSC 133;A/B/C/D   prompt start, prompt end, output start, command end (+ exit code)
#                     B carries rows=N (prompt height) and prev_err=0|1 (previous command failed)
#   OSC 7             current directory
#   OSC 7777;...      custom: E command run, L line being edited, F editing finished,
#                     H history file, P PATH, N alias/function/builtin names
# Each one goes as OSC 7777;<token>;<sequence>, with the token the app passes in MOTH_TOKEN:
# the app ignores sequences without it, so output that prints them can't pose as the shell.
#
# If the prompt is the macOS default, it's replaced with one in the app's style (a header
# with folder, branch and previous command result; `❯` below it). With your own theme
# (starship, p10k…) or MOTH_PROMPT=0 in your .zshrc, it's left alone.
[[ -n "$_MOTH_LOADED" ]] && return
typeset -g _MOTH_LOADED=1 _moth_running= _moth_first=1 _moth_own_prompt= _moth_started= _moth_prev_err=0
typeset -gH _moth_token=$MOTH_TOKEN
unset MOTH_SHELL_INTEGRATION MOTH_TOKEN
zmodload zsh/parameter 2>/dev/null
zmodload zsh/datetime 2>/dev/null
autoload -Uz add-zsh-hook add-zle-hook-widget

_moth_osc() { builtin printf '\e]7777;%s;%s\a' "$_moth_token" "$1" }

# Replace control characters so they don't terminate the OSC sequence.
_moth_clean() { REPLY=${1//[$'\x00'-$'\x1f'$'\x7f']/ } }

# The folder's volume was unmounted and mounted again (or the folder deleted and made again):
# the shell still sits in the old, dead folder, where `ls` fails, so it goes back in through
# the path. Fails if there's nothing to recover.
_moth_recover_cwd() {
  [[ -d $PWD && ! . -ef $PWD ]] || return 1
  builtin cd -q -- "$PWD" 2>/dev/null
}

_moth_report_cwd() {
  local p=${PWD//\%/%25}
  p=${p// /%20}
  _moth_osc "7;file://${HOST:-localhost}$p"
}

# The app sends SIGUSR1 when the folder is back while the shell waits at the prompt, so it
# recovers without a keystroke.
_moth_usr1() {
  _moth_recover_cwd || return 0
  _moth_report_cwd
  zle && zle reset-prompt
  return 0
}

_moth_precmd() {
  local ret=$? ran=$_moth_running
  _moth_recover_cwd
  _moth_prev_err=0
  [[ -n "$ran" ]] && (( ret != 0 )) && _moth_prev_err=1
  if [[ -n "$_moth_running" ]]; then
    _moth_osc "133;D;$ret"
    _moth_running=
  fi
  if [[ -n "$_moth_first" ]]; then
    _moth_first=
    # By now .zshrc has been loaded, so we can tell whether the user has their own prompt.
    if [[ "$MOTH_PROMPT" != 0 && ( "$PS1" == "%n@%m %1~ %# " || "$PS1" == "%m%# " ) ]]; then
      _moth_own_prompt=1
    fi
    # ZLE hooks are only registered here: ZLE isn't ready yet while .zshenv runs.
    add-zle-hook-widget line-init _moth_report_line
    add-zle-hook-widget line-pre-redraw _moth_report_line
    add-zle-hook-widget line-finish _moth_line_finish
    _moth_osc "7777;H;${HISTFILE:-$HOME/.zsh_history}"
    _moth_osc "7777;P;$PATH"
    _moth_osc "7777;N;${(k)aliases} ${(k)galiases} ${(k)functions} ${(k)builtins} ${(k)reswords}"
    # Unless your .zshrc already handles USR1.
    if (( ! ${+functions[TRAPUSR1]} )) && [[ -z ${(M)${(f)"$(trap)"}:#* USR1} ]]; then
      TRAPUSR1() { _moth_usr1 }
    fi
  fi
  _moth_report_cwd
  [[ -n "$_moth_own_prompt" ]] && _moth_build_prompt "$ret" "$ran"
  _moth_osc "133;A"
  # Set the prompt-end mark now and again at the end of precmd, in case a theme
  # (starship, p10k) rebuilds PS1 after us.
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

# Dim header with `❯` below it. Uses ANSI colors, which the app maps to the active theme:
# white = secondary text, bright black = metadata, magenta = branch, green/red = status.
#   ~/work/moth  main  ✓ 41.2s · 14:02
#   ❯
_moth_build_prompt() {
  local ret=$1 ran=$2 branch
  # Long paths: only the last three folders.
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
  # After a command, a blank line separates it from the next block.
  local gap=""
  [[ -n "$ran" ]] && gap=$'\n'
  PS1="$gap$header"$'\n'"%F{$mark}❯%f "
}

# Append the prompt-end mark to PS1 (replacing the previous prompt's, which may still be
# there if PS1 isn't rebuilt). The app uses it to delimit command blocks, so it carries the
# number of rows the prompt takes up and whether the previous command failed.
_moth_mark_prompt() {
  local base=${PS1%"%{"$'\e]7777;'*}
  local expanded=${(%)base}
  local newlines=${expanded//[^$'\n']/}
  PS1="${base}%{"$'\e]7777;'"$_moth_token;133;B;rows=$(( ${#newlines} + 1 ));prev_err=$_moth_prev_err"$'\a'"%}"
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
