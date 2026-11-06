---
applyTo: "crates/horizon-device/src/model.rs,crates/horizon-device/src/x11.rs,crates/horizon-device/tests/key_shortcuts.rs,crates/horizon-device/tests/x11_live.rs"
---

# Native key input

Read `docs/architecture/device-key-input.md` before reviewing this boundary.

The input is an untrusted action request and caller-supplied target data. The
caller must have permission to control the selected local X11 display. A target
file is not proof of ownership or permission.

Keep the closed set of named navigation keys and lower-case letters. Do not
accept arbitrary text, key codes, unknown names or upper-case aliases through
the key field. A key action must retain the existing target, geometry, action
limit, target lock and resize uncertainty checks before input. Keep modifier
release on the existing error and success paths.

Do not add an ambient display fallback, shell command, remote connection or
permission change. A successful dispatch does not prove application success.
The real X11 test proves event delivery and release on its owned test display;
it does not qualify another desktop or application.
