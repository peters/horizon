---
applyTo: "crates/horizon-core/src/browser/remote_host.rs,crates/horizon-core/src/browser/remote_host/**,crates/horizon-core/src/panel/spawn.rs,crates/horizon-core/tests/remote_host_panel.rs,crates/horizon-ui/src/browser_widget/chrome.rs"
---

# Trust boundary of the remote browser panel

A browser panel shows the page of a browser host on another machine and sends the
input of the person there, over one SSH channel that carries the MCP contract of
that host. The design is in the module comment of
`crates/horizon-core/src/browser/remote_host.rs`, and the procedure is
`docs/testing/procedures/remote-browser-panel.md`.

This boundary adds one client to the channel of
`docs/review/trust-boundaries/browser-remote-bridge.md`. That page owns the SSH
transport, the probe of the record of the host and the command line of the remote
server, in `crates/horizon-browser-control/src/remote_host.rs`. This page adds the
rules of the panel that uses that channel.

## Trusted: the SSH target and the actor

The SSH target comes from the settings entry `browser_host`. The panel takes it
as one value and refuses a target that `ssh` reads as an option, that holds a
space or a control character, and that is longer than 255 characters.

- `crates/horizon-core/src/panel/spawn.rs` creates a new browser panel on that
  entry through `start_remote_host`, with the profile of the entry as the target
  of its first page. A target that the rule above refuses gives no panel and no
  channel, so a value of the settings never becomes an argument of `ssh`.
- Only the preset path of a person sets `PanelOptions::follows_browser_host`, so
  only a panel that a person picks follows the entry. A page that an agent asks
  for stays on the browser of this computer, and the entry of that agent reaches
  the host over the bridge instead, so a signed-in profile of the person takes no
  page that an agent asked for.
- A panel that already exists keeps the backend that it was created with: a
  restore of a saved panel and a page of a local profile that a panel duplicates
  take the path they took before the entry existed, so a change of the entry
  moves no live page between machines.
- The actor of every page of a panel is the constant `horizon:person:browser-panel`.
  A person never supplies it, and no agent identity uses it.
- The panel uses the OpenSSH client of this computer. The host key is the host key
  that OpenSSH checks, and the panel adds no trust of its own.
- The panel holds no credential. A key, an agent and an SSH config belong to
  OpenSSH and to the person.

## Untrusted: the remote host, its record and its results

The remote machine writes the record of its profile host, and it answers every
call of the panel.

- The panel reads `host_instance` from `profile-host.json` over the SSH transport,
  through `manifest::valid_host_instance`, and it never takes the instance of this
  computer. The channel of the bridge holds that read, and the panel calls it.
- A result of a call is data, not a command. A frame is a PNG block that the host
  inlines, and the panel never reads a path on the remote machine: a private
  remote path in a result is ignored.
- A frame is decoded into the frame slot of the panel, and the bytes of that frame
  are the bytes that the page produced. The size of one line of the channel is
  bounded to 24 MiB, so one result can not grow the memory of the panel without
  end.
- The reason that the host gives for a refused call reaches the panel note
  unchanged. The panel shows that text and does not run it.
- The URL and the title of the page come from the host. They are text in the panel
  chrome, and the panel uses them for no path, no command and no decision.

## Untrusted: the input of the person

The input of the person becomes the calls of the contract, which are semantic:
a key press on the focused element, a page scroll, a navigation, a viewport.

- A pointer position never reaches the host. The panel refuses a press and a
  release of a pointer with the typed reason `remote_host_pointer_unsupported`,
  and the person sees one line that names the device panel of the machine as the
  way to click the page. A mouse move and a key release change nothing.
- Text crosses as one key press for each character that the US layout holds. A
  character that the layout does not hold is refused with
  `remote_host_text_unsupported`, and it is never sent as page script.
- A wheel event becomes a page scroll. The point in the event does not cross.
- A dropped local file, a video capture, a handoff and the native select are
  refused with `remote_host_command_unsupported`. The panel stages no file on the
  remote machine, so no copy of a local file leaves this computer through the
  channel of a panel.

## What the panel does not protect

- The remote host decides which profile a page belongs to and which page an
  actor sees. The panel adds no scope of its own, and it does not verify that the
  page of its frame is the page that the create asked for.
- One profile of the host shares its cookies and its storage with every page of
  that profile, including a page of an agent.
- A process of the operating-system user on the remote machine can read the
  runtime root of the host and write a request as any actor. Run the host as a
  user that untrusted processes can not become.
- The frames of the identity of the host are not authenticated beyond the SSH
  transport. A compromised remote machine can send any PNG as a frame.

## Lifecycle that a review must keep

- A panel starts one `ssh` process for its channel and no listener.
- No `ssh` of the channel runs on the UI thread. Creating a panel and the Retry of
  a person start a worker thread that reads the record and starts the server, and
  the panel draws the outcome as a state.
- Every `ssh` of the channel carries `-o BatchMode=yes`, a bounded
  `-o ConnectTimeout`, and a bounded server-alive setting, so a host that drops
  packets or asks a question fails in seconds and never waits for a prompt.
- The probe of the record has its own deadline, and the process is killed when the
  deadline passes. A host that stops answering is a typed reason, never a wait.
- The page of the host closes with the panel, and the profile of the host stays.
- A replacement channel closes the page of the channel that it replaces before it
  creates its own page, on the worker thread. A host takes one page of an engine
  at a time, so the order is what makes the Retry and the restart of a panel work.
- A stopped channel leaves the panel with a typed reason and one line that gives
  the SSH target. It retries only when the person asks for a retry.
- A local panel takes none of these paths: its backend, its capabilities and its
  pointer stay local.
