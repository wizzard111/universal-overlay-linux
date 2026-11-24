#!/bin/sh
# Universal Overlay — установка и обновление (Linux).
#
#   ./install.sh                     установить (из архива релиза или из исходников)
#   ./install.sh --update            обновить (так вызывает апдейтер)
#   --install-dir DIR                другая папка установки
#
# В архиве релиза программа лежит в app/. В клоне репозитория её сначала
# собирает source/build/build.sh (нужен Rust: https://rustup.rs).
#
# Всё ставится в домашнюю папку — пароль не нужен:
#   программа        ~/.local/share/universal-overlay
#   команды          ~/.local/bin
#   меню приложений  ~/.local/share/applications
#   автозапуск       ~/.config/autostart
# Настройки (~/.config/universal-overlay) при обновлении не трогаются.
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

# ---------------------------------------------------------------- из исходников
if [ ! -x "$APPDIR/$NAME" ] && [ -f "$ROOT/source/Cargo.toml" ]; then
    command -v cargo >/dev/null 2>&1 || { echo "Нужен Rust: https://rustup.rs (или скачайте готовый релиз)"; exit 1; }
    echo "Сборка Universal Overlay из исходников…"
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
    printf '[%s] ОШИБКА: %s\n' "$(date '+%F %T')" "$*" >> "$LOG"
    exit 1
}

[ -x "$APPDIR/$NAME" ] || fail "не найдена программа в $APPDIR"
VERSION=$(cat "$APPDIR/VERSION" 2>/dev/null || echo "?")
echo
echo "  Universal Overlay $VERSION"
echo "  Папка: $DEST"
echo
if [ $MODE = update ]; then log "Обновление до $VERSION"; else log "Установка $VERSION"; fi

# ---------------------------------------------------------------- 1. остановка
log "Останавливаю запущенный оверлей"
if [ -x "$DEST/$NAME" ]; then "$DEST/$NAME" --quit 2>/dev/null || true; fi
# Процессы ищем по полной командной строке: имя процесса в Linux обрезается
# до 15 символов («universal-overl»), и pgrep/pkill -x его не находят.
pkill -f "/$NAME-settings( |$)" 2>/dev/null || true
i=0
while pgrep -f "/$NAME( |$)" >/dev/null 2>&1 && [ $i -lt 20 ]; do sleep 0.1; i=$((i + 1)); done
pkill -f "/$NAME( |$)" 2>/dev/null || true

# ---------------------------------------------------------------- 2. файлы
log "Удаляю предыдущую версию и копирую новую"
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

# ---------------------------------------------------------------- 3. команды
log "Создаю команды в $BIN"
mkdir -p "$BIN"
for f in "$NAME" "$NAME-settings" uo-probe universal-overlay-run; do
    ln -sf "$DEST/$f" "$BIN/$f"
done
case ":$PATH:" in
    *":$BIN:"*) ;;
    *) log "Примечание: $BIN нет в PATH — команды доступны по полному пути" ;;
esac

# ---------------------------------------------------------------- 4. меню приложений
log "Добавляю в меню приложений"
mkdir -p "$ICONS/scalable/apps" "$ICONS/256x256/apps" "$ICONS/48x48/apps" "$APPS"
if [ -f "$DEST/icon.svg" ]; then cp "$DEST/icon.svg" "$ICONS/scalable/apps/$NAME.svg"; fi
"$DEST/$NAME" --export-icon "$ICONS/256x256/apps/$NAME.png" 256 2>/dev/null || true
"$DEST/$NAME" --export-icon "$ICONS/48x48/apps/$NAME.png" 48 2>/dev/null || true
cat > "$APPS/$NAME.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Universal Overlay
GenericName=Оверлей FPS и датчиков
Comment=FPS, время кадра, графики и датчики GPU/CPU поверх игр
Exec="$DEST/$NAME"
Icon=$NAME
Terminal=false
Categories=Utility;Monitor;Game;
Keywords=fps;overlay;monitor;gpu;cpu;оверлей;
StartupNotify=false
Actions=toggle;settings;update;uninstall;

[Desktop Action toggle]
Name=Показать / скрыть оверлей
Exec="$DEST/$NAME" --toggle

[Desktop Action settings]
Name=Настройки
Exec="$DEST/$NAME" --settings

[Desktop Action update]
Name=Обновить
Exec=sh "$DEST/update.sh" --gui

[Desktop Action uninstall]
Name=Удалить Universal Overlay
Exec=sh "$DEST/uninstall.sh" --gui
EOF
cat > "$APPS/$NAME-settings.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Universal Overlay — настройки
Comment=Настройки оверлея FPS и датчиков
Exec="$DEST/$NAME-settings"
Icon=$NAME
Terminal=false
Categories=Settings;Utility;
EOF
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$APPS" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$ICONS" 2>/dev/null || true

# ---------------------------------------------------------------- 5. автозапуск
log "Включаю автозапуск при входе"
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
log "Подключаю Vulkan-слой для FPS (64- и 32-битные игры)"
"$DEST/$NAME" --install-layer >/dev/null 2>&1 || log "Vulkan-слой не подключён — FPS в Vulkan-играх недоступен"

# ---------------------------------------------------------------- 7. GNOME
DESKTOP=$(printf '%s' "${XDG_CURRENT_DESKTOP:-}" | tr '[:upper:]' '[:lower:]')
if [ -d "$DEST/gnome-extension" ] && command -v gnome-shell >/dev/null 2>&1; then
    log "Устанавливаю расширение GNOME (значок на панели)"
    rm -rf "$EXT"
    mkdir -p "$EXT"
    cp -r "$DEST/gnome-extension/." "$EXT/"
    if ! gnome-extensions enable "$UUID" 2>/dev/null; then
        # Shell ещё не знает о новом расширении (так на Wayland): включаем его
        # в настройках, значок появится после повторного входа.
        cur=$(gsettings get org.gnome.shell enabled-extensions 2>/dev/null || echo "@as []")
        case $cur in
            *"$UUID"*) ;;
            "@as []" | "[]") gsettings set org.gnome.shell enabled-extensions "['$UUID']" 2>/dev/null || true ;;
            *) gsettings set org.gnome.shell enabled-extensions "${cur%]}, '$UUID']" 2>/dev/null || true ;;
        esac
        case $DESKTOP in
            *gnome*) if [ $MODE = install ]; then log "Значок на панели GNOME появится после выхода и повторного входа"; fi ;;
        esac
    fi
fi

# ---------------------------------------------------------------- 8. горячая клавиша на Wayland
if [ "${XDG_SESSION_TYPE:-}" = wayland ] || { [ -n "${WAYLAND_DISPLAY:-}" ] && [ -z "${DISPLAY:-}" ]; }; then
    log "Wayland: назначаю горячую клавишу в настройках рабочего стола"
    "$DEST/$NAME" --sync-shortcut 2>/dev/null | sed 's/^ *//' >> "$LOG" || true
    tail -n 1 "$LOG" | sed 's/^/    /'
fi

# ---------------------------------------------------------------- 9. запуск
log "Запускаю оверлей"
if command -v setsid >/dev/null 2>&1; then
    setsid "$DEST/$NAME" --background >/dev/null 2>&1 < /dev/null &
else
    nohup "$DEST/$NAME" --background >/dev/null 2>&1 < /dev/null &
fi

echo
echo "  Готово. Universal Overlay есть в меню приложений; Ctrl+Alt+O — показать/скрыть."
echo "  OpenGL-игры на NVIDIA или нативном Wayland: в Steam  universal-overlay-run %command%"
echo "  Журнал: $LOG"
printf '[%s] Готово\n' "$(date '+%F %T')" >> "$LOG"
