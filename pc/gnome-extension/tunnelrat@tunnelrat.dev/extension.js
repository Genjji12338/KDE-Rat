// Tunnelrat GNOME Shell extension.
//
// 1. Clipboard bridge for turad. On Wayland, background programs can't read or
//    watch the clipboard; gnome-shell can. This exports dev.tunnelrat.Shell on
//    the session bus. Only callers whose executable is named `turad` are
//    served, and clipboard changes are sent (unicast) to the subscribed turad
//    only, so the extension doesn't open the clipboard up to every app.
// 2. Top-bar indicator showing the connected phone and its battery.

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import St from 'gi://St';
import Clutter from 'gi://Clutter';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const BUS_NAME = 'dev.tunnelrat.Shell';
const OBJ_PATH = '/dev/tunnelrat/Shell';
const IFACE_XML = `
<node>
  <interface name="dev.tunnelrat.Shell">
    <method name="Subscribe"/>
    <method name="GetClipboard"><arg type="s" direction="out" name="text"/></method>
    <method name="SetClipboard"><arg type="s" direction="in" name="text"/></method>
    <method name="SetDeviceStatus">
      <arg type="s" direction="in" name="id"/>
      <arg type="s" direction="in" name="name"/>
      <arg type="s" direction="in" name="kind"/>
      <arg type="i" direction="in" name="battery"/>
      <arg type="b" direction="in" name="charging"/>
      <arg type="b" direction="in" name="online"/>
    </method>
    <method name="SetPhoneStatus">
      <arg type="s" direction="in" name="name"/>
      <arg type="i" direction="in" name="battery"/>
      <arg type="b" direction="in" name="charging"/>
      <arg type="b" direction="in" name="online"/>
    </method>
    <signal name="ClipboardChanged"><arg type="s" name="text"/></signal>
    <property name="Version" type="u" access="read"/>
  </interface>
</node>`;

const MAX_CLIP = 256 * 1024;

/** Resolves to true if the D-Bus `sender` is a process whose executable is turad. */
function isTurad(sender) {
    return new Promise(resolve => {
        Gio.DBus.session.call(
            'org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus',
            'GetConnectionUnixProcessID', new GLib.Variant('(s)', [sender]),
            new GLib.VariantType('(u)'), Gio.DBusCallFlags.NONE, -1, null,
            (conn, res) => {
                try {
                    const [pid] = conn.call_finish(res).deepUnpack();
                    const exe = GLib.file_read_link(`/proc/${pid}/exe`);
                    resolve(GLib.path_get_basename(exe) === 'turad');
                } catch (e) {
                    resolve(false);
                }
            });
    });
}

/** `tura` on $PATH, or where Tunnelrat installs it. */
function turaPath() {
    return GLib.find_program_in_path('tura') ??
        [GLib.build_filenamev([GLib.get_home_dir(), '.local', 'bin', 'tura']),
            GLib.build_filenamev([GLib.get_home_dir(), '.cargo', 'bin', 'tura'])]
            .find(p => GLib.file_test(p, GLib.FileTest.IS_EXECUTABLE)) ?? 'tura';
}

function tura(...args) {
    try {
        Gio.Subprocess.new([turaPath(), ...args], Gio.SubprocessFlags.STDOUT_SILENCE | Gio.SubprocessFlags.STDERR_SILENCE);
    } catch (e) {
        logError(e, 'Tunnelrat: running tura');
    }
}

function openApp() {
    const app = Gio.DesktopAppInfo.new('dev.tunnelrat.Tunnelrat.desktop');
    if (app)
        app.launch([], null);
    else
        Main.notify('Tunnelrat', 'The Tunnelrat app isn\'t installed (pc/app/install.sh)');
}

/** GNOME icon names come in steps of 10; Adwaita has no "100-charging", only "100-charged". */
function batteryIcon(level, charging) {
    const step = Math.min(100, Math.max(0, Math.round(level / 10) * 10));
    if (charging)
        return step === 100 ? 'battery-level-100-charged-symbolic' : `battery-level-${step}-charging-symbolic`;
    return `battery-level-${step}-symbolic`;
}

/**
 * One bordered chip per connected device (phone / laptop icon · battery
 * icon · percent), so a laptop's battery isn't mistaken for this PC's own.
 */
class DevicesIndicator {
    constructor(dir) {
        this._icons = {
            phone: Gio.icon_new_for_string(dir.get_child('icons').get_child('tunnelrat-phone-symbolic.svg').get_path()),
            pc: Gio.icon_new_for_string(dir.get_child('icons').get_child('tunnelrat-laptop-symbolic.svg').get_path()),
        };
        this._devices = new Map(); // id → {name, kind, battery, charging}
        this.button = new PanelMenu.Button(0.0, 'Tunnelrat', false);
        this._row = new St.BoxLayout({style_class: 'tunnelrat-row', y_align: Clutter.ActorAlign.CENTER});
        this.button.add_child(this._row);
        this.button.visible = false;
        Main.panel.addToStatusArea('tunnelrat-phone', this.button);
    }

    update(id, name, kind, battery, charging, online) {
        if (online)
            this._devices.set(id, {name, kind, battery, charging});
        else
            this._devices.delete(id);
        this._render();
    }

    _render() {
        this._row.destroy_all_children();
        this.button.menu.removeAll();
        for (const d of this._devices.values()) {
            const chip = new St.BoxLayout({style_class: 'panel-status-menu-box tunnelrat-chip', y_align: Clutter.ActorAlign.CENTER});
            chip.add_child(new St.Icon({gicon: this._icons[d.kind] ?? this._icons.phone, style_class: 'system-status-icon', icon_size: 16}));
            if (d.battery >= 0) {
                chip.add_child(new St.Icon({icon_name: batteryIcon(d.battery, d.charging), style_class: 'system-status-icon', icon_size: 16}));
                chip.add_child(new St.Label({text: `${d.battery}%`, y_align: Clutter.ActorAlign.CENTER}));
            }
            this._row.add_child(chip);
            const pct = d.battery >= 0 ? ` — ${d.battery}%${d.charging ? ', charging' : ''}` : '';
            const title = new PopupMenu.PopupMenuItem(`${d.name}${pct}`, {reactive: false});
            title.label.add_style_class_name('tunnelrat-menu-title');
            this.button.menu.addMenuItem(title);
            if (d.kind === 'pc') {
                this.button.menu.addAction('View its screen', () => tura('screen', d.name));
                this.button.menu.addAction('Lock it', () => tura('power', 'lock', '--device', d.name));
            } else {
                this.button.menu.addAction('Show its screen on this PC', () => tura('mirror', d.name));
                this.button.menu.addAction('Ring it', () => tura('ring', d.name));
            }
            this.button.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        }
        this.button.menu.addAction('Open Tunnelrat', () => openApp());
        this.button.visible = this._devices.size > 0;
    }

    destroy() {
        this.button.destroy();
    }
}

export default class TunnelratExtension extends Extension {
    enable() {
        this._subscriber = null;   // unique bus name of the verified turad
        this._lastSet = null;      // text we set ourselves (don't echo it back)
        this._indicator = new DevicesIndicator(this.dir);

        this._dbus = Gio.DBusExportedObject.wrapJSObject(IFACE_XML, this);
        this._dbus.export(Gio.DBus.session, OBJ_PATH);
        this._nameId = Gio.bus_own_name_on_connection(Gio.DBus.session, BUS_NAME, Gio.BusNameOwnerFlags.REPLACE, null, null);

        this._selection = global.display.get_selection();
        this._ownerId = this._selection.connect('owner-changed', (_sel, type) => {
            if (type === Meta.SelectionType.SELECTION_CLIPBOARD)
                this._onClipboardChanged();
        });
    }

    disable() {
        this._selection?.disconnect(this._ownerId);
        this._selection = null;
        if (this._nameId)
            Gio.bus_unown_name(this._nameId);
        this._nameId = 0;
        this._dbus?.unexport();
        this._dbus = null;
        this._indicator?.destroy();
        this._indicator = null;
        this._subscriber = null;
    }

    get Version() {
        return 2;
    }

    _onClipboardChanged() {
        if (!this._subscriber)
            return;
        St.Clipboard.get_default().get_text(St.ClipboardType.CLIPBOARD, (_cb, text) => {
            if (!text || text.length > MAX_CLIP || text === this._lastSet || !this._subscriber)
                return;
            Gio.DBus.session.emit_signal(this._subscriber, OBJ_PATH, BUS_NAME, 'ClipboardChanged',
                new GLib.Variant('(s)', [text]));
        });
    }

    async _guard(invocation) {
        if (await isTurad(invocation.get_sender()))
            return true;
        invocation.return_dbus_error('org.freedesktop.DBus.Error.AccessDenied', 'only turad may use the Tunnelrat bridge');
        return false;
    }

    async SubscribeAsync(_params, invocation) {
        if (!await this._guard(invocation))
            return;
        this._subscriber = invocation.get_sender();
        invocation.return_value(null);
    }

    async GetClipboardAsync(_params, invocation) {
        if (!await this._guard(invocation))
            return;
        St.Clipboard.get_default().get_text(St.ClipboardType.CLIPBOARD, (_cb, text) => {
            invocation.return_value(new GLib.Variant('(s)', [text ?? '']));
        });
    }

    async SetClipboardAsync([text], invocation) {
        if (!await this._guard(invocation))
            return;
        this._lastSet = text;
        St.Clipboard.get_default().set_text(St.ClipboardType.CLIPBOARD, text);
        invocation.return_value(null);
    }

    async SetDeviceStatusAsync([id, name, kind, battery, charging, online], invocation) {
        if (!await this._guard(invocation))
            return;
        this._indicator?.update(id, name, kind, battery, charging, online);
        invocation.return_value(null);
    }

    /** Older turad: a single phone. */
    async SetPhoneStatusAsync([name, battery, charging, online], invocation) {
        if (!await this._guard(invocation))
            return;
        this._indicator?.update('phone', name, 'phone', battery, charging, online);
        invocation.return_value(null);
    }
}
