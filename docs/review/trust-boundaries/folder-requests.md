---
applyTo: "crates/horizon-core/src/folder_requests.rs,crates/horizon-ui/src/app/folder_requests.rs,crates/horizon-ui/src/app/startup_folder.rs,crates/horizon-ui/src/main.rs"
---

# Trust boundary of a folder request

`horizon <folder>` asks a Horizon of the same home to show a folder when one
already runs, and a start that names no folder asks that window to show itself.
The second start cannot reach the first one in memory, so it writes a request
into the home and the window that runs reads it. This boundary covers the
request, the answer and the window that applies them.

Read [`first-steps.md`](../first-steps.md) for the command that a person types.

## What is untrusted

Every file in the request folder of the home, `<home>/runtime/folder-requests/`:

- its name, its size, its type and its owner;
- every field of the request record: the verb, the folder and the time.

Any process of the same user can write there, and a program of that user can
send a request that no person typed. A person cannot use the folder to hide the
file from the person who reads it: the folder mode is `0700`, so a second user
of this computer cannot see or write the folder at all, and a file mode is
`0600`.

## What the code guarantees

1. **A request names one folder and one verb.** The record holds `version`,
   `verb`, `folder` and `requested_at`. The only verb is `show`. A record with
   another verb, another version, a relative folder, a folder that holds a
   control character, or a folder longer than `MAX_FOLDER_BYTES` is removed and
   applied to nothing. An empty folder is a request that names no folder: the
   window shows and raises its window and applies no other work, and it answers
   `window`. A window that ends with its window answers such a request with
   `refused` and the reason `keep_running_off`, and the caller then opens its own
   window as it did before this channel existed.
2. **No text of a request reaches a command line.** No field of a request
   becomes a program, an argument, an environment value, a preset name or a
   shell word. The window opens what a start with that folder opens: it shows
   the workspace of the folder, or it makes one with the first agent preset of
   the settings whose program this computer has, or a terminal in that folder.
   A person who receives a request for a folder therefore gets the panel that
   `horizon <folder>` gives, and no other one.
3. **The window resolves the folder again.** The caller's check is not used. The
   window requires an existing directory at the moment that it applies the
   request, through the same function that a start with a folder uses. A folder
   that is gone, or that is a file, is answered with `refused` and the reason
   `folder_gone`, and no panel opens.
4. **A request is young and bounded.** A request older than `REQUEST_MAX_AGE`
   (10 seconds) is dropped and removed, and its answer is not written. At most
   `MAX_REQUESTS_PER_TICK` (4) requests are handled in one tick, and the rest of
   that folder are removed without an apply.
5. **The read is bounded, and a bad file is removed without a full read.** A
   record is read through a reader that stops at `MAX_RECORD_BYTES` (4 096
   bytes). The window removes a request file whose name is not `<id>.request`
   with an id of the form of a UUID, whose type is not a regular file, that a
   symbolic link replaces, whose owner is not the effective user of the window,
   or whose mode carries a group or other bit. It removes a request that does
   not parse, and a request that does not pass rule 1. A claim file and an
   answer file of a caller or of a window that ended are removed when they are
   older than `REQUEST_MAX_AGE`.
6. **The folder of the home is private, and the window checks it.** When the
   caller makes the folder, it uses mode `0700`, whatever the umask. The window
   ignores a folder that is a symbolic link, that is not a directory, that holds
   a group or other bit, or that belongs to another user. It removes nothing in
   that case.
7. **One window takes one request.** A window claims a request by making a file
   of its own with `create_new`, which only one process can do. A claim is
   therefore exclusive, and a second window of the same home takes no request
   that a first one claimed. Horizon does not know which window has the keyboard
   focus, so either window may win; the board that wins is the board that shows
   the folder.
8. **A request that no window takes costs the caller a bounded wait.** The
   caller waits `CALLER_WAIT` (5 seconds) for the answer, then removes its own
   files and prints one sentence. It never starts a second window in that case,
   and it never waits without a bound. A start that names no folder and reads no
   answer opens its own window, as a start of this home does today.
9. **The window never waits on the folder on the UI thread.** Every read,
   claim, write and removal of the channel runs on the coordination worker
   thread, and the frame applies only the folder and the answer. A test runs a
   frame that takes a request in the strict mode of the UI-thread guard.

## What the code does not guarantee

- **It does not guarantee that a person sent the request.** A program of the
  same user can write the same record. It could also start the agent program
  itself, so this channel grants it nothing that it did not have.
- **It does not guarantee that the request reaches the window that a person
  looks at.** With two windows of one home, either one answers.
- **It does not guarantee an answer.** A window that is minimized, suspended or
  busy answers nothing, and the caller prints the sentence that says so.
- **It does not guarantee that every window of the home takes a request.** A
  window that runs with `--ephemeral` keeps no lease, so a start does not see it
  and such a window takes no request.
- **It does not guarantee that the folder is the folder that a person meant.**
  The window checks that the folder exists. It checks nothing about its content.
- **The panel that the request opens is the panel that a person could open.** It
  runs with the rights of the person in that folder, exactly as `horizon
  <folder>` does on an empty window.

## Review notes

- Keep one request equal to one folder. A field that names a program, a preset,
  an argument, a size or an action is a new boundary page and a new review.
- Apply the bounds to the bytes that are read, not to a size that was measured
  before the read.
- Do not read, claim, answer or remove a file of this channel on the UI thread.
- Keep the private modes of the folder and of both records, and keep the owner
  and the mode check before every read of a file that the channel did not
  write in this process.
