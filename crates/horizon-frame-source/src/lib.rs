//! A live picture session of one frame source program, for any UI.
//!
//! Horizon holds no codec and no library of such a program: the contract of the
//! bytes is the whole of what it knows. The program starts with the arguments of
//! the settings, it writes pictures to a Unix stream socket on descriptor 3, and
//! it reads and writes control messages on its standard input and standard output.
//! This crate is Horizon's side of that contract:
//!
//! - [`picture`] reads one picture under the bounds of its own header.
//! - [`control`] frames one control message and names the messages of both sides.
//! - [`convert`] turns the NV12 planes into the RGBA pixels of a frame.
//! - [`input`] is the input of a person, in the pixels of the picture.
//! - [`Session`] starts the program, reads it on threads of its own and keeps the
//!   newest picture only, so a slow frame never queues a picture.
//!
//! ```no_run
//! use horizon_frame_source::{Config, Session, Wake};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let config = Config {
//!     name: "bench".into(),
//!     program: "/usr/local/bin/frame-tool".into(),
//!     arguments: vec!["--size".into(), "1920x1080".into()],
//! };
//! let session = Session::start(config, |wake| match wake {
//!     // Ask the UI for a new frame at once, for example `egui::Context::request_repaint`.
//!     Wake::Visible => {}
//!     // The viewer is not drawn: a later frame is soon enough.
//!     Wake::Hidden => {}
//! })?;
//! session.set_visible(true);
//! session.set_view_size(Some([1280, 720]));
//! # let _ = session.latest_full();
//! // The UI calls `take_updates` when it draws and uploads `Updates::image`.
//! session.detach();
//! # Ok(())
//! # }
//! ```
#![forbid(unsafe_code)]

pub mod control;
pub mod convert;
pub mod error;
pub mod input;
pub mod keys;
pub mod picture;
mod session;
mod text;
mod worker;

pub use control::Stats;
pub use error::Error;
pub use horizon_vnc::{Frame, Image, Status, Wake};
pub use input::{InputEvent, InputState, Point};
pub use keys::{Button, Key};
pub use picture::{Header, Picture};
pub use session::{Config, InputHello, MAX_LABEL_CHARS, STOP_WAIT, Session, Updates, VIEW_DEBOUNCE};
pub use worker::PICTURE_FD;
