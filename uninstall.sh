#!/bin/sh
# Universal Overlay — удаление (Linux).
#
#   ./uninstall.sh           удалить (спросит про настройки)
#   ./uninstall.sh --purge   удалить вместе с настройками
#   --install-dir DIR        другая папка установки
set -eu

NAME=universal-overlay
UUID=universal-overlay@uo.local
DATA=${XDG_DATA_HOME:-$HOME/.local/share}
CONFIG=${XDG_CONFIG_HOME:-$HOME/.config}
DEST=$DATA/$NAME
BIN=$HOME/.local/bin
APPS=$DATA/applications
ICONS=$DATA/icons/hicolor
PURGE=0
GUI=0
prev=
for a in "$@"; do
    case $a in
        --purge) PURGE=1 ;;
        --gui) GUI=1 ;;
    esac
    [ "$prev" = --install-dir ] && DEST=$a
    prev=$a
done

# Из пункта меню приложения терминала нет: открываемся в нём.
if [ $GUI = 1 ] && [ ! -t 1 ]; then
    for t in x-terminal-emulator gnome-terminal kgx konsole xfce4-terminal xterm; do
        if command -v $t >/dev/null 2>&1; then
            case $t in
                gnome-terminal | kgx) exec $t -- sh "$0" --gui ;;
                *) exec $t -e sh "$0" --gui ;;
            esac
        fi
    done
fi

log() { printf '  • %s\n' "$*"; }

echo
echo "  Удаление Universal Overlay"
echo
log "Останавливаю оверлей"
if [ -x "$DEST/$NAME" ]; then "$DEST/$NAME" --quit 2>/dev/null || true; fi
# Процессы ищем по полной командной строке: имя процесса в Linux обрезается
# до 15 символов («universal-overl»), и pgrep/pkill -x его не находят.
pkill -f "/$NAME-settings( |$)" 2>/dev/null || true
sleep 0.5
pkill -f "/$NAME( |$)" 2>/dev/null || true
if [ -x "$DEST/$NAME" ]; then
    log "Отключаю Vulkan-слой и горячую клавишу Wayland"
    "$DEST/$NAME" --uninstall-layer >/dev/null 2>&1 || true
    "$DEST/$NAME" --remove-shortcut >/dev/null 2>&1 || true
fi
log "Удаляю расширение GNOME"
gnome-extensions disable "$UUID" 2>/dev/null || true
rm -rf "$DATA/gnome-shell/extensions/$UUID"
log "Удаляю пункты меню, автозапуск и команды"
rm -f "$APPS/$NAME.desktop" "$APPS/$NAME-settings.desktop" "$CONFIG/autostart/$NAME.desktop"
rm -f "$ICONS/scalable/apps/$NAME.svg" "$ICONS/256x256/apps/$NAME.png" "$ICONS/48x48/apps/$NAME.png"
for f in "$NAME" "$NAME-settings" uo-probe universal-overlay-run; do
    if [ -L "$BIN/$f" ]; then rm -f "$BIN/$f"; fi
done
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$APPS" 2>/dev/null || true
log "Удаляю файлы программы"
rm -rf "$DEST"
rm -f /dev/shm/universal-overlay.* 2>/dev/null || true
if [ -d "$CONFIG/$NAME" ]; then
    if [ $PURGE = 0 ] && [ -t 0 ]; then
        printf '  Удалить и настройки (%s)? [y/N] ' "$CONFIG/$NAME"
        read -r ans || ans=n
        case $ans in [yYдД]*) PURGE=1 ;; esac
    fi
    if [ $PURGE = 1 ]; then
        rm -rf "$CONFIG/$NAME"
        log "Настройки удалены"
    else
        log "Настройки оставлены: $CONFIG/$NAME"
    fi
fi
echo
echo "  Universal Overlay удалён."
if [ $GUI = 1 ] && [ -t 0 ]; then printf '  Нажмите Enter… '; read -r _ || true; fi
