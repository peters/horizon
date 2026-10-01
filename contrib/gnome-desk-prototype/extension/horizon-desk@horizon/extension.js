// Prototype bridge between Horizon and GNOME Shell. It exports a small D-Bus
// interface on the session bus so Horizon can ask which workspaces and windows
// exist, put a window on a workspace, keep one on all of them, and switch.
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const IFACE = `
<node>
  <interface name="dev.horizon.Desk">
    <method name="State"><arg type="s" direction="out" name="json"/></method>
    <method name="EnsureWorkspaces"><arg type="u" direction="in" name="count"/></method>
    <method name="MoveWindow">
      <arg type="s" direction="in" name="title"/><arg type="u" direction="in" name="workspace"/>
      <arg type="b" direction="out" name="found"/>
    </method>
    <method name="StickWindow">
      <arg type="s" direction="in" name="title"/><arg type="b" direction="in" name="on"/>
      <arg type="b" direction="out" name="found"/>
    </method>
    <method name="KeepAbove">
      <arg type="s" direction="in" name="title"/><arg type="b" direction="in" name="on"/>
      <arg type="b" direction="out" name="found"/>
    </method>
    <method name="Place">
      <arg type="s" direction="in" name="title"/>
      <arg type="i" direction="in" name="x"/><arg type="i" direction="in" name="y"/>
      <arg type="i" direction="in" name="w"/><arg type="i" direction="in" name="h"/>
      <arg type="b" direction="out" name="found"/>
    </method>
    <method name="Switch"><arg type="u" direction="in" name="workspace"/></method>
    <signal name="Changed"/>
  </interface>
</node>`;

function windows() {
    return global.get_window_actors()
        .map(actor => actor.meta_window)
        .filter(win => win.get_window_type() === Meta.WindowType.NORMAL || win.get_window_type() === Meta.WindowType.DIALOG);
}

function matching(title) {
    return windows().filter(win => (win.get_title() ?? '').includes(title));
}

export default class HorizonDesk extends Extension {
    enable() {
        this._dbus = Gio.DBusExportedObject.wrapJSObject(IFACE, this);
        this._dbus.export(Gio.DBus.session, '/dev/horizon/Desk');
        this._nameId = Gio.DBus.session.own_name('dev.horizon.Desk', Gio.BusNameOwnerFlags.NONE, null, null);
        const manager = global.workspace_manager;
        const notify = () => this._dbus?.emit_signal('Changed', null);
        this._signals = [
            [manager, manager.connect('workspace-switched', notify)],
            [manager, manager.connect('notify::n-workspaces', notify)],
            [global.display, global.display.connect('window-created', notify)],
        ];
    }

    disable() {
        for (const [object, id] of this._signals ?? [])
            object.disconnect(id);
        this._signals = null;
        this._dbus?.unexport();
        this._dbus = null;
        if (this._nameId) {
            Gio.DBus.session.unown_name(this._nameId);
            this._nameId = 0;
        }
    }

    State() {
        const manager = global.workspace_manager;
        const workspaces = [];
        for (let index = 0; index < manager.get_n_workspaces(); index++)
            workspaces.push({index, windows: []});
        for (const win of windows()) {
            const entry = {
                title: win.get_title() ?? '',
                app_id: win.get_wm_class() ?? '',
                sticky: win.is_on_all_workspaces(),
                above: win.is_above(),
                rect: (() => { const r = win.get_frame_rect(); return [r.x, r.y, r.width, r.height]; })(),
            };
            const index = win.get_workspace()?.index() ?? -1;
            if (win.is_on_all_workspaces())
                workspaces.forEach(workspace => workspace.windows.push(entry));
            else if (workspaces[index])
                workspaces[index].windows.push(entry);
        }
        return JSON.stringify({
            active: manager.get_active_workspace_index(),
            workspaces,
        });
    }

    EnsureWorkspaces(count) {
        const manager = global.workspace_manager;
        // Static workspaces, so the set the demo shows does not shrink under it.
        Meta.prefs_set_num_workspaces(count);
        this._dynamic ??= Gio.Settings.new('org.gnome.mutter');
        this._dynamic.set_boolean('dynamic-workspaces', false);
        while (manager.get_n_workspaces() < count)
            manager.append_new_workspace(false, global.get_current_time());
    }

    MoveWindow(title, workspace) {
        const found = matching(title);
        for (const win of found) {
            win.unstick();
            win.change_workspace_by_index(workspace, false);
        }
        return found.length > 0;
    }

    StickWindow(title, on) {
        const found = matching(title);
        for (const win of found) {
            if (on)
                win.stick();
            else
                win.unstick();
        }
        return found.length > 0;
    }

    KeepAbove(title, on) {
        const found = matching(title);
        for (const win of found) {
            if (on)
                win.make_above();
            else
                win.unmake_above();
        }
        return found.length > 0;
    }

    Place(title, x, y, w, h) {
        const found = matching(title);
        for (const win of found) {
            win.unmaximize();
            win.move_resize_frame(true, x, y, w, h);
        }
        return found.length > 0;
    }

    Switch(workspace) {
        global.workspace_manager.get_workspace_by_index(workspace)?.activate(global.get_current_time());
    }
}
