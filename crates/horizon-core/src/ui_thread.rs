//! The guard that reports work that can wait when it runs on the UI thread.
//!
//! [`docs/architecture/ui-thread.md`](../../../../docs/architecture/ui-thread.md)
//! says which work must not run on the UI thread of Horizon. This module makes
//! that rule visible in the code: a function that can wait starts with
//! [`blocking`], and an entry point of the UI thread opens one [`FrameScope`].
//!
//! The guard is silent everywhere else: a worker thread never opens a frame
//! scope, and the scope belongs to one thread, so a worker that a frame starts
//! is not marked by the frame that started it.
//!
//! A call that the repository accepts on the UI thread runs inside an
//! [`ExceptionScope`], which takes the reason. Every exception is one line that
//! `rg "ui_thread::exception\("` finds, and the section *Known UI-thread work*
//! of the guide lists the same reasons, one for one:
//! `scripts/check-ui-thread-exceptions.py` compares the two lists.

use std::{
    cell::Cell,
    collections::BTreeSet,
    panic::Location,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU8, Ordering},
    },
};

/// The environment variable that makes the guard panic instead of writing a log
/// line. Set it to `panic` before Horizon starts.
pub const STRICT_VARIABLE: &str = "HORIZON_UI_THREAD_GUARD";

/// The mode of the guard of this process: not read yet, log, or panic.
const UNSET: u8 = 0;
const LOG: u8 = 1;
const PANIC: u8 = 2;

static MODE: AtomicU8 = AtomicU8::new(UNSET);

/// The call sites that the log mode reported, so each site reports once for the
/// life of the process.
static REPORTED: Mutex<BTreeSet<(&'static str, u32, u32)>> = Mutex::new(BTreeSet::new());

thread_local! {
    /// How deep the current thread is in frames and in exception scopes.
    static DEPTH: Cell<Depth> = const { Cell::new(Depth { frames: 0, exceptions: 0 }) };
}

#[derive(Clone, Copy)]
struct Depth {
    frames: u32,
    exceptions: u32,
}

/// Marks the current thread as inside a frame of the UI thread while it lives.
///
/// A real entry point of the window holds one for the call that draws a frame or
/// handles an input event, and the headless frame helpers of the UI tests hold
/// one too. Scopes nest: the guard is active until the outermost scope ends.
#[derive(Debug)]
#[must_use = "the scope ends when this value is dropped"]
pub struct FrameScope {
    _private: (),
}

impl FrameScope {
    /// Marks the current thread as inside a frame of the UI thread.
    pub fn enter() -> Self {
        DEPTH.with(|depth| {
            let mut value = depth.get();
            value.frames = value.frames.saturating_add(1);
            depth.set(value);
        });
        Self { _private: () }
    }
}

impl Drop for FrameScope {
    fn drop(&mut self) {
        DEPTH.with(|depth| {
            let mut value = depth.get();
            value.frames = value.frames.saturating_sub(1);
            depth.set(value);
        });
    }
}

/// Accepts the calls that can wait while it lives, with the reason that the
/// repository accepts them.
///
/// The reason is one line in the code and one line in *Known UI-thread work* of
/// the guide. A reason of work that a later change moves to a worker starts with
/// `part 2:` and names the finding, so the next change finds it again.
#[derive(Debug)]
#[must_use = "the scope ends when this value is dropped"]
pub struct ExceptionScope {
    _private: (),
}

/// Accepts the calls that can wait on this thread while the returned scope
/// lives.
pub fn exception(reason: &'static str) -> ExceptionScope {
    let _ = reason;
    DEPTH.with(|depth| {
        let mut value = depth.get();
        value.exceptions = value.exceptions.saturating_add(1);
        depth.set(value);
    });
    ExceptionScope { _private: () }
}

impl Drop for ExceptionScope {
    fn drop(&mut self) {
        DEPTH.with(|depth| {
            let mut value = depth.get();
            value.exceptions = value.exceptions.saturating_sub(1);
            depth.set(value);
        });
    }
}

/// Reports a call that can wait when it runs inside a frame of the UI thread.
///
/// Call it first in a function that reads or writes a file, waits for a lock
/// that a worker holds during I/O, starts or waits for a process, waits on a
/// network, sleeps, joins or receives. Outside a frame scope it does nothing, and
/// inside an [`ExceptionScope`] it does nothing either. `what` names the wait, in
/// a few words, for the person who reads the log line or the panic.
#[inline]
#[track_caller]
pub fn blocking(what: &'static str) {
    let inside_a_frame = DEPTH.with(|depth| {
        let value = depth.get();
        value.frames > 0 && value.exceptions == 0
    });
    if inside_a_frame {
        report(what, Location::caller());
    }
}

/// Makes the guard of this process panic on the first call that can wait inside
/// a frame.
///
/// A test harness calls it once at its start. The start of Horizon does not: it
/// reads [`STRICT_VARIABLE`] instead.
pub fn use_strict_mode() {
    MODE.store(PANIC, Ordering::Relaxed);
}

/// Whether a call that can wait inside a frame panics. It also reads
/// [`STRICT_VARIABLE`] on its first call, because that is when the start of
/// Horizon may set it.
#[must_use]
pub fn is_strict() -> bool {
    match MODE.load(Ordering::Relaxed) {
        UNSET => {
            let strict = matches!(std::env::var(STRICT_VARIABLE).as_deref(), Ok("panic"));
            MODE.store(if strict { PANIC } else { LOG }, Ordering::Relaxed);
            strict
        }
        mode => mode == PANIC,
    }
}

fn report(what: &'static str, at: &'static Location<'static>) {
    assert!(
        !is_strict(),
        "a call that can wait ran on the UI thread: {what} at {at}"
    );
    let first = REPORTED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert((at.file(), at.line(), at.column()));
    if first {
        tracing::error!(
            what,
            at = %at,
            "a call that can wait ran on the UI thread; see docs/architecture/ui-thread.md"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::{FrameScope, blocking, exception, use_strict_mode};

    fn panics_with(what: &'static str, work: impl FnOnce()) -> String {
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
        match panic {
            Ok(()) => panic!("{what} answered and did not report"),
            Err(payload) => payload
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "<no message>".to_owned()),
        }
    }

    #[test]
    fn the_guard_is_silent_outside_a_frame() {
        // No frame scope marks this thread, so the guard returns before it reads
        // the mode: a call that can wait is silent here, also in strict mode.
        blocking("a file of the home");
    }

    #[test]
    fn a_frame_in_strict_mode_panics_with_the_wait_and_where_it_called_from() {
        use_strict_mode();
        let _frame = FrameScope::enter();
        let message = panics_with("a write", || blocking("a write of the mirror"));
        assert!(
            message.contains("a write of the mirror"),
            "the message names the wait: {message}"
        );
        assert!(
            message.contains("ui_thread.rs"),
            "the message names the call site of the wait: {message}"
        );
    }

    #[test]
    fn an_exception_scope_accepts_the_call_that_can_wait() {
        use_strict_mode();
        let _frame = FrameScope::enter();
        {
            let _accepted = exception("part 2: F9 an editor saves its file");
            blocking("a save of an editor panel");
        }
        let message = panics_with("a save after the scope", || blocking("a save of an editor panel"));
        assert!(
            message.contains("a save of an editor panel"),
            "the exception ends with its scope: {message}"
        );
    }

    #[test]
    fn the_scope_belongs_to_the_thread_that_made_it() {
        use_strict_mode();
        let _frame = FrameScope::enter();
        let (answers, answered) = mpsc::channel();
        std::thread::spawn(move || {
            // The worker is not inside a frame, also when a frame started it.
            blocking("a read on a worker thread");
            let _ = answers.send(());
        })
        .join()
        .expect("the worker ends without a report");
        answered.recv().expect("the worker did its call");
    }
}
