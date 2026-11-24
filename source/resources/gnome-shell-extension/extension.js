// Universal Overlay — значок на панели GNOME Shell (45+).
//
// GNOME не показывает значки StatusNotifierItem без стороннего расширения,
// поэтому это маленькое расширение рисует собственный значок. Оно ничего не
// хранит и не опрашивает: каждый пункт меню просто запускает
// `universal-overlay --<команда>`, а уже запущенный оверлей получает её по IPC.
// Левый клик — показать/скрыть оверлей, правый — меню.

import GObject from 'gi://GObject';
import St from 'gi://St';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Clutter from 'gi://Clutter';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

function overlayExe() {
    const local = GLib.build_filenamev([GLib.get_home_dir(), '.local', 'bin', 'universal-overlay']);
    if (GLib.file_test(local, GLib.FileTest.IS_EXECUTABLE))
        return local;
    return GLib.find_program_in_path('universal-overlay');
}

function run(...args) {
    const exe = overlayExe();
    if (!exe) {
        Main.notifyError('Universal Overlay', 'Программа не найдена — установите её заново');
        return;
    }
    try {
        Gio.Subprocess.new([exe, ...args], Gio.SubprocessFlags.STDOUT_SILENCE | Gio.SubprocessFlags.STDERR_SILENCE);
    } catch (e) {
        logError(e, 'Universal Overlay');
    }
}

const Indicator = GObject.registerClass(
class UniversalOverlayIndicator extends PanelMenu.Button {
    _init(extPath) {
        super._init(0.0, 'Universal Overlay');
        this.add_child(new St.Icon({
            gicon: Gio.icon_new_for_string(`${extPath}/icon.svg`),
            style_class: 'system-status-icon',
        }));
        const item = (label, ...args) => {
            const i = new PopupMenu.PopupMenuItem(label);
            i.connect('activate', () => run(...args));
            this.menu.addMenuItem(i);
        };
        item('Показать / скрыть оверлей', '--toggle');
        item('Настройки…', '--settings');
        item('Перечитать конфиг', '--reload');
        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        item('Выход', '--quit');
    }

    vfunc_event(event) {
        if (event.type() === Clutter.EventType.BUTTON_PRESS && event.get_button() === Clutter.BUTTON_PRIMARY) {
            run('--toggle');
            return Clutter.EVENT_STOP;
        }
        return super.vfunc_event(event);
    }
});

export default class UniversalOverlayExtension extends Extension {
    enable() {
        this._indicator = new Indicator(this.path);
        Main.panel.addToStatusArea(this.uuid, this._indicator);
    }

    disable() {
        this._indicator?.destroy();
        this._indicator = null;
    }
}
