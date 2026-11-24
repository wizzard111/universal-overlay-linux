# Changelog

## 0.1.1

* The whole program is in English: settings, overlay, tray menu, installer,
  updater and uninstaller.
* Updating from 0.1.0 replaces the old settings shortcut instead of leaving a
  second one.

## 0.1.0 — first release

* In-game overlay: FPS, frame time with a graph, 1% / 0.1% lows.
* Graphs of any sensor — as many as you like, each with its own color, height,
  history window and scale.
* Flexible styling: layout, font, colors, background, corner radius, overlay
  size 25–400%, opacity, screen corner and monitor.
* Settings app: dark, light or system theme, 8 accent colors, animations;
  changes apply to the overlay instantly.
* Automatic updates from GitHub with SHA-256 verification; the old version is
  removed completely, settings are kept.
* Lightweight: the hidden overlay uses no CPU; only displayed sensors are
  polled; between sensor ticks only the graph is redrawn and sent to the window.
* Vulkan layer and OpenGL hook in 64- and 32-bit builds: FPS in 32-bit games
  under Proton (DXVK) and in older OpenGL games.
* OpenGL on Mesa under X11 / XWayland is measured without a wrapper, through
  X Present.
* A GNOME extension with a panel icon; on Wayland the hotkey is assigned in
  the desktop settings automatically.
* Installs into your home folder without a password: app menu, autostart,
  Update and Uninstall in the icon's context menu.
