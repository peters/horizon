# horizon-vnc

The live VNC client session behind Horizon's native Device viewer, as a crate
that any UI can use. It connects, decodes the desktop on its own thread, keeps
only the latest image and sends pointer and key input. It returns plain RGBA
pixels and takes plain input events, so a UI only uploads pixels and forwards
events. It does not depend on egui, eframe, wgpu, winit or other Horizon crates.

## Add it to an application

Pin a Horizon commit. The crate brings its pinned `vnc-rs` revision with it:

```toml
[dependencies]
horizon-vnc = { git = "https://github.com/peters/horizon.git", rev = "<commit>" }
```

Your application needs the `vnc-rs` pin only if it uses `vnc-rs` itself. Use
the same revision as Horizon's root `Cargo.toml`, so that Cargo builds one copy:

```toml
vnc = { package = "vnc-rs", git = "https://github.com/peters/vnc-rs.git", rev = "773ec2351241c80bbab88ffedae73f1972e0a4c1" }
```

## Use it

```rust
use horizon_vnc::{Config, Route, Session, Wake};

let ctx = egui_ctx.clone();
let session = Session::start(Config::new(Route::Direct("127.0.0.1:5900".parse()?)), move |wake| match wake {
    Wake::Visible => ctx.request_repaint(),
    Wake::Hidden => ctx.request_repaint_after(std::time::Duration::from_secs(1)),
})?;

// In each UI frame:
let updates = session.take_updates();
if let Some(image) = updates.image {
    let picture = egui::ColorImage::from_rgba_unmultiplied(image.frame.size(), image.frame.rgba());
    texture.set(picture, egui::TextureOptions::LINEAR);
}
```

To send input, keep one `InputState` for the viewer and give its events to
`Session::send_input`:

- `desktop_point` maps a position on the drawn image to a desktop pixel,
  through the crop and the scale of `ImageLayout`.
- `InputState::pointer` and `InputState::scroll` send moves, buttons
  (`BUTTON_LEFT`, `BUTTON_MIDDLE`, `BUTTON_RIGHT`) and wheel notches.
- `InputState::modifiers`, `InputState::key` (X11 keysyms; see `input::XK_*`
  and `keysym_for_char`) and `InputState::text` send the keyboard.
- `InputState::clipboard_chord` sends Ctrl or Command with C, X or V, so the
  remote application runs its own clipboard action.
- `InputState::release_all` releases every key and button that is still down.
  Call it when the image loses focus or is not drawn.

## The API

| Item | What it does |
|------|--------------|
| `Session::start(Config, wake)` | Starts the worker thread. `wake` runs on that thread after each update; it must not block. |
| `Route::Direct(addr)`, `Route::Tunnel { program, args }` | TCP, or a program whose stdin and stdout carry the connection (for example `ssh -W`). |
| `Session::take_updates()` | The latest `Image` (a `Frame`, its `ViewOptions` and the `changed` region), status, desktop size, server name and kept clipboard text. |
| `Session::set_visible(bool)` | A hidden viewer gets `Wake::Hidden`, so it can wait for the next frame. |
| `Session::set_options(ViewOptions)` | Crop (`viewport`), image size limits and refresh rate. The desktop resolution does not change. |
| `Session::latest_full()`, `frame_source()` | The last full desktop, for a screenshot or a recorder thread. |
| `Session::send_input(events)` | Queues `InputEvent`s; the worker sends them before the next refresh. |
| `Frame` | RGBA pixels: `size()`, `rgba()` (bytes), `pixels()` (one `u32` per pixel, RGBA in memory order). |
| `present(size, pixels, options)` | The same crop and scale for pixels that a UI already holds. |
| `Status` | `Stopped`, `Connecting`, `Connected` or `Disconnected(reason)`. |

Drop the session to disconnect. To reconnect, start a new session.

With the `test-support` feature, `Session::fixture` makes a connected session
without a server, and `SentInput` gives back the input that it was asked to send.

## Limits

- The session sends no credentials. A server that requires a password fails
  with an error.
- The local clipboard is never sent. Server clipboard text is kept only when
  `Config::keep_server_text` accepts it.
- The session requests Raw, CopyRect and ZRLE pixels in RGBA order, and
  follows desktop resizes. The largest desktop is 8,294,400 pixels (3840 × 2160).
- `Image::changed` is a bounding box. It is the whole image for the first
  image, after a resize, after a change of the crop or the size limits, and
  while the image is cropped or scaled.

## Trust boundary

Everything that the server sends is untrusted: rectangles, desktop sizes,
the desktop name and clipboard text. The pinned `vnc-rs` revision forbids
unsafe code and bounds message sizes, and this crate forbids unsafe code. The
framebuffer refuses rectangles outside the desktop and desktop sizes above the
limit, and the session then stops with `Status::Disconnected`. The server name
is at most 4 KiB but can hold control characters: bound and clean it before you
show it. The crate does not check which addresses it connects to; Horizon
accepts loopback addresses only, and uses SSH for a remote desktop.
