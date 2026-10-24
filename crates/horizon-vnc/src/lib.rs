//! A live VNC client session that any UI can show.
//!
//! [`Session::start`] connects on its own worker thread, decodes the desktop
//! and keeps only the latest image. The UI calls [`Session::take_updates`]
//! when it draws, uploads the RGBA pixels of [`Updates::image`] and sends
//! [`InputEvent`]s with [`Session::send_input`]. [`InputState`] turns a
//! person's pointer and keyboard into those events and releases everything
//! still held when the image loses focus. Nothing here depends on a UI
//! toolkit or a GPU API.
//!
//! ```no_run
//! use horizon_vnc::{BUTTON_LEFT, Config, DesktopPoint, InputState, Route, Session, Wake};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let route = Route::Direct("127.0.0.1:5900".parse()?);
//! let session = Session::start(Config::new(route), |wake| match wake {
//!     // Ask the UI for a new frame at once, for example `egui::Context::request_repaint`.
//!     Wake::Visible => {}
//!     // The viewer is not drawn: a later frame is soon enough.
//!     Wake::Hidden => {}
//! })?;
//!
//! // Each UI frame:
//! let updates = session.take_updates();
//! if let Some(image) = updates.image {
//!     let [width, height] = image.frame.size();
//!     let rgba: &[u8] = image.frame.rgba();
//!     // Upload `rgba` (width × height × 4 bytes), or only `image.changed`.
//!     # let _ = (width, height, rgba);
//! }
//!
//! // A click at a desktop pixel:
//! let mut input = InputState::default();
//! let at = Some(DesktopPoint { x: 10, y: 20 });
//! session.send_input(input.pointer(at, BUTTON_LEFT));
//! session.send_input(input.pointer(at, 0));
//! # Ok(())
//! # }
//! ```
#![forbid(unsafe_code)]

mod error;
mod frame;
mod framebuffer;
pub mod input;
mod session;
mod view;

pub use error::Error;
pub use frame::{Frame, present};
pub use input::{
    BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_RIGHT, DesktopPoint, InputEvent, InputState, Modifiers, SCROLL_NOTCH_POINTS,
    ScreenRect, desktop_point, keysym_for_char,
};
#[cfg(any(test, feature = "test-support"))]
pub use session::SentInput;
pub use session::{
    Config, HostKeyFailure, HostKeyRefusal, Image, Observation, ObservedHostKey, Route, ServerDetails, Session, Status,
    StreamEvidence, UnobservedReason, Updates, Wake,
};
pub use view::{ImageLayout, LayoutError, Region, ViewOptions};
