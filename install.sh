#!/bin/sh
# Universal Overlay — install and update (Linux).
#
#   ./install.sh                     install (from a release archive or from source)
#   ./install.sh --update            update (this is how the updater calls it)
#   --install-dir DIR                another install folder
#
# In a release archive the program is in app/. In a clone of the repository
# it is built first by source/build/build.sh (needs Rust: https://rustup.rs).
#
# Everything goes into your home folder — no password needed:
#   program          ~/.local/share/universal-overlay
#   commands         ~/.local/bin
#   app menu         ~/.local/share/applications
#   autostart        ~/.config/autostart
# Settings (~/.config/universal-overlay) are never touched by an update.
set -eu

NAME=universal-overlay
UUID=universal-overlay@uo.local
DATA=${XDG_DATA_HOME:-$HOME/.local/share}
CONFIG=${XDG_CONFIG_HOME:-$HOME/.config}
DEST=$DATA/$NAME
BIN=$HOME/.local/bin
APPS=$DATA/applications
ICONS=$DATA/icons/hicolor
EXT=$DATA/gnome-shell/extensions/$UUID
MODE=install
for a in "$@"; do
    case $a in --update) MODE=update ;; esac
done
prev=
for a in "$@"; do
    [ "$prev" = --install-dir ] && DEST=$a
    prev=$a
done
ROOT=$(cd "$(dirname "$0")" && pwd)
APPDIR=$ROOT/app

# ---------------------------------------------------------------- from source
if [ ! -x "$APPDIR/$NAME" ] && [ -f "$ROOT/source/Cargo.toml" ]; then
    command -v cargo >/dev/null 2>&1 || { echo "Rust is required: https://rustup.rs (or download a release)"; exit 1; }
    echo "Building Universal Overlay from source…"
    PKG=$(sh "$ROOT/source/build/build.sh" --print-package | tail -n 1)
    exec sh "$PKG/install.sh" "$@"
fi

STATE=${XDG_STATE_HOME:-$HOME/.local/state}/$NAME
mkdir -p "$STATE"
LOG=$STATE/install.log
log() {
    printf '  • %s\n' "$*"
    printf '[%s] %s\n' "$(date '+%F %T')" "$*" >> "$LOG"
}
fail() {
    printf '  ✗ %s\n' "$*" >&2
    printf '[%s] ERROR: %s\n' "$(date '+%F %T')" "$*" >> "$LOG"
    exit 1
}

[ -x "$APPDIR/$NAME" ] || fail "program not found in $APPDIR"
VERSION=$(cat "$APPDIR/VERSION" 2>/dev/null || echo "?")
echo
echo "  Universal Overlay $VERSION"
echo "  Folder: $DEST"
echo
if [ $MODE = update ]; then log "Updating to $VERSION"; else log "Installing $VERSION"; fi

# ---------------------------------------------------------------- 1. stop
log "Stopping the running overlay"
if [ -x "$DEST/$NAME" ]; then "$DEST/$NAME" --quit 2>/dev/null || true; fi
# Match the full command line: Linux cuts process names to 15 characters
# ("universal-overl"), so pgrep/pkill -x would never find them.
pkill -f "/$NAME-settings( |$)" 2>/dev/null || true
i=0
while pgrep -f "/$NAME( |$)" >/dev/null 2>&1 && [ $i -lt 20 ]; do sleep 0.1; i=$((i + 1)); done
pkill -f "/$NAME( |$)" 2>/dev/null || true

# ---------------------------------------------------------------- 2. files
log "Removing the previous version and copying the new one"
STAGE="$DEST.new.$$"
rm -rf "$STAGE"
mkdir -p "$STAGE"
cp -rp "$APPDIR/." "$STAGE/"
cp -p "$ROOT/update.sh" "$ROOT/uninstall.sh" "$STAGE/"
for f in "$NAME" "$NAME-settings" "$NAME-updater" uo-probe universal-overlay-run update.sh uninstall.sh; do
    if [ -e "$STAGE/$f" ]; then chmod 755 "$STAGE/$f"; fi
done
rm -rf "$DEST"
mkdir -p "$(dirname "$DEST")"
mv "$STAGE" "$DEST"

# ---------------------------------------------------------------- 3. commands
log "Creating commands in $BIN"
mkdir -p "$BIN"
for f in "$NAME" "$NAME-settings" uo-probe universal-overlay-run; do
    ln -sf "$DEST/$f" "$BIN/$f"
done
case ":$PATH:" in
    *":$BIN:"*) ;;
    *) log "Note: $BIN is not in PATH — use the commands by their full path" ;;
esac

# ---------------------------------------------------------------- 4. app menu
log "Adding to the app menu"
mkdir -p "$ICONS/scalable/apps" "$ICONS/256x256/apps" "$ICONS/48x48/apps" "$APPS"
if [ -f "$DEST/icon.svg" ]; then cp "$DEST/icon.svg" "$ICONS/scalable/apps/$NAME.svg"; fi
"$DEST/$NAME" --export-icon "$ICONS/256x256/apps/$NAME.png" 256 2>/dev/null || true
"$DEST/$NAME" --export-icon "$ICONS/48x48/apps/$NAME.png" 48 2>/dev/null || true
cat > "$APPS/$NAME.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Universal Overlay
GenericName=FPS and sensor overlay
Comment=FPS, frame time, graphs and GPU/CPU sensors on top of games
Exec="$DEST/$NAME"
Icon=$NAME
Terminal=false
Categories=Utility;Monitor;Game;
Keywords=fps;overlay;monitor;gpu;cpu;frametime;
StartupNotify=false
Actions=toggle;settings;update;uninstall;

[Desktop Action toggle]
Name=Show / hide overlay
Exec="$DEST/$NAME" --toggle

[Desktop Action settings]
Name=Settings
Exec="$DEST/$NAME" --settings

[Desktop Action update]
Name=Update
Exec=sh "$DEST/update.sh" --gui

[Desktop Action uninstall]
Name=Uninstall Universal Overlay
Exec=sh "$DEST/uninstall.sh" --gui
EOF
cat > "$APPS/$NAME-settings.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Universal Overlay Settings
Comment=Settings of the FPS and sensor overlay
Exec="$DEST/$NAME-settings"
Icon=$NAME
Terminal=false
Categories=Settings;Utility;
EOF
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$APPS" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$ICONS" 2>/dev/null || true

# ---------------------------------------------------------------- 5. autostart
log "Enabling autostart at login"
mkdir -p "$CONFIG/autostart"
cat > "$CONFIG/autostart/$NAME.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Universal Overlay
Exec="$DEST/$NAME" --background
Icon=$NAME
Terminal=false
NoDisplay=true
X-GNOME-Autostart-enabled=true
EOF

# ---------------------------------------------------------------- 6. FPS
log "Enabling the Vulkan layer for FPS (64- and 32-bit games)"
"$DEST/$NAME" --install-layer >/dev/null 2>&1 || log "Vulkan layer not enabled — no FPS in Vulkan games"

# ---------------------------------------------------------------- 7. GNOME
DESKTOP=$(printf '%s' "${XDG_CURRENT_DESKTOP:-}" | tr '[:upper:]' '[:lower:]')
if [ -d "$DEST/gnome-extension" ] && command -v gnome-shell >/dev/null 2>&1; then
    log "Installing the GNOME extension (panel icon)"
    rm -rf "$EXT"
    mkdir -p "$EXT"
    cp -r "$DEST/gnome-extension/." "$EXT/"
    if ! gnome-extensions enable "$UUID" 2>/dev/null; then
        # The shell does not know the new extension yet (Wayland): enable it
        # in the settings; the icon appears after signing in again.
        cur=$(gsettings get org.gnome.shell enabled-extensions 2>/dev/null || echo "@as []")
        case $cur in
            *"$UUID"*) ;;
            "@as []" | "[]") gsettings set org.gnome.shell enabled-extensions "['$UUID']" 2>/dev/null || true ;;
            *) gsettings set org.gnome.shell enabled-extensions "${cur%]}, '$UUID']" 2>/dev/null || true ;;
        esac
        case $DESKTOP in
            *gnome*) if [ $MODE = install ]; then log "The GNOME panel icon appears after you sign out and back in"; fi ;;
        esac
    fi
fi

# ---------------------------------------------------------------- 8. Wayland hotkey
if [ "${XDG_SESSION_TYPE:-}" = wayland ] || { [ -n "${WAYLAND_DISPLAY:-}" ] && [ -z "${DISPLAY:-}" ]; }; then
    log "Wayland: assigning the hotkey in the desktop settings"
    "$DEST/$NAME" --sync-shortcut 2>/dev/null | sed 's/^ *//' >> "$LOG" || true
    tail -n 1 "$LOG" | sed 's/^/    /'
fi

# ---------------------------------------------------------------- 9. start
log "Starting the overlay"
if command -v setsid >/dev/null 2>&1; then
    setsid "$DEST/$NAME" --background >/dev/null 2>&1 < /dev/null &
else
    nohup "$DEST/$NAME" --background >/dev/null 2>&1 < /dev/null &
fi

echo
echo "  Done. Universal Overlay is in your app menu; Ctrl+Alt+O shows/hides it."
echo "  OpenGL games on NVIDIA or native Wayland: in Steam use  universal-overlay-run %command%"
echo "  Log: $LOG"
printf '[%s] Done\n' "$(date '+%F %T')" >> "$LOG"
