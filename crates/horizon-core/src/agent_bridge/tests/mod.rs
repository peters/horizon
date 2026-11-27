//! The tests of the bridge: the codec of the line format of Claude Code, one
//! child program on a pipe, and the conversation that its events build.
//!
//! Every line of these tests is hand-written with invented values: no line of a
//! real program is in this repository, and no test starts an agent program. The
//! tests of `program` read the arguments and the environment of a program spec,
//! which start no process.

mod actor;
mod codec;
mod conversation;
mod program;
mod replay;
mod worker;
