#!/bin/sh
# Universal Overlay — обновление (Linux).
#
# В клоне репозитория с git: забирает последние изменения, пересобирает и
# ставит в режиме обновления. Иначе запускает апдейтер, который скачивает
# последний релиз с GitHub, проверяет SHA-256 и обновляет установленную
# программу (старая версия удаляется, настройки остаются).
set -eu

NAME=universal-overlay
DEST=${XDG_DATA_HOME:-$HOME/.local/share}/$NAME
ROOT=$(cd "$(dirname "$0")" && pwd)

if [ "${1:-}" = --gui ] && [ ! -t 1 ]; then
    for t in x-terminal-emulator gnome-terminal kgx konsole xfce4-terminal xterm; do
        if command -v $t >/dev/null 2>&1; then
            case $t in
                gnome-terminal | kgx) exec $t -- sh -c "sh '$0'; printf '\nНажмите Enter… '; read _" ;;
                *) exec $t -e sh -c "sh '$0'; printf '\nНажмите Enter… '; read _" ;;
            esac
        fi
    done
fi

if [ -d "$ROOT/.git" ] && [ -f "$ROOT/source/Cargo.toml" ] && command -v git >/dev/null 2>&1 && command -v cargo >/dev/null 2>&1; then
    echo "Получаю последние изменения…"
    git -C "$ROOT" pull --ff-only
    PKG=$(sh "$ROOT/source/build/build.sh" --print-package | tail -n 1)
    exec sh "$PKG/install.sh" --update
fi

for u in "$ROOT/app/$NAME-updater" "$ROOT/$NAME-updater" "$DEST/$NAME-updater"; do
    if [ -x "$u" ]; then
        exec "$u" --install-dir "$DEST"
    fi
done
echo "Universal Overlay не установлен. Запустите ./install.sh"
exit 1
