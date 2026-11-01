#!/bin/sh
# Adds Moth, run from this folder, to this user's applications menu, and links it as `moth`
# in ~/.local/bin. Nothing else is copied outside ~/.local; Moth itself stays here.
#   ./install.sh               installs
#   ./install.sh --uninstall   removes what it installed
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
# Named after Moth's app id, so the desktop matches its window to the entry.
entry=moth.desktop
data="${XDG_DATA_HOME:-$HOME/.local/share}"
apps="$data/applications"
icons="$data/icons/hicolor/256x256/apps"
bin="$HOME/.local/bin"

refresh() {
  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$apps" >/dev/null 2>&1 || true
  fi
  if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q "$data/icons/hicolor" >/dev/null 2>&1 || true
  fi
}

case "${1:-}" in
  --uninstall)
    rm -f "$apps/$entry" "$icons/moth.png"
    if [ -L "$bin/moth" ]; then
      rm -f "$bin/moth"
    fi
    refresh
    echo "Moth is no longer in the applications menu. This folder is still here."
    exit 0
    ;;
  "") ;;
  *)
    echo "Usage: $0 [--uninstall]" >&2
    exit 2
    ;;
esac

mkdir -p "$apps" "$icons" "$bin"
cp "$here/moth.png" "$icons/moth.png"
# The executable's absolute path, not whatever `moth` PATH finds. The desktop entry spec
# escapes ", `, $ and \ inside a quoted Exec argument.
exec_path="$(printf '%s' "$here/moth" | sed -e 's/[\\"`$]/\\\\&/g')"
while IFS= read -r line; do
  case "$line" in
    Exec=*) printf 'Exec="%s"\n' "$exec_path" ;;
    *) printf '%s\n' "$line" ;;
  esac
done < "$here/$entry" > "$apps/$entry"
ln -sf "$here/moth" "$bin/moth"

refresh
echo "Moth is in the applications menu, and \`moth\` runs it from a terminal if ~/.local/bin is on your PATH."
