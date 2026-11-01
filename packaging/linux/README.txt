Moth for Linux
==============

Open Moth: run ./moth from this folder.

Add it to the applications menu: run ./install.sh. It copies the icon and a menu entry
into ~/.local/share and links ./moth as ~/.local/bin/moth; Moth itself stays in this
folder. If you move the folder, run it again. ./install.sh --uninstall removes what it
added.

Moth needs:
- a distribution with glibc 2.35 or newer: Ubuntu 22.04, Debian 12, Fedora 36 or later;
- Vulkan (Mesa's driver or your graphics card's);
- X11 or Wayland. On GNOME's Wayland session the window has no title bar: drag the tab
  bar to move it, or use Super + drag.

Settings live in ~/.config/moth (config.json, themes, hosts.json, the session) and the
log in ~/.local/state/moth/moth.log.

LICENSE holds Moth's license, the GNU GPL version 3.
