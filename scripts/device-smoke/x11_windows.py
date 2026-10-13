"""Find and close the windows of one process through Xlib, without input on a shared desktop.

The worker image has no xdotool or wmctrl, so this asks each window to close the way a window
manager does: a WM_DELETE_WINDOW client message. The application then exits normally.
"""
import ctypes
import ctypes.util

CLIENT_MESSAGE = 33
XA_CARDINAL = 6
SUCCESS = 0


class XClientMessageEvent(ctypes.Structure):
    _fields_ = [('type', ctypes.c_int), ('serial', ctypes.c_ulong), ('send_event', ctypes.c_int),
                ('display', ctypes.c_void_p), ('window', ctypes.c_ulong),
                ('message_type', ctypes.c_ulong), ('format', ctypes.c_int),
                ('data', ctypes.c_long * 5)]


class XEvent(ctypes.Union):
    _fields_ = [('xclient', XClientMessageEvent), ('pad', ctypes.c_long * 24)]


_ERROR_HANDLER = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)
# A window can disappear between listing and reading it. Xlib's default handler would end this
# process on that BadWindow error, so errors are ignored and the read reports nothing.
_ignore_errors = _ERROR_HANDLER(lambda _display, _event: 0)


def _library():
    path = ctypes.util.find_library('X11') or 'libX11.so.6'
    x11 = ctypes.CDLL(path)
    window = ctypes.c_ulong
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    x11.XSetErrorHandler.argtypes = [_ERROR_HANDLER]
    x11.XSetErrorHandler.restype = _ERROR_HANDLER
    x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
    x11.XDefaultRootWindow.restype = window
    x11.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
    x11.XInternAtom.restype = ctypes.c_ulong
    x11.XQueryTree.argtypes = [ctypes.c_void_p, window, ctypes.POINTER(window), ctypes.POINTER(window),
                               ctypes.POINTER(ctypes.POINTER(window)), ctypes.POINTER(ctypes.c_uint)]
    x11.XGetWindowProperty.argtypes = [
        ctypes.c_void_p, window, ctypes.c_ulong, ctypes.c_long, ctypes.c_long, ctypes.c_int,
        ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_int),
        ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_ulong),
        ctypes.POINTER(ctypes.POINTER(ctypes.c_ubyte))]
    x11.XFree.argtypes = [ctypes.c_void_p]
    x11.XSendEvent.argtypes = [ctypes.c_void_p, window, ctypes.c_int, ctypes.c_long,
                               ctypes.POINTER(XEvent)]
    x11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
    return x11


def _children(x11, display, window):
    root, parent = ctypes.c_ulong(), ctypes.c_ulong()
    children, count = ctypes.POINTER(ctypes.c_ulong)(), ctypes.c_uint()
    if not x11.XQueryTree(display, window, ctypes.byref(root), ctypes.byref(parent),
                          ctypes.byref(children), ctypes.byref(count)):
        return []
    try:
        return [children[index] for index in range(count.value)]
    finally:
        if children:
            x11.XFree(children)


def _window_pid(x11, display, window, atom):
    kind, size, items, after = ctypes.c_ulong(), ctypes.c_int(), ctypes.c_ulong(), ctypes.c_ulong()
    data = ctypes.POINTER(ctypes.c_ubyte)()
    status = x11.XGetWindowProperty(display, window, atom, 0, 1, 0, XA_CARDINAL, ctypes.byref(kind),
                                    ctypes.byref(size), ctypes.byref(items), ctypes.byref(after),
                                    ctypes.byref(data))
    try:
        if status != SUCCESS or size.value != 32 or items.value < 1 or not data:
            return None
        # Xlib returns 32-bit items as C longs.
        return ctypes.cast(data, ctypes.POINTER(ctypes.c_ulong))[0]
    finally:
        if data:
            x11.XFree(data)


def _with_display(name, action):
    x11 = _library()
    previous = x11.XSetErrorHandler(_ignore_errors)
    display = x11.XOpenDisplay(name.encode())
    try:
        if not display:
            raise OSError(f'cannot open X display {name}')
        return action(x11, display)
    finally:
        if display:
            x11.XCloseDisplay(display)
        x11.XSetErrorHandler(previous)


def _windows(x11, display, pid):
    atom = x11.XInternAtom(display, b'_NET_WM_PID', 1)
    if not atom:
        return []
    found, pending = [], [x11.XDefaultRootWindow(display)]
    while pending:
        window = pending.pop()
        if _window_pid(x11, display, window, atom) == pid:
            found.append(window)
        pending.extend(_children(x11, display, window))
    return found


def windows_of(display_name, pid):
    """The windows on `display_name` whose _NET_WM_PID is `pid`."""
    return _with_display(display_name, lambda x11, display: _windows(x11, display, pid))


def request_close(display_name, pid):
    """Send WM_DELETE_WINDOW to each window of `pid`; return how many windows were asked."""
    def close(x11, display):
        windows = _windows(x11, display, pid)
        protocols = x11.XInternAtom(display, b'WM_PROTOCOLS', 0)
        delete = x11.XInternAtom(display, b'WM_DELETE_WINDOW', 0)
        for window in windows:
            event = XEvent()
            event.xclient.type = CLIENT_MESSAGE
            event.xclient.window = window
            event.xclient.message_type = protocols
            event.xclient.format = 32
            event.xclient.data[0] = delete
            x11.XSendEvent(display, window, 0, 0, ctypes.byref(event))
        x11.XSync(display, 0)
        return len(windows)
    return _with_display(display_name, close)
