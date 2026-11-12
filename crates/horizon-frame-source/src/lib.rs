//! A live picture session of one frame source program, for any UI.
//!
//! Horizon holds no codec and no library of such a program: the contract of the
//! bytes is the whole of what it knows. The program starts with the arguments of
//! the settings, it writes pictures to a Unix stream socket on descriptor 3, and
//! it reads and writes control messages on its standard input and standard
//! output. This crate is one side of that contract:
//!
//! - [`picture`] reads one picture under the bounds of its own header.
//! - [`control`] frames one control message and names the messages of both sides.
//! - [`convert`] turns the NV12 planes into the RGBA pixels of a frame.
//! - [`input`] is the input of a person, in the pixels of the picture.
//! - [`Session`] starts the program, reads it on threads of its own and keeps the
//!   newest picture only, so a slow frame never queues a picture.
#![forbid(unsafe_code)]

pub mod control;
pub mod convert;
pub mod error;
pub mod input;
pub mod keys;
pub mod picture;

pub use error::Error;
pub use input::{InputEvent, InputState, Point};
pub use keys::{Button, Key};
pub use picture::{Header, Picture};
