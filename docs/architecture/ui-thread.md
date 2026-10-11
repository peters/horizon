# Keep blocking work off the UI thread

This guide tells you which work must not run on the UI thread of Horizon, and
how to move it to a worker thread. Use it when you write or review code in
`horizon-ui`, or code in `horizon-core` that `horizon-ui` calls during a frame.

## How eframe and egui run

Horizon draws its window with eframe and egui 0.36 (see `Cargo.lock`). The
facts below come from the source of that version.

- One thread runs the window. `main` calls `run_native_with_keyboard_observer`
  ([`native_app.rs`](../../crates/horizon-ui/src/native_app.rs)), and the
  winit event loop runs on that thread. This guide calls it the UI thread.
- eframe calls `App::ui` on the UI thread for each frame. In eframe 0.36,
  `App::update` is replaced by `App::logic` and `App::ui`. Horizon does not
  implement `logic`. eframe runs `BridgeApp`
  ([`device_request_pump.rs`](../../crates/horizon-ui/src/app/device_request_pump.rs)),
  which locks a mutex and calls `ui` of `HorizonApp`
  ([`app/mod.rs`](../../crates/horizon-ui/src/app/mod.rs)).
- egui is an immediate mode library. Each frame lays out, reads input for and
  paints the full interface again. egui keeps no widget objects between frames.
- Thus, a call that waits on the UI thread stops all of the window. No panel
  paints, and no key or click gets an answer.
- Frames run only when something asks for one. Input events ask for a frame.
  Code asks with `ctx.request_repaint()` (the next frame) or
  `ctx.request_repaint_after(duration)` (a frame after at most `duration`).
- While panels are open and show no new output, Horizon asks for a frame
  after 100, 250 or 500 ms, or after 1 s. The time increases while the board
  stays quiet (`finalize_frame` in
  [`app/lifecycle.rs`](../../crates/horizon-ui/src/app/lifecycle.rs)).
  Thus, a result that a worker sends can wait up to 1 s. If no code asks for a
  frame, the result waits for the next input. A worker must ask for a frame.
- `egui::Context` is cheap to clone: each clone refers to the same data. It is
  `Send + Sync`. A worker thread can keep a clone and call
  `request_repaint()`. eframe then wakes the UI thread.

## Work that must not run on the UI thread

Do not do the work below on the UI thread. Each item can wait for an unknown
time, or uses a lot of CPU.

- File I/O. This includes `fsync` (`File::sync_all`, `sync_data`), `rename`,
  directory syncs and file locks (`flock`, `File::lock`).
- A wait for a lock that another thread can hold during I/O. A `Mutex::lock`
  is a wait when a worker holds the same mutex while it writes or calls a
  service.
- Network calls: HTTP, provider APIs, WebDriver and DNS lookups.
- A start of a process, or a wait for a process: `docker`, `ssh`, `git` and
  others. `Command::output` and `Command::status` wait for the process to end.
- The keychain and other OS credential stores. A locked store can show an
  unlock prompt and wait for the person.
- `thread::sleep`, `JoinHandle::join`, `Receiver::recv` and
  `Receiver::recv_timeout`. Use `try_recv` or `try_iter` on the UI thread.
- Heavy CPU work, for example the parse of a large log or the decode or
  encode of an image.

### Incidents

On 2026-10-10, a writeback stall on the full machine froze the Horizon window
for seconds at a time. One `fsync` took 0.2 to 2.8 s. The UI thread called
`fsync`, took file locks and wrote manifests for these operations:

- The preparation of a cloud deployment and the save of cloud panel sessions.
  [#1436](https://github.com/peters/horizon/pull/1436) moved them to worker
  threads.
- The coordination files of browser MCP requests: claims, manifest stamps,
  audits and results. The open PRs
  [#1469](https://github.com/peters/horizon/pull/1469) (claims) and
  [#1489](https://github.com/peters/horizon/pull/1489) (stamps, audits and
  results) move them to one ordered worker.

## Patterns

Use these patterns. Each pattern names code that uses it.

### A worker thread and a channel

1. Copy the inputs that the work needs from the app state.
2. Start a thread with the inputs, a `Sender` and a clone of the
   `egui::Context`.
3. Keep the `Receiver` on the app state.
4. On the worker, send the result.
5. On the worker, call `ctx.request_repaint()` after the send.
6. On each frame, read the `Receiver` with `try_recv`.
7. Apply each result that came.
8. Treat `TryRecvError::Disconnected` as a failure. The thread stopped before
   it sent its result, for example after a panic.

Examples:

- `prepare` and `Pending::next` in
  [`preparation/job.rs`](../../crates/horizon-ui/src/app/cloud_panel/production/preparation/job.rs),
  and `poll_cloud_preparations` in
  [`preparation.rs`](../../crates/horizon-ui/src/app/cloud_panel/production/preparation.rs).
- The repository check of a new cloud in
  [`creation_job.rs`](../../crates/horizon-ui/src/app/cloud_panel/production/creation_job.rs).
  `poll_cloud_creation` also asks for a frame after 100 ms while it waits.

### State of the work on the app

The UI must know what runs, so that it does not start the same work two
times and does not apply an old result.

- Keep a busy value on the app. `prepare_production_deployment` returns early
  while a preparation or a provider check runs for the same cloud. The card
  shows **Preparing** and offers no action.
- Give each request a generation number. Increase it when the person makes a
  new choice, or when the board or session changes. Drop a result whose
  generation is not the current one. Examples: `choose` and `report` in
  [`cards/docker.rs`](../../crates/horizon-ui/src/app/cloud_panel/production/cards/docker.rs),
  and `drain_capture_events` in
  [`speech/engine.rs`](../../crates/horizon-ui/src/app/speech/engine.rs).
- Record which session or board started the work. `poll_cloud_creation`
  cancels the work when the active session is not the session that started it.
- A fence tells a worker to wait for earlier work on its own thread. The
  preparation of a deployment takes the fence of a running session save
  (`fence` in
  [`session_record.rs`](../../crates/horizon-ui/src/app/cloud_panel/production/session_record.rs))
  and waits for it on the worker, not on the UI thread.

### One ordered worker

If writes must stay in order, use one worker thread with a job queue. Do not
start one thread for each write.

- `CredentialWorkbench` in
  [`workbench.rs`](../../crates/horizon-core/src/remote_browser_credential/workbench.rs)
  owns the OS credential store on one thread. The UI sends commands, and
  `poll` reads the events.
- The open PR [#1469](https://github.com/peters/horizon/pull/1469) adds
  `HostIo` for the browser coordination files, and #1489 adds jobs to it. Its worker runs the jobs in the
  order that the UI sends them, and the UI applies the outcomes in the same
  order. A board fence makes a job write nothing when its board was replaced.

### A bounded queue

If the UI can send a job on each frame, put a limit on the queue.
`TransferWorker::submit` in
[`native_app/transfer.rs`](../../crates/horizon-ui/src/native_app/transfer.rs)
refuses a job when four jobs wait. It also refuses a job from an earlier
generation. Its four threads do not keep the jobs in order.

### Cancellation

Give long work a cancellation token, and cancel it when its result has no
use. The repository check of a new cloud keeps a
`horizon_cloud::Cancellation` in `CancelOnDrop`. When the pending creation is
dropped, the work stops at its next check.

### A bounded wait at exit

Work that saves data must finish before the process exits, but the exit must
not wait for an unknown time.

- `poll_shutdown_progress` in
  [`lifecycle/shutdown.rs`](../../crates/horizon-ui/src/app/lifecycle/shutdown.rs)
  runs on the frames of the exit and does not wait for workers. Before the
  exit, it saves the session file on the UI thread. The window shows
  **Closing Horizon…**. The exit waits up to `MAX_SHUTDOWN_WAIT` (10 s)
  for the panels and for each cloud session save (`cloud_sessions_recorded`).
  After that time, it stops the browsers by force, which can take 3 s more.
- Before that wait, the exit waits for companion access removals and saved
  session deletions, with no time limit. The window still paints.
- A synchronous wait at exit, for example in `run_exit_cleanup`, must also
  have a time limit. The window cannot paint during that wait.

## Work that can stay on the UI thread

- Changes to the app state in memory.
- Reads of data in memory that the UI thread owns, or that a worker gives to
  it through a channel.
- `Mutex::try_lock`, which returns at once when the lock is held. The UI then
  shows the old data, or tries again on a later frame. Do not loop on
  `try_lock`. A file lock is different: the open of the lock file is file I/O.
- A short `Mutex::lock`, but only when no thread holds that mutex during I/O,
  a process or a wait.

## Review checklist

Use this list for each PR that changes `horizon-ui`, or `horizon-core` code
that `horizon-ui` calls during a frame.

- [ ] No new path from `App::ui` calls `std::fs` writes, `sync_all`,
  `rename`, a file lock that waits, or `std::process::Command`.
- [ ] No new `lock()` on the UI thread uses a mutex that a worker holds during
  I/O, a network call or a credential store call.
- [ ] No new `recv`, `recv_timeout`, `join` or `sleep` runs on the UI thread,
  except a bounded wait at exit.
- [ ] Each worker sends its result and then calls `request_repaint()`.
- [ ] The UI drops a result from an earlier generation, session or board.
- [ ] The UI shows a busy state, and a second request of the same work is
  refused or merged.
- [ ] Writes that must stay in order use one ordered worker.
- [ ] Work that saves data finishes within the bounded exit wait.
- [ ] A test proves that the UI call returns while the I/O waits.

## Test

Write a regression test that stops the fake I/O and measures the UI call.

1. Make the I/O wait. Use one of these methods:
   - Replace the file with a named pipe (`mkfifo`). A read of a named pipe
     waits until a writer opens it.
   - Hold the lock in the test, for example the file lock or the mutex that
     the code takes.
   - Give the worker a job that waits on a channel until the test sends a
     value.
2. Record `Instant::now()`.
3. Call the UI function, for example the poll function of the frame.
   Result: The call returns while the I/O still waits.
4. Release the I/O before the first assertion.
   Result: The worker thread can end, also when an assertion fails.
5. Assert that the call took less than a limit, for example 2 s.
6. Assert the busy state.
7. Assert that the UI did not apply the result yet.

Examples:

- `a_record_that_does_not_answer_never_stalls_the_frame_that_deploys` in
  [`preparation/tests.rs`](../../crates/horizon-ui/src/app/cloud_panel/production/preparation/tests.rs)
  uses a named pipe.
- `blocked_persistence_never_blocks_submission_and_stale_completions_are_discarded`
  in
  [`native_app/transfer.rs`](../../crates/horizon-ui/src/native_app/transfer.rs)
  holds the worker on a channel. It also makes sure that the UI drops results
  of an earlier generation.

Named pipes are not available on Windows. Put the test module behind
`#[cfg(all(test, unix))]`, and put its helpers behind the same condition.

## Known UI-thread work

This work still runs on the UI thread. Do not copy it into new code.

- The browser request claims, the manifest stamps after a panel moves
  (`restamp_browser_manifests_for_placement`, which each frame calls), and
  the audits, manifest
  writes and results of creates, visibility changes and closes. #1469 and
  #1489 move them.
- Browser request file work that #1469 and #1489 do not move: recovery claims,
  provider usage and catalog results, `poll_cloud_offers` on each frame,
  remote slot leases, `manifest::read` calls that retry on Windows, and
  transcript deletion. [#1470](https://github.com/peters/horizon/issues/1470)
  tracks them.
- `plan_remote_create` in
  [`browser_remote_create.rs`](../../crates/horizon-ui/src/app/browser_remote_create.rs)
  locks the keychain store mutex, which the credential worker can hold, and
  reads OS credentials. #1470 tracks it.
- The session auto-save (`auto_save_runtime_state` in
  [`persistence.rs`](../../crates/horizon-ui/src/app/persistence.rs)) writes
  and renames the session file on the UI thread.
  [#1448](https://github.com/peters/horizon/issues/1448) tracks a durable save
  off the UI thread.
- `Choices::set` calls `sync_all` on the UI thread. With **Keep my choice for
  this repository** selected, a cloud start and **Open on This PC** call it
  (`keep` in
  [`new_workspace.rs`](../../crates/horizon-ui/src/app/cloud_panel/production/new_workspace.rs)).
  The **Forget** link calls it too
  ([`new_workspace/choice.rs`](../../crates/horizon-ui/src/app/cloud_panel/production/new_workspace/choice.rs)).
- Some cloud card paths read the cloud record from disk under a lock that
  they take with `try_lock`. They do not wait for the lock, but the read
  itself is file I/O.
- Two synchronous waits at exit have no time limit. `on_exit` in
  [`app/mod.rs`](../../crates/horizon-ui/src/app/mod.rs) waits until the
  companion access removals are saved. `run_exit_cleanup` joins the worker of
  a saved session deletion. The other waits at exit have a limit of 10 s.
