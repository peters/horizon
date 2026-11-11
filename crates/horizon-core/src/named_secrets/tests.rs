//! The rules of a named secret, and what the panels of this computer get of one.
//!
//! The published values are one map of the process, so the tests of both modules
//! take one guard and start from no secret.

mod panels;
mod rules;

/// One test at a time reads or writes the published environment.
fn exclusive() -> super::environment::Exclusive {
    super::environment::exclusive()
}
