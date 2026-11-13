//! The link of a browser host, driven by a page of a test.
//!
//! The page of these tests is not Firefox and not geckodriver: the wire rules of
//! this module are what is tested (the greeting, one frame in flight, the page
//! that came back, the order of the input, a typed run as one message). A real
//! Firefox session is a manual check, because it needs a browser on the machine
//! and the driver of that browser, so the test that starts one is ignored with
//! its reason.
//!
//! The tests of the links of one session are the other file of this module: a
//! link that vanished without a goodbye, and the session that gives its page to
//! the link after it.

pub(crate) mod fixtures;
mod link;
mod session_of_links;

pub(crate) use fixtures::{CollectingWriter, FakePage, Peer};
