# Shared terminals

A shared terminal is a terminal panel of one computer that the other computers
of the same server can show and type in. The computer that runs the program of
the panel is the owner. A computer that shows the panel is a follower, and its
panel is a follow panel.

The server is the hub. The computers never reach each other: each one holds a
live link to the server, and the server relays the frames. The server writes
nothing of a terminal to a file. A screen, the output and the typed bytes go
through its memory only.

This page gives the design of the parts. The trust boundary is in
[`docs/review/trust-boundaries/shared-terminals.md`](../review/trust-boundaries/shared-terminals.md).
The server and its sync link are in [`horizon-server.md`](horizon-server.md).

## The parts

| Part | Code | Function |
|---|---|---|
| The frames and the relay | `crates/horizon-sync/src/live/` | The frames of a live link, the hub of the server, and the frames that wait for each link. |
| The live link of a computer | `crates/horizon-sync/src/supervisor/live.rs` | `LiveSupervisor` opens the link and opens it again. `LiveHandle` is the interface of the application. |
| The owner side | `crates/horizon-pty-tap/src/share.rs`, `crates/horizon-core/src/terminal/share.rs` | The outlet copies the output of a terminal. A screen draws its grid and sets its modes. |
| The follow panel | `crates/horizon-core/src/terminal/follow.rs`, `crates/horizon-core/src/panel/follow.rs`, `crates/horizon-core/src/board/follow.rs` | A terminal that shows a stream, keeps the bytes of the person for the stream, and starts no program. |
| The join | `crates/horizon-ui/src/app/shared_terminals.rs`, `crates/horizon-ui/src/app/shared_terminals/` | The application lists the terminals of this computer, sends their screens and output, and runs the follow panels of its window. |
| The settings and the window | `crates/horizon-core/src/config/server.rs`, `crates/horizon-ui/src/app/settings/server/sharing.rs`, `crates/horizon-ui/src/app/sidebar/computers.rs` | The name and the two switches of a computer, and the section **Computers** of the sidebar. |

## The live link

A computer holds two links to its server. The sync link carries the machine
list, the secrets and the settings. The live link carries the shared terminals.
It is a second transport: a second `ssh` child, or a second socket to the TCP
listener.

- **The greeting is the greeting of the sync link.** The live link sends the
  same `hello`: protocol version 4, the device name, and the token on a link of
  the listener. It sends no store id. A TCP link applies the address rule of
  the sync link before it sends one byte.
- **The answer of the server decides.** A server of this build answers version
  6 in `hello_ok`. A computer sends a live frame only to a server that answered
  6 or higher. An older server cannot read a live frame and would end the link.
- **An older server is not an error.** The computer closes the transport,
  reports the state `Unsupported` and asks again after 5 minutes. Its sync link
  does not change.
- **`open` makes the live link.** `open` must be the first request after the
  greeting. A link that sent another request first ends with `not_live`. A live
  request other than `open` on a link that is not a live link ends with
  `not_live` too.
- **A live link carries live requests only.** Another frame ends it with
  `live_link`. A second `open` changes nothing.
- **One live link for each computer.** A new live link of a device name ends
  the older one with `live_replaced`. The new link starts with an empty list.
- **At most 32 live links.** The server refuses one more with `live_limit`.

## The frames

A live frame is one frame of the link: a 4-byte length and one JSON document of
at most 256 KiB. A computer sends `live` frames, and the server sends
`live_event` frames. The bytes of a terminal are base64 text in the field
`data`. Each example below is one frame.

```json
{"live":{"open":{"label":"Computer A"}}}
{"live":{"panels":{"panels":[{"id":"p1","title":"build","kind":"shell","state":"","workspace":"work","cols":80,"rows":24,"typing":true}]}}}
{"live":{"follow":{"owner":"computer-a","panel":"p1"}}}
{"live":{"screen":{"panel":"p1","cols":80,"rows":24}}}
{"live":{"output":{"panel":"p1","data":"aGk="}}}
{"live":{"typed":{"owner":"computer-a","panel":"p1","data":"bHMN"}}}
{"live_event":{"watchers":{"panel":"p1","watchers":[{"device":"computer-b","label":"Computer B"}],"refresh":true}}}
{"live_event":{"screen":{"owner":"computer-a","panel":"p1","cols":80,"rows":24}}}
{"live_event":{"output":{"owner":"computer-a","panel":"p1","data":"aGk="}}}
{"live_event":{"typed":{"panel":"p1","from":"computer-b","label":"Computer B","data":"bHMN"}}}
{"live_event":{"ended":{"owner":"computer-a","panel":"p1","reason":"offline"}}}
```

### The requests of a computer

| Request | Fields | What it says | Bound |
|---|---|---|---|
| `open` | `label` | This link is a live link. `label` is the readable name of the computer. | 60 characters. |
| `panels` | `panels` | The full list of the panels that this computer shares. It replaces the last list. | 64 panels with different identifiers. |
| `follow` | `owner`, `panel` | Start the follow of one panel of the computer `owner`. A follow that the computer already has is a `refresh`. | 8 follows for each computer, and 4 followers for each panel. |
| `unfollow` | `owner`, `panel` | End that follow. | None. |
| `refresh` | `owner`, `panel` | Ask the owner for a new screen of a panel that this computer follows. | One in a second for one follower and one panel. |
| `screen` | `panel`, `cols`, `rows` | A new screen of a panel of this computer starts with this grid. The output that follows draws it. | 1 to 1000 columns and rows. |
| `output` | `panel`, `data` | Output of that panel. | 1 byte to 48 KiB before base64. |
| `ended` | `panel` | That panel is gone, and it leaves the list. `LiveHandle` does not send this request: it sends the list without the panel. | None. |
| `typed` | `owner`, `panel`, `data` | The bytes that the person typed in a panel that this computer follows. | 1 byte to 16 KiB before base64. |

### The events of the server

| Event | Fields | What it says |
|---|---|---|
| `opened` | `device`, `computers` | The answer to `open`: the device name of this link, and the list. |
| `computers` | `computers` | The computers that have a live link, each with its label and the panels that it shares. The computer that gets the list is in it. |
| `watchers` | `panel`, `watchers`, `refresh` | To the owner: the computers that follow this panel now. `refresh: true` asks for a new screen. An empty list says that no computer follows the panel. |
| `screen` | `owner`, `panel`, `cols`, `rows` | To a follower: a new screen of the panel starts with this grid. |
| `output` | `owner`, `panel`, `data` | To a follower: output of the panel. |
| `ended` | `owner`, `panel`, `reason` | To a follower: the follow ended or did not start. |
| `typed` | `panel`, `from`, `label`, `data` | To the owner: the bytes that the person of the computer `from` typed in the panel. |

The `reason` of `ended` is one of four values:

| Value | Cause |
|---|---|
| `closed` | The owner does not share the panel any more. |
| `offline` | The live link of the owner ended. |
| `not_found` | The owner has no live link, or it does not share that panel, or the panel is a panel of the computer that asked. |
| `limit` | The panel has 4 followers, or the computer that asked follows 8 panels. |

### The texts and their bounds

The server removes the control characters of each text and cuts the text at its
bound before it keeps or relays it. The bound is a number of characters.

| Text | Bound |
|---|---|
| The `label` of a computer | 60 |
| The device name in `owner` of a `follow` | 40 |
| The `id` of a panel | 64 |
| The `title` of a panel | 120 |
| The `kind` of a panel, such as `claude` or `shell` | 24 |
| The `state` of a panel, such as `working` or `blocked` | 24 |
| The `workspace` of a panel | 80 |

A panel with an empty identifier leaves the list, and so does a second panel
with an identifier that the list already has. The server sets `cols` and `rows`
of a panel to a value from 1 to 1000. The field `typing` of a panel says whether
the owner lets the other computers type in it. A request that names a computer
or a panel that the server does not hold changes nothing.

A frame with a field that the protocol does not know is not a frame of the
protocol. A frame whose `data` is not base64 is not one either. The server ends
that link with `bad_frame`. An `output` frame or a `typed` frame that is empty
or above its bound is different: the server drops the frame and keeps the link.

## The relay of the server

The hub is `crates/horizon-sync/src/live/hub.rs`. It is one value of the server
process, in the registry of the links. It holds the live link of each device
name, the list of each computer and the follows.

1. **The server stores nothing of a terminal.** The hub opens no file. A frame
   that waits for a link waits in memory. A log line of the hub names a device
   and a count, and never a title or a byte of a panel. The greeting of a live
   link writes the row of its computer in the store, as each greeting does.
2. **No request waits for another link.** The hub puts a frame into the outbox
   of its link and returns. One thread for each live link writes the frames of
   that outbox, one at a time.
3. **The list is one frame.** `panels` replaces the list of its computer. When
   the list changes, the followers of a panel that left get `ended` with
   `closed`, and each live link gets `computers`. The panels of all computers
   share 192 KiB, an equal part for each computer. The frame leaves out the
   panels of a computer that do not fit in its part.
4. **A follow has conditions.** The owner must be another computer with a live
   link, and its list must hold the panel. If not, the computer that asked gets
   `ended` with `not_found`. At a limit, it gets `ended` with `limit`. When a
   follow starts, the owner gets `watchers` with `refresh: true`.
5. **A screen starts the output.** A follower gets the output of a panel only
   after a `screen` of that panel. The server drops a screen and output of a
   panel that no computer follows.
6. **The server names the sender.** The server writes the `owner` of a `screen`
   and of `output` from the link that sent the request. It writes the `from`
   and the `label` of `typed` the same way. A frame cannot name another sender.
7. **Typed bytes have two conditions.** The computer that sent them must follow
   the panel now, and the list of the owner must say `typing: true` for that
   panel. If not, the server drops the frame.
8. **A follower asks for a new screen.** `refresh`, and a `follow` that the
   computer already has, send `watchers` with `refresh: true` to the owner. The
   server relays one such request in a second for one follower and one panel,
   and drops the others. The follower gets output until the new screen.
9. **The end of a follow tells the owner.** `unfollow` drops the screen and the
   output of that panel that wait for the follower. The owner gets `watchers`
   with the followers that stay.
10. **The end of a live link tells each side.** The panels of that computer
    leave the list. Its followers get `ended` with `offline`. The owners of the
    panels that it followed get `watchers`, and each other link gets
    `computers`.
11. **A removal ends the live link.** `device_remove` ends each open link of
    that computer with `removed`. The live link is one of them.

### The ceiling of one link

At most 1 MiB of frames wait for one live link
(`crates/horizon-sync/src/live/outbox.rs`). An `output` frame and a `typed`
frame count their base64 text and 512 bytes. Each other frame counts its JSON
and 8 bytes.

- **A screen and output can leave.** The server does not keep a `screen` or an
  `output` frame that does not fit under the ceiling. It also drops the screen
  and the output of that panel that wait for that link. The follower is then
  behind: it gets no more frames of that panel.
- **The other frames stay.** `opened`, `computers`, `watchers`, `ended` and
  `typed` are kept frames. The newest `computers` frame replaces one that waits.
  If a kept frame does not fit, the server first drops all the output that waits
  for that link. Each follow of that link that got output is then behind.
- **A computer that does not read loses its link.** If the kept frames alone
  do not fit under the ceiling, the server ends the link with `live_limit`.
- **A follower that reads again gets a new screen.** When less than 256 KiB
  wait for a link that is behind, the owner of each panel that it lost gets
  `watchers` with `refresh: true`. The follower gets output again from the next
  screen.

## The handle of a computer

`LiveSupervisor::start` starts one thread that opens the live link with the
settings of the sync link. `LiveHandle` is what the application calls
(`crates/horizon-sync/src/supervisor/live.rs`).

- **No call of the handle does I/O or waits for the link.** A call changes the
  state under a short lock, and the thread of the link sends the frames. The UI
  thread can call the handle.
- **The handle sends the wanted state again on each new link.** The label, the
  list of the panels and the follows are state of the handle. A new link sends
  `open`, the list and each follow. The screens, the output and the typed bytes
  that waited for the last link do not go to the new one.
- **Screens, output and typed bytes wait under a ceiling of 1 MiB.** `screen`
  and `output` answer `false` when the link is away or when the frames of the
  call do not fit. Nothing of that call is sent then. `output` divides its bytes
  into frames of at most 48 KiB.
- **Typed bytes never wait for a later link.** `typed` divides its bytes into
  frames of at most 16 KiB. The handle drops them while the link is away, while
  the link does not have the follow, and above the ceiling.
- **Events wait for the application under a ceiling of 4 MiB.** Above it, the
  reader of the link waits until the application takes the events with
  `take_events`, and no event is lost. The output then waits at the server,
  under the ceiling of the link.
- **The handle keeps what the application asked for.** It drops a `screen` and
  `output` of a panel that this computer does not want to follow. An `ended`
  event removes its follow from the wanted state. Only the newest list waits
  for the application, and the entry of this computer in it has `own: true`.
- **The state is one of four values.** They are `Connecting`, `Open`,
  `Unsupported` and `Down` with the reason of the last attempt. A change of the
  state is an event.
- **A lost link starts again on the delays of the sync link.** They are 3, 6,
  12 and 24 seconds, and then 30 seconds. An attempt that lasted 10 seconds
  starts the delays again. After `removed` and `not_logged_in` the handle makes
  no more attempt.

## The owner side

### The outlet

`ShareOutlet` is the end where the owner takes the output of one terminal for
its followers (`crates/horizon-pty-tap/src/share.rs`). The reader of the
terminal holds the same value and copies each read into it. This is the same
for a terminal of this process and for a terminal of a holder.

- **A terminal that no computer follows pays one atomic load for each read.**
  The reader takes no lock and copies nothing.
- **`set_wanted` says whether a computer follows.** It is one flag and not a
  count. A change empties the outlet, and the outlet copies nothing until a
  screen starts it.
- **The outlet holds only output that continues the newest screen.** It copies
  from the instant of a screen. It stops when more than 1 MiB wait, and when no
  computer follows.
- **Too much output drops all of it.** If a read does not fit in 1 MiB, the
  outlet drops what waits and stops. A stream with a hole draws a wrong screen.
- **`take` gives the bytes and one signal.** The bytes are the output since the
  last take, in the order of the program. `overflowed` is `true` while a
  computer follows and the outlet does not copy. It stays `true` until the next
  screen, so it is the one signal to send a screen.
- **A new grid stops the outlet.** A resize that changes the rows or the
  columns of the terminal stops it, because the followers draw the old grid. A
  change of the cell size alone does not.
- **A snapshot shares no output.** A terminal that shows a snapshot has an
  outlet that no reader fills. It shares its screen and no output of its
  process.
- **The outlet is the outlet of one terminal.** A panel that starts a new
  terminal has a new outlet.

### The screen and its order rule

`Terminal::share_screen` gives one screen: the columns, the rows and the bytes
that draw the grid and set its modes
(`crates/horizon-core/src/terminal/share.rs`). No byte is lost or sent two
times between a screen and the output after it:

1. The reader of the terminal holds the lease of the grid for one full pass. It
   gives every byte of that pass to the parser before it gives the lease back.
2. `share_screen` takes the lease, starts the outlet, locks the grid, and then
   gives the lease back. It writes the screen while it holds the lock.
3. The grid then holds every byte that the reader read before that instant. The
   outlet holds every byte that the reader reads after it.
4. The owner calls `share_screen` and `take` on one thread. It sends their
   results in the order of the calls.

The bytes of a screen have this order:

1. `ESC c`, a full reset. A terminal that showed an older screen then draws the
   same as a new terminal.
2. `CSI ? 1049 h`, when the program is on the alternate screen.
3. The rows of the screen of the program, with the colours of each cell and the
   attributes bold, dim, italic, underline, reverse and strikeout. The screen
   draws a tab cell and hidden text as blanks, so each cell keeps its column.
4. `CSI ? 6 h`, when the origin mode is on.
5. The position of the cursor. If the cursor waits behind the last column, the
   screen draws the cell under it one more time. The cursor of the follower then
   waits at the same place.
6. The pen: the colours and the attributes of the next output.
7. The modes of the table below, the flags of the keyboard protocol, and the
   shape of the cursor.

The rows are the rows of the program. They do not move when the person of the
owner reads the history of the terminal.

### The input modes in a screen

The terminal of the follower makes the bytes of each key, each paste and each
mouse report. A screen therefore sets the modes that the program asked for. It
writes one sequence for each mode that differs from the state of a new
terminal.

| Mode | Sequence |
|---|---|
| The cursor is hidden | `CSI ? 25 l` |
| Application cursor keys | `CSI ? 1 h` |
| Application keypad | `ESC =` |
| Mouse reports: click, drag, motion | `CSI ? 1000 h`, `CSI ? 1002 h`, `CSI ? 1003 h` |
| Mouse reports in UTF-8 and in SGR form | `CSI ? 1005 h`, `CSI ? 1006 h` |
| Focus reports | `CSI ? 1004 h` |
| Bracketed paste | `CSI ? 2004 h` |
| Alternate scroll is off | `CSI ? 1007 l` |
| Urgency hints are off | `CSI ? 1042 l` |
| Line wrap is off | `CSI ? 7 l` |
| Insert mode | `CSI 4 h` |
| Line feed and new line mode | `CSI 20 h` |
| The flags of the keyboard protocol: 1, 2, 4, 8 and 16 | `CSI > <sum> u` |
| The shape of the cursor, when it is not the default | `CSI <number> SP q` |

The modes that change how output is drawn come after the rows: line wrap,
insert, line feed and new line, and origin. The vi mode of the owner is not a
mode of the program, and a screen does not set it.

## The follow panel

### The terminal of a follower

`Terminal::spawn_follow` makes a terminal that starts no program and owns no
process of this computer (`crates/horizon-core/src/terminal/follow.rs`). Its
event loop gets one end of a socket pair as the link of a holder. The VT parser
and the status reader then run as they do for each other panel. The other end
belongs to the feed, and nothing reads the frames that the event loop writes to
it.

`FollowSpec` names the terminal. It holds the device name and the label of the
owner, and the identifier, the title and the kind of its panel. It also holds
the grid and `typing`. `spawn_follow` removes the control characters of each
text and cuts it at 200 characters. It sets the grid to 2 to 1000 columns and 1
to 500 rows. The kind selects the keyboard protocol that the owner gives a
panel of that kind.

`Board::create_follow_panel` opens the panel. The kind of the panel itself is
always the shell, so no agent rule of Horizon applies to it. The kind of
`FollowSpec` reads the title and the status reports of the stream. The panel
asks for the size of its grid, for 20 to 240 columns and 4 to 80 rows.

### The feed

`FollowFeed` is the stream side of a follower. Each clone feeds the same panel.

| Call | What it does |
|---|---|
| `screen(cols, rows)` | A new screen starts with this grid. |
| `output(bytes)` | Output of the stream, which the terminal shows. |
| `ended(reason)` | The stream ended. The terminal keeps its last screen, and the panel shows one line with the reason under the grid. |
| `take_typed()` | The bytes that the person typed since the last call: keys, pastes and mouse reports. |
| `take_lost()` | Whether the feed dropped output since the newest screen and the last call. |
| `closed()` | Whether the person closed the panel. |

- **No call waits.** The link does not block. Output that the link does not
  take waits in the feed. It goes to the link at the next call of `output`,
  `screen`, `take_typed` or `Terminal::process_events`.
- **At most 8 MiB of output wait.** Above that, the feed drops what waits and
  keeps the newest output. It sends `CAN` first, which ends a sequence that the
  cut left open, and `take_lost` answers `true`.
- **At most 1 MiB of typed bytes wait.** Above that, the feed drops the oldest
  bytes.
- **Typed bytes have two conditions.** The feed drops them when `typing` is
  `false`, and after `ended` until a new screen.

### The order rule of a new screen

No byte of a new screen is parsed on the old grid:

1. `screen` drops the output that waits in the feed.
2. It changes the grid in the same call, on the thread of the caller, under the
   lock of the grid.
3. Only then it gives the link the start of the new screen: `CAN`,
   `CSI ? 2026 l` and `ESC c`.
4. Output of the new screen can go to the link only after `screen` returned.
   Output of the old screen that the link still holds is parsed before the
   reset.

`CAN` ends an open sequence, and `CSI ? 2026 l` ends an open synchronized
update. The reset clears the screen, the history and the modes of the follower.

### What a follower never does

| A follower | Where |
|---|---|
| Writes no byte to its link. The bytes of the person go to the feed. | `Terminal::write_protocol` in `terminal/lifecycle.rs` |
| Answers no question of the program. Examples are the cursor position, the device attributes, a colour, the size of the text area and a clipboard read. The terminal of the owner answers. | `Terminal::answer_query` in `terminal/events.rs` |
| Sends no focus report. | `Terminal::set_focused` calls `write_protocol` |
| Takes no copy request of the program into the clipboard of this computer. | `Terminal::handle_event` in `terminal/events.rs` |
| Changes no grid. A resize returns at once, and the widget asks for none. Only a new screen changes the grid. | `terminal/resize.rs`, `crates/horizon-ui/src/terminal_widget/mod.rs` |
| Restarts no program, and reports no end of a program. | `Panel::restart_request`, `Panel::apply_restart`, `Board::begin_restart`, `panel/program_end.rs` |
| Signals no process at a close, and the exit of Horizon waits for no program of it. | `Terminal::hanging_child`, `Terminal::begin_work_shutdown` |
| Is in no saved session, so no start restores it. | `crates/horizon-core/src/runtime_state.rs` |

## The join in the application

The join is `crates/horizon-ui/src/app/shared_terminals.rs` with its modules
`link.rs`, `owner.rs` and `follower.rs`. `HorizonApp::poll_shared_terminals`
runs it one time in each frame, after the panels took the keys of that frame:

1. It takes the events of the link.
2. It gives the link the list and the output of this computer.
3. It runs the follow panels of this window.

Each call of the handle, of a feed and of an outlet returns at once, so the UI
thread does the full join.

### The settings of a computer

Three fields of the settings section `server` belong to shared terminals
(`crates/horizon-core/src/config/server.rs`). That section stays with one
computer, so no other computer changes them.

| Field | Default | What it sets |
|---|---|---|
| `computer_name` | Empty | The name that the other computers show for this computer. An empty name is the host name. The name is one line of at most 60 characters. A draft of the Settings window changes nothing on the live link; the link takes the name that the store saved. |
| `share_terminals` | `true` | The other computers see the terminals of this computer. |
| `remote_typing` | `true` | The other computers type in those terminals. It has no effect while `share_terminals` is `false`. |

A computer that follows a server therefore shares its terminals, with typing
on, until its person clears a switch. The store writes a field only when it
differs from its default, and a store without the fields gives the defaults.

The **Server** tab of the Settings window shows the section **Shared
terminals** for a computer that follows a server and has its login there
(`crates/horizon-ui/src/app/settings/server/sharing.rs`). The section holds one
sentence, the field **Name of this computer** and the two switches. The second
switch is disabled while the first one is off. The sentence is
`SHARED_TERMINALS_NOTICE`:

> The other computers that you link to this server can see the terminals of
> this computer and type in them. Anyone who gets into one of those computers,
> or into the server, can then run commands here.

The same sentence is in the section **Join with a link** of each other
computer. Such a computer follows no server, or the server removed it. The
sentence is also on the card of a request that names a server
(`crates/horizon-ui/src/app/settings_request.rs`). A change in the Settings
window applies to the join at once, before **Save**, as each setting of that
window does.

### The live link of the window

- **It starts with the sync link.** The first frame after the sync link of this
  computer exists starts a `LiveSupervisor` with the settings of that link. The
  label is `ServerConfig::computer_label`: the name of the settings, or the host
  name.
- **It ends with the sync link.** A stop of the sync link stops the live link
  (`HorizonApp::stop_shared_terminals`), and so does the start of a new sync
  link.
- **It starts again in two cases.** The first case is a sync link that was
  `away` in one frame and is not `away` in the next frame. The live link has no
  keepalive, so the return of the sync link is the sign that the live link can
  be dead. The second case is a new value of `computer_name` in the settings
  store: the label of the link is the saved name, so a key in the name field of
  the Settings window sends nothing and starts no link.
- **The old link stops on a thread of its own.** The stop of a `LiveSupervisor`
  waits for the thread of the link, so no frame does it.
- **A lost link forgets what only an open link holds.** This occurs at the
  states `Down` and `Unsupported` and at each new start. The join then ends
  each watch of this computer, empties the list of the other computers and ends
  the stream of each follow panel. A new link gets the list and the follows
  again.

### The owner side

`owner.rs` holds the list of this computer, the watch of each panel that another
computer follows, and the check of the typed bytes.

- **The list holds each terminal panel of the board.** A terminal panel is a
  panel with a terminal of its own, such as a shell, an agent, a command or an
  `ssh` panel. The list holds no follow panel, no card, no editor, no browser
  panel and no device panel. It holds at most 64 panels, and it is empty while
  `share_terminals` is `false`.
- **Each entry has eight fields.**
  - `id`: the local identifier of the panel, which a saved session keeps.
  - `title`: the title of the title bar.
  - `kind`: the name of the panel kind, such as `shell` or `claude`.
  - `state`: `working`, `background`, `done`, `blocked` or empty.
  - `workspace`: the name of the workspace.
  - `cols` and `rows`: the grid.
  - `typing`: the value of `remote_typing`.
- **A trigger asks for a new list.** There are four triggers:
  - terminal output in the frame;
  - a new count of panels or of workspaces;
  - a new version of the attention states;
  - a change of one of the two switches.

  One more build occurs each 2 seconds, for a new title and for a resize that
  printed nothing.
- **The list has two bounds.** The join builds it at most one time in 250 ms.
  It gives the list to the link only when the list differs from the last one.
- **Only a panel of the list has a watch.** A `watchers` event for another
  panel changes nothing, whatever the server names. A watch ends when its panel
  closed, left the list or has no follower. A watched panel that closed leaves
  the list of the link at once (`LiveHandle::ended`).
- **A watched panel is drained in each frame.** The window asks for a frame
  each 50 ms while a panel has a watch. Each drain calls `set_wanted(true)`, so
  a panel that started a new terminal continues its share with the new outlet.
- **A screen is due in five cases.**
  - A new follower came.
  - A `watchers` event says `refresh: true`.
  - A take says `overflowed`.
  - 250 ms went by after a new follower came.
  - 250 ms went by after the link refused a screen or output.

  The second screen of a new follower repairs a first screen that started in
  the middle of an escape sequence.
- **No output goes out while a screen is due.** The screen holds that output.
- **The owner checks a typed event.** `HorizonApp::shared_typed` writes the
  bytes into the program of the panel only when four conditions are true:
  1. `share_terminals` is `true`.
  2. `remote_typing` is `true`.
  3. The identifier of the event is in the list that the link has.
  4. A panel of the board has that identifier.

  It does not read the sender of the event.

### The follower side

`follower.rs` holds the other computers of the list and the follow panels of
this window.

- **The list of this window holds the other computers.** The join removes the
  entry of this computer.
- **A press on a row opens one follow panel.**
  `HorizonApp::open_shared_terminal` makes the panel in the active workspace
  with `Board::create_follow_panel`, and it calls `LiveHandle::follow`. A
  second press on the same row goes to the open panel. The kind of the list
  selects the keyboard protocol, and a kind that is no terminal kind is a
  shell.
- **The events of a stream go to the feed.** A `screen`, an `output` and an
  `ended` event go to the feed of the panel that follows that panel of that
  computer.
- **Typed bytes go out with their frame.** The join runs after the panels took
  the keys, so `take_typed` gives the bytes of that frame to
  `LiveHandle::typed`.
- **A follower asks for a new screen in two cases.** The first case is
  `take_lost`. The second case is a terminal that leaves the alternate screen
  on which its newest screen started, because the screen under it did not
  travel. The follower asks again each 1.2 seconds until a screen arrives.
- **A stream that ended keeps its panel.** The panel keeps its last screen and
  shows the cause under the grid, wrapped over at most four rows, with the whole
  line as hover text. The feed drops the bytes of the
  person. The panel follows again by itself when a later list names its panel.
  The cause `limit` is the exception: such a panel does not follow again.
- **A lost link ends each stream.** The cause is `this computer has no link to
  its server`. Each follow panel follows again when the list of a new link
  names its panel.
- **A closed follow panel ends its follow.** The join calls
  `LiveHandle::unfollow`.
- **`typing` is the value at the open of the panel.** A panel that the person
  opened while the owner had typing off drops the bytes of the person, also
  after the owner turns typing on.

### What the person sees

- **The section Computers of the sidebar.**
  `crates/horizon-ui/src/app/sidebar/computers.rs` draws it under the machines.
  It shows while the list holds another computer, and while the live link is
  down. Each computer has its name and one row for each panel that it shares,
  with the mark of its state. A press on a row opens its follow panel. The
  hover text of a row says whether the person can type in it.
- **The line of a link that is down.** The section shows
  `Shared terminals: no link` from the state `Down` until the link opens or
  starts again, and the reason of the state under it in at most three rows, with
  the whole reason as hover text. An older server gives the state `Unsupported`,
  which shows no line and no section.
- **The chip of a watched panel.** Its title bar says `seen by <name>` for one
  follower, and `seen by <n> computers` for more than one.
- **The chip of a follow panel.** Its title bar says `on <name>`. It says
  `on <name> · view only` while the list says that typing is off, and when the
  panel was opened without typing.
- **A panel shows one chip.** The chip of a shared terminal takes the place of
  two other chips. They are the chip of the agent that opened the panel and the
  chip of its machine (`crates/horizon-ui/src/app/panels.rs`).
- **Each text of another computer is cut.** The window shows a name, a title
  and a reason without control characters, at 80 characters at most (`shown`).
  The hover text of the name of a computer is the exception: it is the device
  name as the list gave it.

## The limits that are known

- **The live link has no keepalive.** No request of a live link has a bound. A
  computer can go away and not close its transport. It then stays in the list
  until that transport fails or its device opens a new live link. On `ssh`, the
  keepalive of `ssh` ends a dead transport. On TCP, no timer finds a dead path:
  the link ends when a read or a write fails.
- **One device name in two programs.** Two Horizon programs with one device name
  each end the live link of the other at each attempt, because the newer link
  wins. The state of each one goes between `Open` and `Down`.
- **The alternate screen.** A screen holds the grid that the program shows now.
  When the program is on the alternate screen, a screen does not hold the
  screen under it. After the program leaves the alternate screen, the follower
  shows an empty screen, and its cursor can be on another row, until the next
  screen. The follower asks for that screen when its terminal leaves the
  alternate screen on which its newest screen started.
- **A sequence that is not complete.** The parser of the reader is not
  accessible. A screen can start while the parser holds part of an escape
  sequence or a synchronized update. The follower then gets the end without the
  start, and it can draw wrong text until the next screen. No code finds this
  condition. The second screen of a new follower repairs it for a join, and
  the other screens have no second screen.
- **The size of the panel of a follower.** The grid of a follower is the grid of
  the owner, and the person cannot change it. A grid above 240 columns or 80
  rows opens at that size, and the panel clips what does not fit in its body.
- **A grid above the bounds.** The server sets the grid of a `screen` to at
  most 1000 columns and rows, and the feed sets at most 500 rows. The bytes of
  a screen draw the grid of the owner. A follower of a larger grid draws them
  on a smaller grid.
- **The history starts at the newest screen.** Each screen starts with a reset
  that clears the history of the follower. The server relays a screen to each
  follower of the panel that is not behind. A new follower, a refresh, a resize
  of the owner and too much output therefore clear the history of those
  followers.
- **The stack of the keyboard flags.** A screen sets the flags that are active.
  It does not hold the flags under them. After the program removes the active
  flags, the follower has no flags until the next screen. The owner has the
  flags under them.
- **What a screen does not hold.** A screen does not hold these items:
  - The history and the scroll region.
  - The tab stops and the character sets.
  - The saved cursor.
  - A changed palette and the title.
  - Hyperlinks, and the style and the colour of an underline.
- **A screen has no fixed bound.** Its bytes increase with the cells of the
  grid. One call of `LiveHandle::output` refuses bytes that do not fit under
  the ceiling of 1 MiB. The join then makes the screen again each 250 ms, and
  no bound ends that. A screen that does not fit in what is free of the 1 MiB
  of a follower at the server puts that follower behind again.
- **One switch for all terminals.** `share_terminals` shares each terminal
  panel of the computer or none. No setting shares one panel or keeps one panel
  out, and the list also holds the agent panels and the `ssh` panels.
- **Typing that was off at the open.** A follow panel keeps the `typing` value
  of its open. The person closes the panel and opens the row again to type
  after the owner turned typing on.
- **The owner does not see who types.** `shared_typed` does not read the sender
  of a `typed` event, and the window shows no mark while another computer
  types. The chip of the panel names the followers, not the computer that
  types.
- **The name field starts the link again at each change.** A change of the
  field **Name of this computer** applies before **Save**. Each new value
  starts the live link again, so the followers of this computer lose their
  streams for a moment at each change.
- **A short loss of the sync link.** The join reads the state of the sync link
  one time in each frame. It does not see a sync link that is away for less
  than one frame, and the live link does not start again then.
- **An older server shows nothing.** The state `Unsupported` shows no line and
  no section, so the person cannot tell an older server from a server with no
  other computer.
- **The list is built each 2 seconds.** That build reads each panel of the
  board, also when nothing changed.

## The constants

| Constant | Value | Code |
|---|---|---|
| One frame | 256 KiB | `MAX_FRAME_BYTES`, `crates/horizon-sync/src/frame.rs` |
| Output in one frame | 48 KiB | `MAX_LIVE_OUTPUT_BYTES` |
| Typed bytes in one frame | 16 KiB | `MAX_LIVE_TYPED_BYTES` |
| Panels that one computer shares | 64 | `MAX_LIVE_PANELS` |
| Panels that one computer follows | 8 | `MAX_LIVE_FOLLOWS` |
| Computers that follow one panel | 4 | `MAX_LIVE_WATCHERS` |
| Live links of one server | 32 | `MAX_LIVE_LINKS` |
| Panels in the list of the computers | 192 KiB | `LIST_BYTES` |
| Frames that wait for one live link at the server | 1 MiB | `CEILING_BYTES` |
| A link that was behind is ready again | Below 256 KiB | `RESUME_BYTES` |
| One refresh for one follower and one panel | 1 second | `REFRESH_EVERY` |
| Frames that wait for the link at a computer | 1 MiB | `QUEUE_CEILING_BYTES` |
| Events that wait for the application | 4 MiB | `EVENT_CEILING_BYTES` |
| The next attempt after an older server | 5 minutes | `UNSUPPORTED_RETRY` |
| Output that waits in the outlet | 1 MiB | `SHARE_BUFFER_BYTES` |
| Output that waits in the feed | 8 MiB | `MAX_WAITING_OUTPUT` |
| Typed bytes that wait in the feed | 1 MiB | `MAX_TYPED` |
| The grid of a follower | 1000 columns, 500 rows | `MAX_FOLLOW_COLS`, `MAX_FOLLOW_ROWS` |
| One text of a follower | 200 characters | `MAX_TEXT` |
| The name of a computer in the settings | 60 characters | `MAX_COMPUTER_NAME_CHARS` |
| The shortest time between two builds of the list | 250 ms | `LIST_EVERY` |
| The longest time without a build of the list | 2 seconds | `LIST_CHECK` |
| The frames of a window with a watched panel | Each 50 ms | `DRAIN_EVERY` |
| The second screen of a new follower, and the next screen after a refused frame | 250 ms | `NEXT_SCREEN` |
| The next request of a follower for a screen | 1.2 seconds | `ASK_AGAIN` |
| One text of another computer in the window | 80 characters | `SHOWN_CHARS` |
