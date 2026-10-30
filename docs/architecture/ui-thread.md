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
- eframe calls `App::ui` on the UI thread in each pass of a frame. A frame
  is one call of `Context::run_ui`. It usually has one pass. When code calls
  `request_discard`, egui discards the pass and runs one more, up to
  `Options::max_passes` (2 by default). Thus, one frame can call `App::ui` two
  times. In eframe 0.36, `App::update` is replaced by `App::logic` and
  `App::ui`. Horizon does not implement `logic`. eframe runs `BridgeApp`
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
  Thus, a result that a worker thread sends can wait up to 1 s. If no code asks for a
  frame, the result waits for the next input. A worker thread must ask for a frame.
- `egui::Context` is cheap to clone: each clone refers to the same data. It is
  `Send + Sync`. A worker thread can keep a clone and call
  `request_repaint()`. eframe then wakes the UI thread.

## Work that must not run on the UI thread

Do not do the work below on the UI thread. Each item can wait for an unknown
time, or uses a lot of CPU.

- File I/O. This includes `fsync` (`File::sync_all`, `sync_data`), `rename`,
  directory syncs and file locks (`flock`, `File::lock`).
- A wait for a lock that another thread can hold during I/O. A `Mutex::lock`
  is a wait when a worker thread holds the same mutex while it writes or calls a
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
for seconds at a time, during the stop and resume of a Hetzner cloud. On
2026-10-11, the same stall froze the window for 5 to 10 s when a new RunPod
cloud started. That day, one `fsync` took 0.2 to 2.8 s. In the two
incidents, the UI thread called `fsync` for the preparation of a cloud
deployment and the save of cloud panel sessions.
[#1436](https://github.com/peters/horizon/pull/1436) moved them to worker
threads.

The coordination files of browser MCP requests are the same type of work. The
claims, manifest stamps, audits and results take file locks, write manifests
and call `fsync`.
[#1469](https://github.com/peters/horizon/pull/1469) moved the claims to one
ordered worker thread, and
[#1489](https://github.com/peters/horizon/pull/1489) moved the stamps, audits
and results to the same worker thread. This guide also covers the request and
result half of the same work: the recovery claims and answers, the provider
usage, catalog, cloud offer and cloud request results, and the manifest owner
read of each remote panel.

## Patterns

Use these patterns. Each pattern names code that uses it.

### A worker thread and a channel

1. Copy the inputs that the work needs from the app state.
2. Start a thread with the inputs, a `Sender` and a clone of the
   `egui::Context`.
3. Keep the `Receiver` on the app state.
4. On the worker thread, send the result.
5. On the worker thread, call `ctx.request_repaint()` after the send.
6. On each frame, read the `Receiver` with `try_recv`.
7. Apply each result that came.
8. If the result has not come and `try_recv` returns
   `TryRecvError::Disconnected`, treat it as a failure. The thread stopped
   before it sent its result, for example after a panic. A worker thread that sent
   its result and then ended also disconnects the channel. Thus, do not let a
   disconnect after the result replace that result.

Examples:

- [Image destination sign-in](../../crates/horizon-ui/src/app/cloud_panel/production/creation/publishing/state.rs) starts the platform URL handler on a worker thread. A separate receiver reports its result and requests a repaint. Its error does not replace the metadata error. A scope change discards its pending result.
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
- A fence tells a worker thread to wait for earlier work on its own thread. The
  preparation of a deployment takes the fence of a running session save
  (`fence` in
  [`session_record.rs`](../../crates/horizon-ui/src/app/cloud_panel/production/session_record.rs))
  and waits for it on the worker thread, not on the UI thread.

### One ordered worker thread

If writes must stay in order, use one worker thread with a job queue. Do not
start one thread for each write.

- `CredentialWorkbench` in
  [`workbench.rs`](../../crates/horizon-core/src/remote_browser_credential/workbench.rs)
  owns the OS credential store on one thread. The UI sends commands, and
  `poll` reads the events.
- `HostIo` in
  [`browser_host_io.rs`](../../crates/horizon-ui/src/app/browser_host_io.rs)
  does the work on the browser coordination files. #1489 added its first jobs.
  Its worker thread runs the jobs in the order that the UI sends them, and
  the UI applies the outcomes in the same order. A job that must not
  write for a replaced board, for example a claim, takes a board fence when
  the UI sends it. Before it writes, the job checks the fence. If the board
  was replaced, the job writes nothing.

### One claim at a time, and one answer per request

A job that the UI sends on each tick can queue one job per tick. Do not send
one while a job of the same kind still runs. Record the start on the app
state, and clear it when the outcome of the job applies.

- `claim_provider_usage` in
  [`browser_provider_usage.rs`](../../crates/horizon-ui/src/app/browser_provider_usage.rs)
  and `claim_recovery_requests` in
  [`browser_recovery.rs`](../../crates/horizon-ui/src/app/browser_recovery.rs)
  claim only while their guard is clear. A slow disk cannot queue one claim per
  tick.
- `refresh_remote_recovery_scope` reads the manifest owner of each remote panel
  in one job. `scope_reading` holds while that read runs. A read that fails
  keeps the scope that the last good read recorded, and a successful read that
  finds no owner clears the record.

Answer a request on the worker, and only once. A request that was claimed is
answered from the state in memory. A write that fails, because a lock waits
longer than its bound, keeps the result: a later frame writes it again, at most
every 250 ms, until the request's deadline. A retry cannot answer twice,
because the write of a result removes the request file.

- `publish_host_usage_result` in
  [`browser_provider_usage.rs`](../../crates/horizon-ui/src/app/browser_provider_usage.rs)
  and `answer_recovery` in
  [`browser_recovery.rs`](../../crates/horizon-ui/src/app/browser_recovery.rs)
  write the answer of a usage, catalog, cloud offer, cloud or recovery request
  with one worker write. The write that follows finds no request file and writes
  nothing.
- `RefusedAnswers` in
  [`answer_retries.rs`](../../crates/horizon-ui/src/app/answer_retries.rs)
  holds the results that the worker could not write. One write of a refused
  answer runs at a time, so a disk that stays busy queues no retry per frame. An
  answer whose deadline passed is dropped with a warning: the agent has its own
  timeout for that request.
- The agent request polls of the cloud cards (`poll_cloud_create_requests`,
  `poll_cloud_delete_requests`, `poll_cloud_companion_requests` and
  `answer_cloud_list_requests`) run on the UI thread and only build the answer
  there. Each one hands the answer to `publish_host_usage_result`, with
  `answer_result` in `create_agent.rs`, `delete_agent.rs` and
  `companions/agent.rs`.
- A cloud command (`exec.rs`) runs on its own thread, which answers the queue
  there. No UI path of it reaches the queue.
- The cloud offer poll answers each request once. Its deadline bounds the wait
  for the prices. The poll takes no lock of its own.

The browser request pumps send their file and slot work to these two threads.
A create that names a remote target reads its OS-store values on the workbench
thread (`request_value` in
[`browser_remote_credentials.rs`](../../crates/horizon-ui/src/app/browser_remote_credentials.rs)).
Each frame polls the answer and does not wait for it. The create leases its
provider slot as a job of the coordination worker thread, and it opens its
panel only after the lease applies
([`browser_remote_slots.rs`](../../crates/horizon-ui/src/app/browser_remote_slots.rs)).
A starting create also reads its manifest as a job of that thread, at most
every 250 ms, and each frame judges the snapshot that it holds. A duplicate
create reads the source manifest in the ordered job that starts it, so no
frame reads it and no other job waits for it.

### A bounded queue

If the UI can send a job on each frame, put a limit on the queue.
`TransferWorker::submit` in
[`native_app/transfer.rs`](../../crates/horizon-ui/src/native_app/transfer.rs)
refuses a job when four jobs wait. It also refuses a job from an earlier
generation. Its four threads do not keep the jobs in order.

### A delete that stays wanted until its job runs

A close must not wait for the disk. A transcript file is workspace data, so its
removal takes file I/O. Put the delete on the coordination worker, and keep the
local ids whose delete is queued in a set. The job removes the files only while
its id is still in that set.

- `delete_closed_transcript` in
  [`transcript_deletes.rs`](../../crates/horizon-ui/src/app/transcript_deletes.rs)
  queues the delete of the transcript of the panel that just closed.
- `keep_restored_transcripts` and `keep_transcripts_of_saved_board` drop the
  deletes of the panels that a restored board carries again. A restore marks the
  panels of its saved board before any wait of an exit or of a replacement can
  run the queued deletes. Thus, a restored panel never loses its transcript,
  also when the disk is slow.

### Cancellation

Give long work a cancellation token, and cancel it when its result has no
use. The repository check of a new cloud keeps a
`horizon_cloud::Cancellation` in `CancelOnDrop`. When the pending creation is
dropped, the work stops at its next check.

### A bounded wait at exit

Give work that saves data a bounded time to finish before the process
exits. The exit must not wait for an unknown time. If the work does not
finish in that time, Horizon exits without it, and the data is not saved or
not durable.

- `poll_shutdown_progress` in
  [`lifecycle/shutdown.rs`](../../crates/horizon-ui/src/app/lifecycle/shutdown.rs)
  runs on the frames of the exit and does not wait for worker threads. Before the
  exit, it queues the last session save. The session writer thread makes it
  durable, and the frames of the exit check for it. The window shows
  **Closing Horizon…**. The exit waits up to `MAX_SHUTDOWN_WAIT` (10 s)
  for the panels, for each cloud session save (`cloud_sessions_recorded`), for
  the session writer and for a save of a parked cloud draft's record
  (`draft_store::Store`).
  If the browsers did not stop in that time, it stops them by force, which
  can take 3 s more. Then it waits up to 2 s (`EXIT_FLUSH`) for the jobs of
  the browser coordination worker thread. The window paints during these waits.
- Before that wait, the exit waits for companion access removals and saved
  session deletions, with no time limit. The window still paints.
- A synchronous wait at exit, for example in `run_exit_cleanup`, must also
  have a time limit. The window cannot paint during that wait.

## Work that can stay on the UI thread

- Changes to the app state in memory.
- Reads of data in memory that the UI thread owns, or that a worker thread gives to
  it through a channel.
- `Mutex::try_lock`, which returns at once when the lock is held. The UI then
  shows the old data, or tries again on a later frame. Do not loop on
  `try_lock`. A file lock is different: the open of the lock file is file I/O.
- A short `Mutex::lock`, but only when no thread holds that mutex during I/O,
  a process or a wait.

## Review checklist

Use this list for each PR that changes `horizon-ui`, or `horizon-core` code
that `horizon-ui` calls during a frame.

- [ ] No new path from `App::ui` does file I/O or starts a process. File I/O
  includes opens, reads, writes, `sync_all`, `rename` and a file lock that
  waits. A process includes `std::process::Command`.
- [ ] No new `lock()` on the UI thread uses a mutex that a worker thread holds during
  I/O, a network call or a credential store call.
- [ ] No new `recv`, `recv_timeout`, `join` or `sleep` runs on the UI thread,
  except a bounded wait at exit.
- [ ] Each worker thread sends its result and then calls `request_repaint()`.
- [ ] The UI drops a result from an earlier generation, session or board.
- [ ] The UI shows a busy state, and a second request of the same work is
  refused or merged.
- [ ] Writes that must stay in order use one ordered worker thread.
- [ ] Work that saves data can finish in the bounded exit wait. The PR
  says what is not saved when the wait ends first.
- [ ] A test proves that the UI call returns while the I/O waits.

## Test

Write a regression test that stops the fake I/O and measures the UI call.

1. Make the I/O wait. Use one of these methods:
   - Replace the file with a named pipe (`mkfifo`). A read of a named pipe
     waits until a writer opens it.
   - Hold the lock in the test, for example the file lock or the mutex that
     the code takes.
   - Give the worker thread a job that waits on a channel until the test sends a
     value.
2. Before the call, start a release thread. The thread waits for a signal
   from the test with `recv_timeout` and a time limit that is longer than the
   limit of step 6, for example 10 s. Then it releases the I/O.
   Result: If the UI call waits for the I/O, the release thread releases it
   after the time limit. The call returns, and the test fails at step 6. The
   test does not wait forever.
3. Record `Instant::now()`.
4. Call the UI function, for example the poll function of the frame.
   Result: The call returns while the I/O still waits.
5. Send the signal to the release thread before the first assertion.
   Result: The worker thread can end, also when an assertion fails.
6. Assert that the call took less than a limit, for example 2 s.
7. Assert the busy state.
8. Assert that the UI did not apply the result yet.
9. Join the release thread.

The examples below show how to make the I/O wait. They release the I/O only
after the call, so a new test must also start the release thread of step 2.

- `a_record_that_does_not_answer_never_stalls_the_frame_that_deploys` in
  [`preparation/tests.rs`](../../crates/horizon-ui/src/app/cloud_panel/production/preparation/tests.rs)
  uses a named pipe.
- `blocked_persistence_never_blocks_submission_and_stale_completions_are_discarded`
  in
  [`native_app/transfer.rs`](../../crates/horizon-ui/src/native_app/transfer.rs)
  holds the worker thread on a channel. It also makes sure that the UI drops results
  of an earlier generation.
- `a_held_recovery_lock_never_stalls_the_frame_that_claims` in
  [`browser_recovery/tests.rs`](../../crates/horizon-ui/src/app/browser_recovery/tests.rs)
  holds the file lock of a coordination queue from a helper thread that releases
  it on a signal, as step 2 requires. It also checks that the claim runs on the
  worker and that the request is still answered.

## Known UI-thread work

This work still runs on the UI thread. Do not copy it into new code.

- The Device request claim and the `manifest::read` of a browser capture in
  `capture_panel`
  ([`device_requests.rs`](../../crates/horizon-ui/src/app/device_requests.rs)),
  and the read of a dropped file in `editor_panel_size_for_file`
  ([`util.rs`](../../crates/horizon-ui/src/app/util.rs)).
  [#1917](https://github.com/peters/horizon/issues/1917) tracks them.
- The save of a recovered startup board (`save_recovered_startup_runtime_state`
  in [`persistence.rs`](../../crates/horizon-ui/src/app/persistence.rs)) writes
  and renames the session file on the UI thread, without a sync. While the
  session writer thread writes that session, the save is refused, and the
  startup prompt offers to try again. The session
  auto-save queues the board for the session writer thread
  ([`writer.rs`](../../crates/horizon-core/src/session_store/writer.rs)) and
  does not write.
- `poll_config_reload`, which `prepare_frame` in
  [`app/lifecycle.rs`](../../crates/horizon-ui/src/app/lifecycle.rs) calls,
  reads the metadata of the config file at most every 2 s. When the file
  changed, it also reads and parses the config on the UI thread.
- `refresh_active_session_lease`, which `finalize_frame` calls, writes and
  renames the lease file of a saved session at most every 2 s
  (`refresh_lease` in
  [`lease.rs`](../../crates/horizon-core/src/session_store/lease.rs)).
- `Choices::set` writes and renames its file on the UI thread. A worker thread
  makes the file durable
  ([#1491](https://github.com/peters/horizon/pull/1491)). With **Keep my choice
  for this repository** selected, a cloud start and **Open on This PC** call it
  (`keep` in
  [`new_workspace.rs`](../../crates/horizon-ui/src/app/cloud_panel/production/new_workspace.rs)).
  The **Forget** link calls it too
  ([`new_workspace/choice.rs`](../../crates/horizon-ui/src/app/cloud_panel/production/new_workspace/choice.rs)).
- Some cloud card paths read the cloud record from disk under a lock that
  they take with `try_lock`. They do not wait for the lock, but the read
  itself is file I/O.
- A session switch and the restore of a cloud prototype board wait on the
  UI thread for up to 2 s (`EXIT_FLUSH`) until the jobs of the browser
  coordination worker thread end (`finish_browser_host_io_before_replacement` in
  [`browser_host_io.rs`](../../crates/horizon-ui/src/app/browser_host_io.rs)).
  The window does not paint during this wait.
- Two synchronous waits at exit have no time limit. `on_exit` in
  [`app/mod.rs`](../../crates/horizon-ui/src/app/mod.rs) waits until the
  companion access removals are saved. `run_exit_cleanup` joins the worker thread of
  a saved session deletion. The other synchronous waits in
  `run_exit_cleanup` have a time limit, for example 10 s for the casting
  teardown on Linux, 2 s for the browser coordination worker thread and 10 s
  for the session writer thread. Each
  wait has its own limit, so together they can take longer.
