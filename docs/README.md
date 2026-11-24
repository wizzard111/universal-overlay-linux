# Universal Overlay for Linux

A lightweight in-game overlay: FPS, frame time with a graph, 1% / 0.1% lows,
graphs of any sensor (load, clocks, GPU/CPU temperatures, memory), flexible
styling and size, and automatic updates from GitHub.

**[Download the latest version](https://github.com/wizzard111/universal-overlay-linux/releases/latest)** —
`universal-overlay-linux-x86_64.tar.gz`: unpack it and run `./install.sh`.

Other systems: [Windows](https://github.com/wizzard111/universal-overlay-windows) · [macOS](https://github.com/wizzard111/universal-overlay-macos).

License: [MIT](../LICENSE). What's new: [CHANGELOG](CHANGELOG.md). Built together with Claude.

## Install, update, uninstall

Only three scripts are at the top; everything else is in folders:

| File | What it does |
|---|---|
| `install.sh` | Installs or reinstalls. From a release archive it takes the ready program from `app/`; in a clone of the repository it builds it first (needs [Rust](https://rustup.rs)) |
| `update.sh` | In a git clone: pulls the changes, rebuilds and updates. Otherwise downloads the latest GitHub release, verifies its SHA-256 and updates. The old version is removed completely; your settings are kept |
| `uninstall.sh` | Removes everything the installer added; asks about your settings (`--purge` removes them without asking) |

No password is needed: everything goes into your home folder.
* The program goes to `~/.local/share/universal-overlay`, commands to `~/.local/bin`.
* The app and its settings appear in the app menu; the icon's context menu
  also has Update and Uninstall.
* Autostart is enabled.
* The Vulkan layer for FPS is enabled for both 64- and 32-bit games,
  including 32-bit DXVK under Proton.
* On GNOME a small extension adds a panel icon: left click shows/hides the
  overlay, right click opens the menu. On Wayland the icon appears after
  signing in again.
* On Wayland the hotkey is assigned automatically in the GNOME / KDE settings:
  only the desktop may grab global keys there.

You can also update from the app: General → Updates.

## Where FPS comes from

* **Vulkan** (native, Proton / DXVK / VKD3D) — an implicit Vulkan layer. It
  only timestamps `vkQueuePresentKHR` and writes it to `/dev/shm`; it draws
  nothing and changes nothing. To disable it for a game:
  `DISABLE_UNIVERSAL_OVERLAY=1 %command%`.
* **OpenGL on Mesa** (AMD, Intel) under X11 / XWayland — no setup, via the
  `PresentCompleteNotify` events of the X Present extension.
* **OpenGL on NVIDIA or native Wayland** — set the game's launch options in
  Steam to `universal-overlay-run %command%`. The wrapper preloads a tiny hook
  (64 and 32 bit) that only timestamps `glXSwapBuffers` / `eglSwapBuffers`.

The process of the active window is measured. If it presents nothing, the one
that presented last is used (for games inside the Steam container).

## Features

* **Frame time and FPS**, a graph that highlights bad frames. Spikes are never
  lost: each pixel shows the slowest frame.
* **1% / 0.1% lows** — percentile or average of the worst frames.
* **Graphs of any sensor** — as many as you like, each with its own color,
  height, history window and scale.
* **Sensors:** GPU — load, clocks, temperatures, power (amdgpu hwmon, i915,
  NVIDIA NVML); CPU — load, clock, temperature (k10temp / zenpower /
  coretemp); RAM.
* **Styling:** layout, font, sizes, colors, background, corner radius,
  **overlay size** (25–400%), **opacity**, screen corner and monitor.
* **Settings app** — dark, light or system theme, 8 accent colors,
  animations; changes apply to the overlay instantly.

The default hotkey is **Ctrl+Alt+O**. Commands: `universal-overlay
--toggle | --show | --hide | --settings | --reload | --quit`.

## Resource use

The hidden overlay sleeps with no timers and captures no frames. The visible
one takes ~8 MB of memory. Only the sensors that are shown are polled. Between
ticks only the graph is redrawn, and only its rectangle is sent to the X server
through shared memory (MIT-SHM). Rendering is software-only with no GPU
context; lows are computed by O(n) selection.

Tested in WSL (Debian 13, WSLg): Vulkan FPS through the layer (`vkcube`),
OpenGL through `universal-overlay-run` (exactly 60.00 FPS for a program with
60 Hz vsync), clean install and uninstall.

## Requirements

X11 or XWayland with a compositor (GNOME, KDE and every Wayland desktop always
have one).

## Building

```
./install.sh                     build and install
sh source/build/build.sh         build a release only → source/target/dist
```

Pushing a `v*` tag runs `.github/workflows/release.yml`.

## Layout

```
install.sh  update.sh  uninstall.sh     install, update, uninstall
docs/                                   documentation
source/
  crates/common     config, hotkeys, fonts, IPC, /dev/shm protocol
  crates/overlay    overlay: core, renderer, frames (Vulkan layer, X Present), sensors, tray
  crates/settings   settings app
  crates/updater    updater (the system's curl and tar, SHA-256 check)
  crates/vklayer    Vulkan layer
  crates/glhook     OpenGL hook (LD_PRELOAD)
  resources/        icon, GNOME extension, universal-overlay-run wrapper
  build/            release build
```
