# Code review instructions

These rules set the scope of a code review in this repository. `AGENTS.md` has
the full project rules.

## What to report

- Report a finding only when you can name a concrete input or state and the
  wrong result that it causes.
- A PR that lacks an item that `AGENTS.md` requires, such as a test procedure,
  the GIF of a UI change or the second skill copy, is a finding without a
  runtime input.
- Report findings in the lines that this PR changes.
- In unchanged code, report only a problem that this PR causes, makes
  reachable or makes worse.
- On a later review of the same PR, start with the lines that changed since the
  last review. In the other lines of this PR, report a missed defect only with
  a concrete trigger. Do not look there for new attack surfaces.
- Do not report a design decision that a module comment or a `docs/` file on
  the base branch states as deliberate. For a PR from `peters`, the PR body
  and the docs of this PR count too.
- Do not report a finding again when a reply from `peters` on its thread gave
  the reason that it is not a defect. Report it again only when the code
  changes.
- These exemptions do not cover new work on the UI thread, or a change that
  removes a protection that a trust boundary names.

## Trust boundaries

A file in `.github/instructions/` states the trust boundary of one subsystem.
Review that subsystem against its boundary. Do not widen the boundary to find
new attack surfaces.

## The UI thread

The main thread is the UI thread. It runs `App::ui` and the other eframe and
winit callbacks. New code on this thread must not do any of this work:

- File I/O: an open, a read, a write, a metadata read, a folder listing,
  `fsync`, `rename` or a file lock.
- The start of a process, or a wait for a process.
- A network call, or a call to an OS credential store.
- A blocking wait: `recv`, `recv_timeout`, `join`, `sleep`, or a `lock()` on a
  mutex that a worker thread holds during I/O, a process or a wait. A bounded
  wait at exit is permitted.
- Heavy CPU work, for example the parse of a large log or the decode of an
  image.

`docs/architecture/ui-thread.md` gives the patterns for a worker thread. Its
section "Known UI-thread work" lists work that is already on the UI thread.
Report that work only when this PR adds to it.

## Size

The size thresholds in `AGENTS.md` are review guidance. Do not report the
size, file count or line count of a PR whose body explains why it must stay
together and how to review it.
