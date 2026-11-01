#!/usr/bin/env bash
# Usage: launch-linux.sh <moth executable> <out dir>
#
# Opens Moth twice with Mesa's software Vulkan (lavapipe), each time for 30 seconds, and
# writes to <out dir>:
#   x11.png, x11.log          on a virtual X server (Xvfb) with openbox
#   wayland.png, wayland.log  on a headless Wayland compositor (sway)
#   windows.txt, vulkaninfo.txt  the X window tree and the Vulkan setup
# Fails only if Moth quits before its 30 seconds are up.
#
# On a bare Xvfb, without a window manager, gpui's X11 window is mapped but never drawn: the
# screenshot comes out black although Moth runs. Any window manager fixes that; openbox is a
# small one.
set -euo pipefail

# `--x11` is this script calling itself inside the virtual X server.
mode=main
if [[ "$1" == --x11 ]]; then
  mode=x11
  shift
fi
app="$(realpath "$1")"
out="$(mkdir -p "$2" && cd "$2" && pwd)"

# Only lavapipe, even if the runner has another Vulkan driver.
icd="$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)"
if [[ -n "$icd" ]]; then
  export VK_ICD_FILENAMES="$icd" VK_DRIVER_FILES="$icd"
fi
export RUST_BACKTRACE=1
# A fresh home, so the run neither reads nor leaves settings, sessions or logs.
HOME="$(mktemp -d)"
export HOME
unset XDG_CONFIG_HOME XDG_STATE_HOME

# Starts Moth, waits 30 seconds, takes the screenshot with the rest of the arguments, and
# writes <name>.status: running, or exited. The display comes from the environment.
run() {
  local name="$1"
  shift
  "$app" > "$out/$name.log" 2>&1 &
  local pid=$!
  sleep 30
  "$@" || echo "screenshot failed" >> "$out/$name.log"
  if kill -0 "$pid" 2>/dev/null; then
    echo "still running" >> "$out/$name.log"
    echo running > "$out/$name.status"
    kill "$pid"
    wait "$pid" 2>/dev/null || true
  else
    local code=0
    wait "$pid" || code=$?
    echo "exited with $code" >> "$out/$name.log"
    echo exited > "$out/$name.status"
  fi
  if [[ -f "$out/$name.png" ]] && command -v convert >/dev/null; then
    echo "$name.png: mean brightness $(convert "$out/$name.png" -colorspace Gray -format '%[fx:mean]' info:)" >> "$out/$name.log"
  fi
}

if [[ "$mode" == x11 ]]; then
  openbox > "$out/openbox.log" 2>&1 &
  openbox=$!
  sleep 2
  run x11 import -window root "$out/x11.png"
  xwininfo -root -tree > "$out/windows.txt" 2>&1 || true
  kill "$openbox" 2>/dev/null || true
  exit 0
fi

vulkaninfo --summary > "$out/vulkaninfo.txt" 2>&1 || true

# X11, with the window manager inside the same virtual server.
xvfb-run --auto-servernum --server-args="-screen 0 1600x1100x24" \
  "$(realpath "$0")" --x11 "$app" "$out" || true

# Wayland, which gpui picks whenever WAYLAND_DISPLAY is set.
runtime="$(mktemp -d)"
chmod 700 "$runtime"
XDG_RUNTIME_DIR="$runtime" WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
  WLR_RENDERER=pixman sway -c /dev/null > "$out/sway.log" 2>&1 &
sway=$!
socket=""
for _ in $(seq 50); do
  socket="$(ls "$runtime" 2>/dev/null | grep -m1 '^wayland-[0-9]*$' || true)"
  [[ -n "$socket" ]] && break
  sleep 0.2
done
if [[ -n "$socket" ]]; then
  (
    unset DISPLAY
    export XDG_RUNTIME_DIR="$runtime" WAYLAND_DISPLAY="$socket"
    run wayland grim "$out/wayland.png"
  )
else
  echo "::warning::sway did not start, so Moth was not opened on Wayland."
  cat "$out/sway.log"
fi
kill "$sway" 2>/dev/null || true

failed=()
for name in x11 wayland; do
  if [[ ! -f "$out/$name.log" ]]; then
    # Only a missing sway is forgiven.
    [[ "$name" == wayland ]] || failed+=("$name")
    continue
  fi
  echo "== $name"
  cat "$out/$name.log"
  [[ "$(cat "$out/$name.status" 2>/dev/null)" == running ]] || failed+=("$name")
done
if [[ ${#failed[@]} -gt 0 ]]; then
  echo "::error::Moth quit within 30 seconds of starting on ${failed[*]}."
  exit 1
fi
