# Keep blocking work off the UI thread

This guide tells you which work must not run on the UI thread of Horizon, and
how to move it to a worker thread. Use it when you write or review code in
`horizon-ui`, or code in `horizon-core` that `horizon-ui` calls during a frame.

## How eframe and egui run

Horizon draws its window with eframe and egui 0.37 (see `Cargo.lock`). The
facts below come from the source of that version.

- One thread runs the window. `main` calls `run_native_with_keyboard_observer`
  ([`native_window.rs`](../../crates/horizon-ui/src/native_window.rs)), and the
  winit event loop runs on that thread. This guide calls it the UI thread.
- eframe calls `App::ui` on the UI thread in each pass of a frame. A frame
  is one call of `Context::run_ui`. It usually has one pass. When code calls
  `request_discard`, egui discards the pass and runs one more, up to
  `Options::max_passes` (2 by default). Thus, one frame can call `App::ui` two
  times. eframe calls `App::logic` before each call of `App::ui`. It also
  calls `logic` alone for a window that draws nothing:
  [the pass of such a window](#the-pass-of-a-window-that-draws-no-frame) is
  below. eframe runs `BridgeApp`
  ([`device_request_pump.rs`](../../crates/horizon-ui/src/app/device_request_pump.rs)),
  which locks a mutex and calls `logic` and `ui` of `HorizonApp`
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

## The pass of a window that draws no frame

A window that the desktop does not show gets no frame. On Wayland, a redraw
waits for a frame callback, and the compositor sends none for a surface that it
does not present. winit does not report that state there. A minimized window on
X11 gets no frame either. The work below needs no drawing, so Horizon does it
in `App::logic` (`HorizonApp::logic_pass` in
[`app/undrawn.rs`](../../crates/horizon-ui/src/app/undrawn.rs)).

- **eframe calls `logic` in place of a frame.** When code asked for a repaint
  and no frame comes, eframe runs a pass that calls only `logic`. On Wayland,
  that pass comes 250 ms after the request (`MISSED_REDRAW_TIMEOUT` of eframe).
  For a window that winit reports as minimized or as not visible, the pass
  comes at once, and at most each 100 ms.
- **Each repaint request leads to that pass.** `ctx.request_repaint()` and
  `ctx.request_repaint_after(duration)` of each thread do. Thus, a worker
  thread wakes the pass as it wakes a frame.
- **The pass before a frame does nothing.** `logic` also runs before each
  `ui`. `logic_pass` does its work only in place of a frame. It reads two
  values of egui for this. The count of the complete frames is higher than at
  the last `ui`, and the time of the input is the same. Thus, no work runs two
  times in one frame.
- **The pass does five things.**
  1. It gives the wake to each terminal that has none.
  2. It starts the program of a panel that a request opened, and it applies
     the answers of the coordination worker.
  3. It reads the events of each terminal that has one. The answers to the
     questions of a program go out. The title, the status and the end of the
     program are read.
  4. It runs the window part of the
     [shared terminals](shared-terminals.md#what-a-window-with-no-frame-does).
  5. It reads the states of the agent panels, and it sends a desktop notice
     that is due.
- **The pass costs nothing when nothing asks for it.** It has no timer. A
  terminal asks for it with the first event after the window read that
  terminal (`Wake::Event`), and only while the window draws nothing. While
  frames run, the poll of a frame reads the events. In the pass, a terminal
  with no event costs one atomic load.
- **A terminal gets its pass at once.** The 250 ms of eframe are too long for
  a program that waits for an answer of its terminal. Thus, the wake of a
  terminal does not ask for a repaint. It sends the wake `TERMINAL_WAKE_PASS`
  through the proxy of the event loop, as the pump of the device requests
  does. `user_event` in
  [`native_window.rs`](../../crates/horizon-ui/src/native_window.rs) then
  calls `DeviceRequestBridge::pass_on_ui_thread`, and the pass runs with no
  wait. The window draws nothing from the first pass that eframe runs in place
  of a frame. Until that pass, an event waits for eframe one time.
- **The calls at once have a bound.** The events of one terminal join: the
  terminal asks one time until the window read it. Each call books 16 ms, and
  no call goes out while 128 ms are booked. Thus, eight calls go at once, and
  about sixty a second after them. An event above that bound asks for a
  repaint, and eframe runs its pass as before. A program that prints with no
  end gets at most about 66 passes a second on Wayland. A call that arrives
  after a frame ran does nothing, because that frame took the events.
- **One pass can ask for two more.** After a pass that took output, one pass
  follows 600 ms later: the hook of an agent writes the state of a turn after
  the last output of that turn. After a pass that asked for a read of the
  agent states, one pass follows 100 ms later and takes the answer. A pass
  that took no output and asked for no read asks for no pass.
- **The rules of a frame apply.** The pass runs in a `FrameScope`, and it must
  not wait. It must not draw, measure text or read the pointer. egui runs no
  pass of its own then, so `ctx.input` holds the input of the last frame. Only
  the focus and the viewports are new, in `input.raw`.
- **A copy to the clipboard waits for a frame.** A copy that a program asked
  for with OSC 52 stays with its terminal until a frame hands it over.

The pump of the device requests
([`device_request_pump.rs`](../../crates/horizon-ui/src/app/device_request_pump.rs))
is older than `App::logic`. It wakes the UI thread through the proxy of the
event loop when a request file appears, and it needs no repaint request.

### What waits for a frame

The work below runs only in a frame (`update_ui` in
[`app/mod.rs`](../../crates/horizon-ui/src/app/mod.rs)). A window that draws
nothing does not do it:

- All drawing, and all input of a person: keys, the pointer, shortcuts, dropped
  files and speech.
- The copy of a program to the clipboard.
- The server sync (`poll_server_sync`): the machine list, the secrets and the
  settings of the sync link. Also the settings that another computer changed
  (`apply_settings_io`, `poll_config_reload`).
- The machines: their load, their watch and the SSH hosts.
- The browser panels: their events, and the create requests of an agent.
- The upkeep of a panel: the SSH reconnect, the read of the working directory,
  a close or a restart that a person asked for, the save of an editor and the
  start of a device video.
- The session: the switch, the catalog, the pickup, the lease and the save of
  the runtime state. The pump of the device requests saves the state that a
  request changed.
- The close request of the window (`exit_on_close_request`).
- The folder of a second `horizon <folder>`.
- The idle state of an agent whose program reports no state. The screen of its
  terminal is read only in a pass that took output.

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
- The secrets file and any other store of a value. A read or a commit waits for
  the disk.
- `thread::sleep`, `JoinHandle::join`, `Receiver::recv` and
  `Receiver::recv_timeout`. Use `try_recv` or `try_iter` on the UI thread.
- Heavy CPU work, for example the parse of a large log or the decode or
  encode of an image.

### Incidents

On 2026-10-10, a writeback stall on the full machine froze the Horizon window
for seconds at a time. On 2026-10-11, the same stall froze the window for 5 to
10 s. That day, one `fsync` took 0.2 to 2.8 s. In the two incidents, the UI
thread called `fsync` on its own frame.
[#1436](https://github.com/peters/horizon/pull/1436) moved the calls to worker
threads.

The coordination files of browser MCP requests are the same type of work. The
claims, manifest stamps, audits and results take file locks, write manifests
and call `fsync`.
[#1469](https://github.com/peters/horizon/pull/1469) moved the claims to one
ordered worker thread, and
[#1489](https://github.com/peters/horizon/pull/1489) moved the stamps, audits
and results to the same worker thread. This guide also covers the request and
result half of the same work: the recovery claims and answers, the provider
usage and catalog results, and the manifest owner read of each remote panel.

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

- The saved-conversation deletion in
  [`session_deletion.rs`](../../crates/horizon-ui/src/app/panels/session_deletion.rs)
  starts one named thread. The thread holds a clone of the context and calls
  `request_repaint` after each progress report.
- `spawn_remote_host_catalog_refresh` in
  [`remote_hosts.rs`](../../crates/horizon-ui/src/app/remote_hosts.rs) reads the
  host catalog on a thread and keeps the `Receiver` on the app state. Each frame
  reads it with `try_recv`.
- `spawn_startup_bootstrap` in
  [`session.rs`](../../crates/horizon-ui/src/app/session.rs) validates the saved
  agent sessions on a thread and sends one outcome.
- `AttachedHostSlot::connecting` in
  [`attached_host.rs`](../../crates/horizon-core/src/browser/attached_host.rs)
  starts the thread `browser-attach`. That thread reads the `DevTools` endpoint
  of another machine, starts its browser and opens the forward, so the `ssh`
  calls of a browser panel of that machine never run on the UI thread. Creating
  the panel returns at once, and the panel draws the connect as a state. The
  Retry of the person starts one more connect on a new thread.

### State of the work on the app

The UI must know what runs, so that it does not start the same work two
times and does not apply an old result.

- Keep a busy value on the app. `remote_hosts_refresh_in_flight` in
  [`remote_hosts.rs`](../../crates/horizon-ui/src/app/remote_hosts.rs) refuses a
  second refresh of the host catalog while one runs.
- Count the boards that were replaced. `HostIo` in
  [`browser_host_io.rs`](../../crates/horizon-ui/src/app/browser_host_io.rs)
  counts them in `replaced` and `retired`. The frame that applies an outcome of
  a board that no longer stands drops the outcome.
- Record which session or board started the work. A job that must not write for
  a replaced board takes the board fence when the UI sends it, and it checks the
  fence before it writes.

### A panel that waits for its preparation

A panel needs reads of its own data before its process starts: the saved screen
of its transcript, its working directory, the saved work of its agent and the
text of its file. The frame creates the panel with a screen that says Horizon
prepares it, and gives those reads to the coordination worker.

- `Board::create_panel_waiting` and `Board::from_runtime_state_waiting`
  ([`board/workspaces.rs`](../../crates/horizon-core/src/board/workspaces.rs))
  record each panel that waits in `pending_starts`. A panel that runs no local
  process starts at once.
- `queue_panel_starts`
  ([`app/panel_starts.rs`](../../crates/horizon-ui/src/app/panel_starts.rs))
  sends one preparation for each of them, keeps the ones that wait, and asks
  for no second preparation of the same panel. The frame that applies an answer
  starts the process with `Board::start_panel`, or drops the answer when the
  panel is gone.
- `prepare_panel_start`
  ([`panel/spawn/start.rs`](../../crates/horizon-core/src/panel/spawn/start.rs))
  is the work: the transcript, the working directory, the `script` probe and the
  saved work of an agent. `warm_script_support`
  ([`transcript.rs`](../../crates/horizon-core/src/transcript.rs)) resolves the
  probe once at the start of Horizon, on a worker.
- `prepare_restart` and `finish_restart` prepare a restart the same way. The old
  process ends between the two, so the check that its conversation is open
  nowhere runs after that teardown. `request_panel_restart`
  ([`app/panel_restarts.rs`](../../crates/horizon-ui/src/app/panel_restarts.rs))
  runs both, and a machine terminal keeps its attempt count and its delay.

### A value of a process that a worker reads

A shell panel follows the directory that its process is in, so a `cd` reaches the
session save and the saved board. On Linux that question is one `readlink` of
`/proc`. macOS has no such file: `pgrep -P` walks to the deepest child of the
panel, and `lsof` reads the directory of that child. Those are two programs, and
a frame must not start them.

- `Panel::process_output` ([`panel.rs`](../../crates/horizon-core/src/panel.rs))
  records the process whose directory may have changed in `cwd_read`, and
  `Board::cwd_reads` hands the request out. The frame makes no read of its own.
- `queue_panel_cwd_reads` ([`app/cwd_reads.rs`](../../crates/horizon-ui/src/app/cwd_reads.rs))
  sends one read for each panel that has a request, on the coordination worker
  thread. One read runs for a panel at a time and no panel is read more often than
  its interval, so a terminal that prints on every frame sends no more than that.
  The frame that sends a read takes the request of that panel, and output that
  comes while the read runs records a request of its own, so a directory that
  changed in between is read again.
- `read_cwd` ([`terminal/cwd.rs`](../../crates/horizon-core/src/terminal/cwd.rs))
  is the work. `CwdReader` is a value and not a condition of the build, so the
  reader of macOS runs on any computer with stand-in programs on a private
  `PATH`. The read starts no program on a computer without `pgrep` or `lsof`, and
  each program is ended by its own process id when `PROGRAM_BOUND` (2 s) passes.
  A program that hangs holds the worker for a bounded time only.
- The frame that applies the answer keeps it with
  `Board::record_working_directory`, and marks the board for the session save.
  The saved board records the directory that the panel keeps, not a read of the
  frame: a directory that a person changed is a moment old at most.

### A value of a file that a worker reads and the frame shows

The attention state of an agent panel comes from the work ledger of its launch,
which is a file of the Horizon home. A frame must not read it.

- `AttentionState` ([`app/attention.rs`](../../crates/horizon-ui/src/app/attention.rs))
  keeps one named thread, `agent-attention`, and a job and a result channel. The
  frame asks for a read of the ledgers of the agent panels at most every 500 ms,
  and only while no read runs. The thread reads each record and sends the
  ledgers back.
- The frame reads the answer with `try_recv` and asks for one repaint only when
  a state changed. A quiet agent therefore costs one file read per interval and
  no repaint, and a changed state asks for exactly one. The frame already runs
  at least once a second while a panel is open, so an answer never waits for
  input.
- The frame applies the answer in memory, and the sidebar reads that value. A
  read that fails keeps the state that the last good read published.

### A worker that owns the desktop clipboard

`crate::clipboard`
([`clipboard.rs`](../../crates/horizon-ui/src/clipboard.rs)) owns the desktop
clipboard on one thread, as `primary_selection` owns the PRIMARY selection. A
frame sends it the text or the image of a copy; the thread encodes the image and
writes it. A test build records the copies instead of publishing them.

### One ordered worker thread

If writes must stay in order, use one worker thread with a job queue. Do not
start one thread for each write.

- `CredentialWorkbench` in
  [`workbench.rs`](../../crates/horizon-core/src/remote_browser_credential/workbench.rs)
  owns the secrets file on one thread. The UI sends commands, and
  `poll` reads the events.
- `HostIo` in
  [`browser_host_io.rs`](../../crates/horizon-ui/src/app/browser_host_io.rs)
  does the work on the browser coordination files. #1489 added its first jobs.
  Its worker thread runs the jobs in the order that the UI sends them, and
  the UI applies the outcomes in the same order. A job that must not
  write for a replaced board, for example a claim, takes a board fence when
  the UI sends it. Before it writes, the job checks the fence. If the board
  was replaced, the job writes nothing. A worker thread that cannot start does
  not make the UI thread run its job: the job waits in a queue, and the next
  job or frame tries a worker again. The jobs keep their order, and the job's
  outcome still reaches the UI.
- Horizon reads the size of a dropped editor file on that worker too.
  `queue_dropped_editor_file`
  ([`file_drop/editor_panels.rs`](../../crates/horizon-ui/src/app/file_drop/editor_panels.rs))
  records the drop that the frame resolved in `pending_editor_drops`, and it
  queues the read of `editor_panel_size_for_file`. One panel for each file opens
  on the frame that applies its size, in the order the person dropped the
  files. A read that applies after a board replacement opens no panel. A file
  that is not there opens its panel with the default size.
- An editor panel writes its file on that worker too. `queue_editor_saves`
  ([`editor_saves.rs`](../../crates/horizon-ui/src/app/editor_saves.rs)) queues
  the whole text of each buffer whose save the frame asked for, and of each
  buffer whose panel leaves the board with text that its file does not hold. A
  closed panel hands its text to the board before it leaves, so the write of a
  close takes its turn after the writes asked for before it. The frame of a
  close returns at once, and the exit waits for that write within `EXIT_FLUSH`.
  A write that fails while its panel still stands says why in the mode bar of
  the panel; a write that fails after its panel left says why in one line of the
  machine section of the sidebar, because the text is then lost.

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
- `claim_device_requests` in
  [`device_requests/claim.rs`](../../crates/horizon-ui/src/app/device_requests/claim.rs)
  claims the Device panel requests of this host, and `write_device_answer`
  writes each answer. Both are jobs of the coordination worker. The frame or
  the request pump applies each request a claim took, in claim order, and
  answers it. A claim removes the request file, so the host keeps each request
  it took: a request that no frame applies is answered with `host_shutdown`
  before an exit or a session switch flushes the worker, or with
  `request_expired` when its deadline passed. The watcher of an unpresented
  host wakes while a claim, a request it took or an answer is outstanding, so
  all three settle with no frame. The claim job also reads the manifest that a
  browser capture judges, so the frame that takes the capture reads no file.
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
  write the answer of a usage, catalog or recovery request with one worker
  write. The write that follows finds no request file and writes
  nothing.
- `RefusedAnswers` in
  [`answer_retries.rs`](../../crates/horizon-ui/src/app/answer_retries.rs)
  holds the results that the worker could not write. One write of a refused
  answer runs at a time, so a disk that stays busy queues no retry per frame. An
  answer whose deadline passed is dropped with a warning: the agent has its own
  timeout for that request.
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
[`native_window/transfer.rs`](../../crates/horizon-ui/src/native_window/transfer.rs)
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
use. An SSH upload in
[`ssh_upload/worker.rs`](../../crates/horizon-ui/src/app/ssh_upload/worker.rs)
keeps a control channel from the UI to its thread. `cancel` sends
`UploadControl::Cancel`, and the thread stops at its next check.

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
  for the panels and for the session writer.
  If the browsers did not stop in that time, it stops them by force, which
  can take 3 s more. Then it waits up to 2 s (`EXIT_FLUSH`) for the jobs of
  the browser coordination worker thread, which also write the text of the
  editor buffers that the close asked for. The window paints during these waits.
- Before that wait, the exit waits for saved session deletions, with no time
  limit. The window still paints.
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
- [ ] A new function that can wait calls `ui_thread::blocking`. A new call that
  the repository accepts on the UI thread has one `ui_thread::exception` with
  its reason, and the list of known work above names that reason.
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

- `the_close_reads_the_panel_manifest_on_the_coordination_worker` in
  [`browser_close_requests.rs`](../../crates/horizon-ui/src/app/browser_close_requests.rs)
  uses a named pipe for the manifest, so the read blocks until a writer
  publishes the ownership.
- `blocked_persistence_never_blocks_submission_and_stale_completions_are_discarded`
  in [`native_window/transfer.rs`](../../crates/horizon-ui/src/native_window/transfer.rs)
  holds the worker thread on a channel. It also makes sure that the UI drops
  results of an earlier generation.
- `a_held_recovery_lock_never_stalls_the_frame_that_claims` in
  [`browser_recovery/tests.rs`](../../crates/horizon-ui/src/app/browser_recovery/tests.rs)
  holds the file lock of a coordination queue from a helper thread that releases
  it on a signal, as step 2 requires. It also checks that the claim runs on the
  worker and that the request is still answered.
- `a_held_read_never_stalls_the_drop_frame` in
  [`file_drop/editor_panels/tests.rs`](../../crates/horizon-ui/src/app/file_drop/editor_panels/tests.rs)
  drops a file that is a named pipe and releases it from a helper thread. It
  checks that the frame that takes the drop returns, that the panel opens after
  the read, and that the panel has the size of the file.

## The guard

The rule of this guide is in the code. The module `ui_thread`
([`ui_thread.rs`](../../crates/horizon-core/src/ui_thread.rs)) reports a call
that can wait when it runs on the UI thread, and the tests of the window run it
in strict mode.

- `ui_thread::FrameScope::enter()` marks the current thread as inside a frame
  while the returned value lives. The entry points of the window hold one:
  `BridgeApp::ui`, `BridgeApp::raw_input_hook`, `BridgeApp::on_exit`,
  `poll_on_ui_thread` of the device request pump, and `HorizonApp::new`, which
  runs after the window exists. The headless frame helper of the UI tests holds
  one too. Scopes nest.
- `ui_thread::blocking(what)` goes first in a function that can wait. It does
  nothing outside a frame scope. Inside one it reports the wait and the caller
  location: it panics in strict mode, and it writes one `tracing::error!` for
  each call site for the life of the process otherwise.
- Strict mode is a process switch. The UI tests turn it on, and so does the
  start of Horizon when `HORIZON_UI_THREAD_GUARD=panic` is in the environment.
  Without that variable the guard writes the log line, which is the default.
- `ui_thread::exception(reason)` accepts the calls that can wait on this thread
  while the returned value lives. Each use is one line that
  `rg "ui_thread::exception\("` finds, and the reason is one entry of the list
  below. `scripts/check-ui-thread-exceptions.py` fails when the two lists do not
  name the same reasons, so the list cannot drift.

Rule for a new exception: take one only for work that the repository accepts on
the UI thread, with a reason that says why. Work that a later change moves to a
worker takes a reason that starts with `part 2:` and names its finding, so that
change finds the scope again. Do not accept work that this guide forbids without
that plan.

A crate that cannot reach `horizon-core` guards the call site instead:
`horizon-browser`, `horizon-browser-control` and `horizon-vnc` are below it, so
the guard sits in the `horizon-core` or `horizon-ui` function that calls them.
The `ManifestLock` of a video and the join of a VNC session are examples.

## Known UI-thread work

The guard accepts this work today, with the reason that the code writes. Each
line is one `ui_thread::exception` in the code. Do not copy it into new code.

- `terminal spawn: a terminal panel is a process`: `Terminal::spawn`
  ([`terminal/lifecycle.rs`](../../crates/horizon-core/src/terminal/lifecycle.rs))
  forks and starts the shell of a new panel, whichever path asked for it: a new
  panel, a restored board and a restart all end here. A panel is one process, so
  a frame that starts a panel owns this start.
- `exit cleanup: the window closes`: `run_exit_cleanup`
  ([`lifecycle/shutdown.rs`](../../crates/horizon-ui/src/app/lifecycle/shutdown.rs))
  waits for panels, for browsers and for the session writer. The window closes,
  and each wait has its own bound. One synchronous wait at exit has no time
  limit: `run_exit_cleanup` joins the worker thread of a saved session deletion.
  Each wait has its own limit, so together they can take longer.
- `board drop: a replaced board waits for its jobs`: the drop of `HostIo`
  ([`browser_host_io.rs`](../../crates/horizon-ui/src/app/browser_host_io.rs))
  waits for the coordination jobs of a board that goes, for at most the bound of
  an exit.
- `session switch: the old board is flushed for up to 2 s`: a session switch and
  a replacement of the board wait on the UI thread until the jobs of the browser
  coordination worker thread end
  (`finish_browser_host_io_before_replacement`). The window does not paint
  during this wait.
- `startup recovery: the repaired board is written before it opens`:
  `save_recovered_startup_runtime_state`
  ([`persistence.rs`](../../crates/horizon-ui/src/app/persistence.rs)) writes and
  renames the session file, without a sync. While the session writer thread
  writes that session, the save is refused, and the startup prompt offers to try
  again or to open the repaired board without saving it. The session auto-save
  queues the board for the session writer thread
  ([`writer.rs`](../../crates/horizon-core/src/session_store/writer.rs)) and does
  not write. This scope stays until the session writer answers a failed board
  write, because without that answer the two choices of the prompt cannot say
  what happened.
- `clipboard paste: the toolkit reads the desktop clipboard`: eframe and
  egui-winit read the desktop clipboard for a paste inside their own event
  handling, on the UI thread, before Horizon sees the text. Horizon asks for the
  paste in `clipboard_paste::request`
  ([`terminal_widget/clipboard_paste.rs`](../../crates/horizon-ui/src/terminal_widget/clipboard_paste.rs))
  and does no clipboard work itself. The framework offers no way to read the
  clipboard on a worker.

Horizon's own copies do not stay on the frame: the thread that owns the desktop
clipboard writes the text and the image of every copy
([`clipboard.rs`](../../crates/horizon-ui/src/clipboard.rs)).

Two frame paths that this guide listed before do not read a file any more: the
watch of the settings store and the refresh of the lease of a session. Both run
on a worker and report their outcome to a later frame.
