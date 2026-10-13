#!/usr/bin/env python3
"""A small X11 window that offers the given files as one drag, like a file manager does.

Usage: DISPLAY=:N drag.py <x> <y> <file>...
Press the mouse button on the window, move into Horizon and release it there.
"""
import os
import sys
from pathlib import Path

# Only the X11 display of the fixture; never a Wayland desktop of the person.
if not os.environ.get('DISPLAY'):
    sys.exit('Set DISPLAY to the display of the fixture')
os.environ.pop('WAYLAND_DISPLAY', None)
os.environ['GDK_BACKEND'] = 'x11'

import gi  # noqa: E402

gi.require_version('Gdk', '3.0')
gi.require_version('Gtk', '3.0')
from gi.repository import Gdk, Gtk  # noqa: E402

x, y = int(sys.argv[1]), int(sys.argv[2])
uris = [Path(name).resolve().as_uri() for name in sys.argv[3:]]
window = Gtk.Window(title='Drag source')
window.set_decorated(False)
window.set_keep_above(True)
window.move(x, y)
label = Gtk.Label(label='\n'.join(Path(name).name for name in sys.argv[3:]))
box = Gtk.EventBox()
box.add(label)
box.set_size_request(260, 40 + 18 * len(uris))
box.drag_source_set(Gdk.ModifierType.BUTTON1_MASK, [], Gdk.DragAction.COPY)
box.drag_source_add_uri_targets()
box.connect('drag-data-get', lambda _w, _c, data, _i, _t: data.set_uris(uris))
window.add(box)
window.connect('destroy', Gtk.main_quit)
window.show_all()
Gtk.main()
