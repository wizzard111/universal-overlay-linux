// Universal Overlay — panel icon for GNOME Shell (45+).
//
// GNOME shows no StatusNotifierItem icons without a third-party extension,
// so this small extension draws its own icon. It stores and polls nothing:
// every menu item just runs `universal-overlay --<command>`, and the running
// overlay receives it over IPC. Left click shows/hides the overlay, right
// click opens the menu.

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
        Main.notifyError('Universal Overlay', 'Program not found — please reinstall it');
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
        item('Show / hide overlay', '--toggle');
        item('Settings…', '--settings');
        item('Reload config', '--reload');
        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        item('Exit', '--quit');
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
