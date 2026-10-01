// Prototype bridge between Horizon and GNOME Shell. It exports a small D-Bus
// interface on the session bus so Horizon can ask which workspaces and windows
// exist, put a window on a workspace, keep one on all of them, and switch.
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import St from 'gi://St';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

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
    <method name="MoveClass">
      <arg type="s" direction="in" name="appId"/><arg type="u" direction="in" name="workspace"/>
      <arg type="b" direction="out" name="found"/>
    </method>
    <method name="PlaceClass">
      <arg type="s" direction="in" name="appId"/>
      <arg type="i" direction="in" name="x"/><arg type="i" direction="in" name="y"/>
      <arg type="i" direction="in" name="w"/><arg type="i" direction="in" name="h"/>
      <arg type="b" direction="out" name="found"/>
    </method>
    <method name="ToggleOverview"/>
    <method name="PressWorkspaceKey"><arg type="s" direction="in" name="direction"/></method>
    <signal name="Changed"/>
  </interface>
</node>`;

const PANEL_PREFIX = 'horizon-panel-';
const ACCENT = 'rgba(96, 148, 255, 0.95)';

function isPanel(win) {
    return (win.get_wm_class() ?? '').startsWith(PANEL_PREFIX);
}

function windows() {
    return global.get_window_actors()
        .map(actor => actor.meta_window)
        .filter(win => win.get_window_type() === Meta.WindowType.NORMAL || win.get_window_type() === Meta.WindowType.DIALOG);
}

function withClass(appId) {
    return windows().filter(win => win.get_wm_class() === appId);
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
            [global.display, global.display.connect('window-created', (_display, win) => this._brand(win))],
        ];
        // Windows that exist already.
        for (const win of windows())
            this._brand(win);
    }

    // Marks a Horizon panel window as Horizon's: a thin accent outline around the whole window and a
    // small "Horizon" tag in the empty left of its title bar. The window itself is not touched.
    _brand(win) {
        const apply = () => {
            if (!isPanel(win))
                return false;
            const actor = win.get_compositor_private();
            if (!actor)
                return false;
            if (actor._horizonMark) {
                this._layoutMark(win, actor);
                return true;
            }
            const outline = new St.Widget({
                reactive: false,
                style: `border: 2px solid ${ACCENT}; border-radius: 12px;`,
            });
            const tag = new St.BoxLayout({
                reactive: false,
                y_align: 2,
                style: `background-color: ${ACCENT}; border-radius: 99px; padding: 2px 10px 2px 8px; spacing: 6px;`,
            });
            tag.add_child(new St.Widget({
                reactive: false, width: 8, height: 8, y_align: 2,
                style: 'background-color: #0b1220; border-radius: 99px;',
            }));
            tag.add_child(new St.Label({
                text: 'Horizon', y_align: 2,
                style: 'color: #0b1220; font-weight: bold; font-size: 11px;',
            }));
            actor.add_child(outline);
            actor.add_child(tag);
            actor._horizonMark = {outline, tag};
            actor._horizonSignals = [
                win.connect('size-changed', () => this._layoutMark(win, actor)),
                win.connect('position-changed', () => this._layoutMark(win, actor)),
            ];
            this._layoutMark(win, actor);
            return true;
        };
        if (!apply()) {
            // The class is only known once the window has mapped.
            const id = win.connect('notify::wm-class', () => {
                if (apply())
                    win.disconnect(id);
            });
        }
    }

    _layoutMark(win, actor) {
        const mark = actor._horizonMark;
        if (!mark)
            return;
        const frame = win.get_frame_rect();
        const buffer = win.get_buffer_rect();
        const dx = frame.x - buffer.x;
        const dy = frame.y - buffer.y;
        mark.outline.set_position(dx, dy);
        mark.outline.set_size(frame.width, frame.height);
        mark.tag.set_position(dx + 12, dy + 8);
    }

    _unbrand() {
        for (const actorWin of global.get_window_actors()) {
            const mark = actorWin._horizonMark;
            if (!mark)
                continue;
            mark.outline.destroy();
            mark.tag.destroy();
            for (const id of actorWin._horizonSignals ?? [])
                actorWin.meta_window.disconnect(id);
            actorWin._horizonMark = null;
        }
    }

    disable() {
        this._unbrand();
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
        const flat = windows().map(win => {
            const r = win.get_frame_rect();
            return {
                app_id: win.get_wm_class() ?? '',
                title: win.get_title() ?? '',
                workspace: win.is_on_all_workspaces() ? -1 : (win.get_workspace()?.index() ?? -1),
                rect: [r.x, r.y, r.width, r.height],
                focused: win.has_focus(),
            };
        });
        const area = Main.layoutManager.getWorkAreaForMonitor(Main.layoutManager.primaryIndex);
        return JSON.stringify({
            active: manager.get_active_workspace_index(),
            workarea: [area.x, area.y, area.width, area.height],
            workspaces,
            windows: flat,
        });
    }

    EnsureWorkspaces(count) {
        const manager = global.workspace_manager;
        // Static workspaces first, so GNOME does not cull the empty ones as they are created.
        this._dynamic ??= Gio.Settings.new('org.gnome.mutter');
        this._dynamic.set_boolean('dynamic-workspaces', false);
        Meta.prefs_set_num_workspaces(count);
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

    MoveClass(appId, workspace) {
        const found = withClass(appId);
        for (const win of found) {
            win.change_workspace_by_index(workspace, false);
        }
        return found.length > 0;
    }

    PlaceClass(appId, x, y, w, h) {
        const found = withClass(appId);
        for (const win of found) {
            win.unmaximize();
            win.move_resize_frame(true, x, y, w, h);
        }
        return found.length > 0;
    }

    ToggleOverview() {
        Main.overview.toggle();
    }

    // Runs what GNOME runs for Ctrl+Alt+Left and Ctrl+Alt+Right, including its switcher popup.
    PressWorkspaceKey(direction) {
        const binding = {get_name: () => `switch-to-workspace-${direction}`};
        // GNOME 50 added an event argument before the binding.
        if (Main.wm._showWorkspaceSwitcher.length >= 4)
            Main.wm._showWorkspaceSwitcher(global.display, null, null, binding);
        else
            Main.wm._showWorkspaceSwitcher(global.display, null, binding);
    }

    Switch(workspace) {
        global.workspace_manager.get_workspace_by_index(workspace)?.activate(global.get_current_time());
    }
}
