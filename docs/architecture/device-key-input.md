# Native key input

The device library sends key actions to one caller-selected local X11 display.
The caller must own that test display. The target file does not grant permission
to control a person's desktop or another application.

`Action::Key` accepts the existing named navigation keys and the lower-case
letter names `a` through `z`. A letter is a key symbol. It is not a text action.
For example, `q` with `control` sends a shortcut; `p` with `control` and `shift`
sends a different shortcut. Upper-case names and strings with more than one
letter are not aliases. Use the modifier list for Shift.

The JSON decoder accepts only the declared key names. The library checks the
target, geometry and action limits before input. The CLI also keeps its target
lock and resize uncertainty checks. A letter uses the same modifier press and
release path as a named key. This change adds no default display, raw key code,
shell command, remote connection or permission to resize.

The caller must obtain fresh geometry from the selected target's screenshot
before an action. After an indeterminate result, observe the target before a new
action. A dispatch receipt proves that input reached the backend. It does not
prove that an application accepted a shortcut or completed its operation.

Tests cover JSON round trips, rejected names, the existing named keys, real key
events on an owned Xvfb display, modifier release and stale geometry refusal.
Application tests still need the actual application and its observed result.
