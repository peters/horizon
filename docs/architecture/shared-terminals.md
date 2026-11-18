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
| The frames and the relay | `crates/horizon-sync/src/live/` | The frames of a live link (`protocol.rs`), the hub of the server (`hub.rs`), the frames that wait for one link (`outbox.rs`), the loop and the sign of life of one link (`relay.rs`, `alive.rs`). |
| The live link of a computer | `crates/horizon-sync/src/supervisor/live.rs` | `LiveSupervisor` opens the link and opens it again. `LiveHandle` is the interface of the application. |
| The owner side | `crates/horizon-pty-tap/src/share.rs`, `crates/horizon-core/src/terminal/share.rs` | The outlet copies the output of a terminal. A screen draws its grid and sets its modes. |
| The follow panel | `crates/horizon-core/src/terminal/follow.rs`, `crates/horizon-core/src/panel/follow.rs`, `crates/horizon-core/src/board/follow.rs` | A terminal that shows a stream, keeps the bytes of the person for the stream, and starts no program. |
| The join | `crates/horizon-ui/src/app/shared_terminals.rs`, `crates/horizon-ui/src/app/shared_terminals/` | The window lists the terminals of this computer. The share pump, a thread of its own, sends their screens and output and runs the follow panels. |
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
  9 in `hello_ok`. A computer sends a live frame only to a server that answered
  version 9 or higher (`LIVE_PROTOCOL_VERSION`). An older server cannot read a
  live frame and would end the link.
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
  The program that loses the link shows `another Horizon of this computer holds
  the live link` and asks again after 60 seconds, and after 5 minutes for each
  attempt after that.
- **Each side says that it is alive.** Each side sends a `heartbeat` frame
  while it has no other frame for 10 seconds, and a link that reads no frame
  for 30 seconds ends. Thus a computer that goes away leaves the list of the
  others in about half a minute. [The sign of life](#the-sign-of-life) gives
  the bounds and the reasons.
- **At most 32 live links.** The server refuses one more with `live_limit`.

### The sign of life

Each side of a live link keeps the two times of that link: when it last read a
frame and when it last sent one (`crates/horizon-sync/src/live/alive.rs`,
`Limits`).

| Bound | Value | What it does |
|---|---|---|
| The tick | 1 second | How often a side examines its two times. |
| The period of a heartbeat | 10 seconds | A side that sent no frame for this time sends `heartbeat`. |
| The silence that ends a link | 30 seconds | A side that read no frame for this time ends the link. |

The server ends a silent link with `live_silent` and the reason `This computer
sent no frame for thirty seconds.` The hub then removes the link as it removes
any link that ended: the panels of that computer leave the list, its followers
get `ended` with `offline`, and the owners that it followed get `watchers`.
The program of the silent computer ends its own link with the reason `the
server sent no frame for 30 seconds` and makes a new attempt on the delays of
[the handle of a computer](#the-handle-of-a-computer).

`heartbeat` is the sign of life and nothing else. The hub drops one that a
computer sends, and a computer takes one that the server sends as no event.
Both times rise at every frame of any kind, so a link that carries a screen or
a list needs no heartbeat.

## The frames

A live frame is one frame of the link: a 4-byte length and one JSON document of
at most 256 KiB. A computer sends `live` frames, and the server sends
`live_event` frames. The bytes of a terminal are base64 text in the field
`data`. Each example below is one frame.

```json
{"live":{"open":{"label":"Computer A"}}}
{"live":{"panels":{"panels":[{"id":"p1","title":"build","kind":"shell","state":"","workspace":"work","cols":80,"rows":24,"typing":true}]}}}
{"live":{"follow":{"owner":"computer-a","panel":"p1"}}}
{"live":{"resize":{"owner":"computer-a","panel":"p1","cols":132,"rows":43}}}
{"live":{"screen":{"panel":"p1","cols":80,"rows":24,"fresh":true,"everyone":false}}}
{"live":{"output":{"panel":"p1","data":"aGk=","replay":false}}}
{"live":{"typed":{"owner":"computer-a","panel":"p1","data":"bHMN"}}}
{"live":"heartbeat"}
{"live_event":{"watchers":{"panel":"p1","watchers":[{"device":"computer-b","label":"Computer B"}],"refresh":true}}}
{"live_event":{"screen":{"owner":"computer-a","panel":"p1","cols":80,"rows":24,"fresh":true,"everyone":false}}}
{"live_event":{"output":{"owner":"computer-a","panel":"p1","data":"aGk=","replay":false}}}
{"live_event":{"typed":{"panel":"p1","from":"computer-b","label":"Computer B","data":"bHMN"}}}
{"live_event":{"resized":{"panel":"p1","from":"computer-b","label":"Computer B","cols":132,"rows":43}}}
{"live_event":{"ended":{"owner":"computer-a","panel":"p1","reason":"offline"}}}
{"live_event":"heartbeat"}
```

### The requests of a computer

| Request | Fields | What it says | Bound |
|---|---|---|---|
| `open` | `label` | This link is a live link. `label` is the readable name of the computer. | 60 characters. |
| `panels` | `panels` | The full list of the panels that this computer shares. It replaces the last list. | 64 panels with different identifiers. |
| `follow` | `owner`, `panel` | Start the follow of one panel of the computer `owner`. A follow that the computer already has is a `refresh`. | 8 follows for each computer, and 4 followers for each panel. |
| `unfollow` | `owner`, `panel` | End that follow. | None. |
| `refresh` | `owner`, `panel` | Ask the owner for a new screen of a panel that this computer follows. | One in a second for one follower and one panel. |
| `resize` | `owner`, `panel`, `cols`, `rows` | The person of this computer typed in a panel that it follows, and the body of its panel fits this grid better: the program of the owner takes the grid until its own person types. | 2 to 1000 columns and 1 to 500 rows, and two requests a second for one follower and one panel. |
| `screen` | `panel`, `cols`, `rows`, `fresh`, `everyone` | A new screen of a panel of this computer starts with this grid. The output that follows draws it. `fresh` says that the follower forgets its screen, its history and its modes. `everyone` says that each follower of the panel takes this screen. | 2 to 1000 columns and 1 to 500 rows. |
| `output` | `panel`, `data`, `replay` | Output of that panel. `replay: true` says that these bytes draw the screen that came before them. | 1 byte to 48 KiB before base64. |
| `ended` | `panel` | That panel is gone, and it leaves the list. `LiveHandle` does not send this request: it sends the list without the panel. | None. |
| `typed` | `owner`, `panel`, `data` | The bytes that the person typed in a panel that this computer follows. | 1 byte to 16 KiB before base64. |
| `heartbeat` | None | The sign of life of this computer. It says that the link is there, and nothing else. | One in 10 seconds. |

### The events of the server

| Event | Fields | What it says |
|---|---|---|
| `opened` | `device`, `computers` | The answer to `open`: the device name of this link, and the list. |
| `computers` | `computers` | The computers that have a live link, each with its label and the panels that it shares. The computer that gets the list is in it. |
| `watchers` | `panel`, `watchers`, `refresh` | To the owner: the computers that follow this panel now. `refresh: true` asks for a new screen. An empty list says that no computer follows the panel. |
| `screen` | `owner`, `panel`, `cols`, `rows`, `fresh`, `everyone` | To a follower: a new screen of the panel starts with this grid. `fresh` says that the follower forgets its screen, its history and its modes, and `everyone` says who takes this screen. |
| `output` | `owner`, `panel`, `data`, `replay` | To a follower: output of the panel. `replay: true` says that these bytes draw the screen that came before them. |
| `ended` | `owner`, `panel`, `reason` | To a follower: the follow ended or did not start. |
| `typed` | `panel`, `from`, `label`, `data` | To the owner: the bytes that the person of the computer `from` typed in the panel. |
| `resized` | `panel`, `from`, `label`, `cols`, `rows` | To the owner: the person of the computer `from` typed in the panel that it follows, and the body of its panel fits this grid better. The program of the panel takes the grid until the person of the owner types there. |
| `heartbeat` | None | The sign of life of the server, for a link with nothing else to carry. |

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
of a panel to 2 to 1000 columns and 1 to 500 rows, with
`horizon_core::bounded_screen_grid`, which a follower applies to its own grid
too. The grid of a `screen` and the grid of a panel therefore have one pair of
bounds. The field `typing` of a panel says whether the owner lets the other
computers type in it. A request that names a computer or a panel that the
server does not hold changes nothing.

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
   panel that no computer follows. A screen with `everyone: true` reaches each
   follower of the panel, and one with `everyone: false` reaches only the
   followers that wait for a screen. A `fresh: true` screen makes each of them
   forget its screen, its history and its modes.
6. **The server names the sender.** The server writes the `owner` of a `screen`
   and of `output` from the link that sent the request. It writes the `from`
   and the `label` of `typed` the same way. A frame cannot name another sender.
7. **Typed bytes have two conditions.** The computer that sent them must follow
   the panel now, and the list of the owner must say `typing: true` for that
   panel. If not, the server drops the frame.
8. **A follower asks for a new screen.** `refresh`, and a `follow` that the
   computer already has, send `watchers` with `refresh: true` to the owner. The
   server relays one such request in a second for one follower and one panel,
   and drops the others. The request also drops the screen and the output of
   that panel that wait for the follower, so the follower gets no output until
   the new screen.
9. **The end of a follow tells the owner.** `unfollow` drops the screen and the
   output of that panel that wait for the follower. The owner gets `watchers`
   with the followers that stay.
10. **The end of a live link tells each side.** The panels of that computer
    leave the list. Its followers get `ended` with `offline`. The owners of the
    panels that it followed get `watchers`, and each other link gets
    `computers`.
11. **A removal ends the live link.** `device_remove` ends each open link of
    that computer with `removed`. The live link is one of them.
12. **The replay goes to the followers that took the screen.** An `output`
    frame with `replay: true` draws the screen that came before it. The server
    relays such a frame only to a follower whose newest screen is still
    arriving, and live output clears that mark. A follower that missed the
    screen therefore skips the bytes that would draw it.
13. **A follower gives the program a grid.** `resize` reaches the owner of a
    panel that this computer follows while the list of the owner says
    `typing: true` for that panel. The server cuts the grid at its bounds and
    passes two requests a second for one follower and one panel. It drops a
    request that comes sooner, and one of a computer that does not follow the
    panel. [The grid of the program](#the-grid-of-the-program) gives the rule.

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
- **The handle says that it is alive, and it ends a silent link.** It sends
  `heartbeat` while it sent no frame for 10 seconds, and it ends the link when
  the server sent no frame for 30 seconds. [The sign of life](#the-sign-of-life)
  gives the bounds.
- **A lost link starts again on the delays of the sync link.** They are 3, 6,
  12 and 24 seconds, and then 30 seconds. An attempt that lasted 10 seconds
  starts the delays again. A link that the server ended with `live_replaced`
  waits 60 seconds, and 5 minutes for each attempt after that, so the two
  programs of one device name stop to end the link of each other. A server that
  answered an older version gets one attempt each 5 minutes. After `removed`
  and `not_logged_in` the handle makes no more attempt.

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

`Terminal::share_screen(fresh)` gives one screen: the columns, the rows and the
bytes that draw the grid and set its modes
(`crates/horizon-core/src/terminal/share.rs`). A fresh screen starts with a
full reset and the history of the terminal; a screen that is not fresh keeps
the history of its follower and draws the rows in place. No byte is lost or
sent two times between a screen and the output after it:

1. The reader of the terminal holds the lease of the grid for one full pass. It
   gives every byte of that pass to the parser before it gives the lease back.
2. `share_screen` takes the lease, starts the outlet, locks the grid, and then
   gives the lease back. It writes the screen while it holds the lock.
3. The grid then holds every byte that the reader read before that instant. The
   outlet holds every byte that the reader reads after it.
4. The owner makes the screen and calls `take` on one thread. It sends their
   results in the order of the calls.

`Terminal::share_source` gives a `ShareSource`: the outlet, the way to the
screen and the way to the program of one terminal. It goes to another thread,
and `ShareSource::screen` gives the same screen there. The share pump of the
application is that one thread. A follower has no source.

The bytes of a fresh screen have this order:

1. `ESC c`, a full reset. A terminal that showed an older screen then draws the
   same as a new terminal.
2. The alternate screen: `CSI ? 1049 h`, when the program is on it.
3. The lines above the viewport, oldest first, each one followed by `CR LF`.
   They scroll into the history of the follower, whose viewport then ends under
   them. A program on the alternate screen has no such lines.
4. The rows of the screen of the program, with the colours of each cell and the
   attributes bold, dim, italic, underline, reverse and strikeout. The screen
   draws a tab cell and hidden text as blanks, so each cell keeps its column.
   `CR LF` takes the viewport to the bottom row when the last rows paint
   nothing.
5. `CSI ? 6 h`, when the origin mode is on.
6. The position of the cursor. If the cursor waits behind the last column, the
   screen draws the cell under it one more time. The cursor of the follower then
   waits at the same place.
7. The pen: the colours and the attributes of the next output.
8. The modes of the table below, the flags of the keyboard protocol, and the
   shape of the cursor.

A screen that is not fresh draws only the alternate screen, the rows and the
modes:

1. `CSI ? 1049 h`, on the alternate screen, or `CSI ? 1049 l` on the primary
   screen. The rows belong on the screen that this sequence names.
2. The pen goes to its default, and the modes that change how a row is drawn
   go off: `CSI 4 l`, `CSI ? 6 l`, `CSI ? 7 l` and `CSI 20 l`. Then each row
   goes to its own place (`CSI <row> ; 1 H`, `CSI 2 K`, the cells). The history
   and the screen of the follower stay as they are, so a reset and a scroll are
   not part of such a screen.
3. The mode of the origin, the position of the cursor, the pen, every mode of
   the table below in its set or unset form, the flags of the keyboard
   protocol, and the shape of the cursor.

The rows are the rows of the program. They do not move when the person of the
owner reads the history of the terminal.

### The input modes in a screen

The terminal of the follower makes the bytes of each key, each paste and each
mouse report. A screen therefore sets the modes that the program asked for. A
fresh screen writes one sequence for each mode that differs from the state of a
new terminal, and a screen that is not fresh writes one sequence for each mode,
in its set or unset form, because it replaces the modes of the follower.

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
| The flags of the keyboard protocol: 1, 2, 4, 8 and 16 | `CSI > <sum> u` on a fresh screen, and `CSI = <sum> u` on one that is not |
| The shape of the cursor, when it is not the default | `CSI <number> SP q` |
| The shape of the cursor is the default, on a screen that is not fresh | `CSI 0 SP q` |

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
| `screen(fresh, cols, rows)` | A new screen starts with this grid. A fresh screen makes the terminal forget its screen, its history and its modes. |
| `output(bytes)` | Output of the stream, which the terminal shows. |
| `ended(reason)` | The stream ended. The terminal keeps its last screen, and the panel shows one line with the reason under the grid. |
| `take_typed()` | The bytes that the person typed since the last call: keys, pastes and mouse reports. |
| `take_lost()` | Whether the feed dropped output since the newest screen and the last call. |
| `closed()` | Whether the person closed the panel. |
| `on_alternate_screen()` | Whether the terminal shows the alternate screen now. |
| `grid()` | The grid of the newest screen of the stream. |
| `fit()` and `set_fit(cols, rows)` | The grid that the body of the panel fits, which its widget writes and the next ask for a grid reads. |

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
- **The feed asks for the frame of its stream.** `screen` and `output` arm the
  output wake of the terminal, so the caller asks the window for nothing.

### The order rule of a new screen

No byte of a new screen is parsed on the old grid:

1. `screen` drops the output that waits in the feed.
2. It changes the grid in the same call, on the thread of the caller, under the
   lock of the grid.
3. Only then it gives the link the start of the new screen. A fresh screen
   starts with `CAN`, `CSI ? 2026 l` and `ESC c`; a screen that is not fresh
   starts with `CAN` and `CSI ? 2026 l` alone.
4. Output of the new screen can go to the link only after `screen` returned.
   Output of the old screen that the link still holds is parsed before the
   reset.

`CAN` ends an open sequence, and `CSI ? 2026 l` ends an open synchronized
update. `ESC c` is the full reset that clears the screen, the history and the
modes of the follower, so only a fresh screen has it. A screen that is not
fresh draws each row at its place and leaves the history of the follower as it
is.

### What a follower never does

| A follower | Where |
|---|---|
| Writes no byte to its link. The bytes of the person go to the feed. | `Terminal::write_protocol` in `terminal/lifecycle.rs` |
| Answers no question of the program. Examples are the cursor position, the device attributes, a colour, the size of the text area and a clipboard read. The terminal of the owner answers. | `Terminal::answer_query` in `terminal/events.rs` |
| Sends no focus report. | `Terminal::set_focused` calls `write_protocol` |
| Takes no copy request of the program into the clipboard of this computer. | `Terminal::handle_event` in `terminal/events.rs` |
| Changes no grid of its own terminal. A resize returns at once, and a new screen of the stream sets the grid. The follower asks the owner for the grid of its own body. | `terminal/resize.rs`, `FollowFeed::screen`, `FollowFeed::set_fit` |
| Restarts no program, and reports no end of a program. | `Panel::restart_request`, `Panel::apply_restart`, `Board::begin_restart`, `panel/program_end.rs` |
| Signals no process at a close, and the exit of Horizon waits for no program of it. | `Terminal::hanging_child`, `Terminal::begin_work_shutdown` |
| Is in no saved session, so no start restores it. | `crates/horizon-core/src/runtime_state.rs` |

## The join in the application

The join is `crates/horizon-ui/src/app/shared_terminals.rs` with its modules
`link.rs`, `list.rs`, `pump.rs`, `owner.rs` and `follower.rs`. It has two
parts: the share pump and the part of the window.

### The share pump

A window that the desktop does not show can get no frame. On Wayland, a redraw
waits for a frame callback, and the compositor sends none for a surface that it
does not present. Thus no byte of a shared terminal waits for a frame. One
thread, the share pump (`pump.rs`), moves each byte.

- **The pump starts and stops with the live link.** Each start of a live link
  starts a pump, and each stop of a live link stops its pump. The thread of a
  new pump first ends the pump before it, so two pumps never take the output of
  one terminal.
- **One pass does all the work.** A pass does these steps in this order:
  1. It takes the events of the link.
  2. It copies what the window wants, if the window wrote a new value.
  3. It applies the events.
  4. It gives the link a new list, and it starts each new follow.
  5. It sends the screens and the output of the watched panels.
  6. It runs the follow panels.
  7. It gives the window what it found, and it asks for one frame.
- **No pass waits.** Each call of the handle, of a feed and of an outlet
  returns at once.
- **The pump holds only values that go to another thread.** For a terminal
  panel of this computer, it holds the identifier and the `ShareSource`. For a
  follow panel, it holds the identifiers and the `FollowFeed`.
- **The pump sleeps while it has no stream.** It has a stream while a panel of
  this computer has a watch, and while a follow panel is open. An event of the
  link wakes the pump, and so does a notice of the window. While it has a
  stream, the pump also runs each 25 ms (`PERIOD`). The reader of a terminal
  fills an outlet with no notice, and so does a person who types in a follow
  panel.
- **The window and the pump share two values.** The window writes `Wanted`: the
  terminal panels of this computer, the follow panels and the two switches.
  The pump writes `Found`: the other computers, the chip texts and the reason
  of a link that is down. A lock is held for one copy only. The pump takes a
  lock only when the lock is free, and it comes back 25 ms later when the
  window held it. Thus the pump never waits for the window, and a frame waits
  for one copy at most.
- **The pump asks for a frame only when `Found` changed.**

### The part of the window

`HorizonApp::poll_shared_terminals` runs one time in each frame, after the
panels took the keys of that frame. It does the work that needs the board:

1. It starts and stops the live link with the sync link.
2. It builds the list of the terminal panels of this computer (`list.rs`).
3. It gives the pump a new list, a changed switch and each follow panel that
   opened or closed. While a follow panel is open, it also sends a notice in
   each frame, so the keys of that frame go out at once.
4. It takes `Found` for the section of the sidebar and for the chips.

### What a window with no frame does

A window that gets no frame continues each stream on the pump. The part of the
window runs in the pass that eframe calls in place of a frame:
[the UI thread guide](ui-thread.md#the-pass-of-a-window-that-draws-no-frame)
gives its rules. An event of a terminal asks for that pass at once. A repaint
request also wakes it, as the pump makes one when `Found` changed. On Wayland,
the pass of a repaint request comes 250 ms after the request. Thus, these
things need no frame:

- The list: a panel that a request opened or closed, a panel that started a
  new terminal, a new title, a new state and a new grid.
- The new start of the live link after the sync link came back, when the
  worker of the sync link asks for a repaint.
- The answers of a terminal to the questions of its program, such as the
  position of the cursor. This is true for each terminal of the window, shared
  or not.

These things wait for the next frame of that window:

- A changed switch and a new name of the computer. A person changes them in
  the window.
- The start of the sync link, which `poll_server_sync` makes in a frame.
- The section of the sidebar and the chips.

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
  `away` in one frame and is not `away` in the next frame. The second case is a
  new value of `computer_name` in the settings store: the label of the link is
  the saved name, so a key in the name field of the Settings window sends
  nothing and starts no link.
- **The old link and its pump stop on a thread of their own.** The stop of a
  `LiveSupervisor` and the stop of a pump wait for a thread, so no frame does
  them. A pump stops in one pass at most.
- **A lost link forgets what only an open link holds.** This occurs at the
  states `Down` and `Unsupported` and at each new start. The pump then ends
  each watch of this computer, empties the list of the other computers and ends
  the stream of each follow panel. A new link gets the list and the follows
  again.

### The owner side

`list.rs` builds the list of this computer in a frame. `owner.rs` is the owner
side of the pump: the watch of each panel that another computer follows, and
the check of the typed bytes.

- **The list holds each terminal panel of the board.** A terminal panel is a
  panel with a terminal of its own, such as a shell, an agent, a command or an
  `ssh` panel. The list holds no follow panel, no card, no editor, no browser
  panel and no device panel. It holds at most 64 panels. The pump gives the
  link an empty list while `share_terminals` is `false`.
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
- **The list has two bounds.** A frame builds it at most one time in 250 ms.
  The pump gives the list to the link only when the list differs from the last
  one.
- **Only a panel of the list has a watch.** A `watchers` event for another
  panel changes nothing, whatever the server names. A watch ends when its panel
  closed, left the list or has no follower. A watched panel that closed leaves
  the list of the link at once (`LiveHandle::ended`).
- **A watched panel is drained in each pass of the pump.** Each drain calls
  `set_wanted(true)`. A frame gives the pump the source of a panel that started
  a new terminal, so that panel continues its share with the new outlet.
- **A screen is due in five cases.**
  - A new follower came.
  - A `watchers` event says `refresh: true`.
  - A take says `overflowed`.
  - 250 ms went by after a new follower came.
  - 250 ms went by after the link refused a screen or output.

  The second screen of a new follower repairs a first screen that started in
  the middle of an escape sequence.
- **No output goes out while a screen is due.** The screen holds that output.
  The pump also takes nothing from the outlet then, and a watch starts with a
  screen. Thus each take continues a screen that the same pump made.
- **The owner checks a typed event.** `Run::typed` of `owner.rs` writes the
  bytes into the program of the panel only when four conditions are true:
  1. `share_terminals` is `true`.
  2. `remote_typing` is `true`.
  3. The identifier of the event is in the list that the link has.
  4. A terminal panel of this computer has that identifier.

  It does not read the sender of the event to decide. The pump reads the two
  switches from its copy of what the window wants. It takes the events of a
  pass before that copy, so a switch that is off in the copy refuses each event
  of the pass. The name of the sender goes to the chip; see
  [the chip of a watched panel](#what-the-person-sees).

#### The screens of an owner

The pump decides why a screen goes out, which decides what the screen holds
and who takes it (`Why` in `owner.rs`).

| Why | `fresh` | `everyone` | What it holds | Who takes it |
|---|---|---|---|---|
| A new follower came | `true` | `false` | The lines above the viewport, then the rows | The followers that wait for a screen |
| A follower asked for a screen | `false` | `false` | The rows, in place | The followers that wait for a screen |
| The second screen of a join, a take that overflowed, a refused screen or output | `false` | `true` | The rows, in place | Each follower of the panel |

A fresh screen carries at most 1000 lines above the viewport, oldest first,
and at most 256 KiB of them (`HISTORY_LINES`, `HISTORY_BYTES`). A program on
the alternate screen has no history above its viewport, so its fresh screen
carries none. A screen that is not fresh keeps the history of each follower and
draws each row at its place. A new grid of the owner stops the outlet, so the
next take overflows and asks for the third kind of screen.

The pump keeps the bytes of one screen while the link takes it in parts
(`Owed`). It gives the link the grid first, then the bytes in parts of at most
48 KiB, and no output of the panel goes out between them. A part that the link
refuses waits for the next pass with half of the part. A screen above 8 MiB
(`KEPT_SCREEN_BYTES`) is not sent at all, so the followers keep the screen that
they have.

#### The grid of the program

The grid of the program of a panel follows the computer that typed in it last
(`follower.rs`, `owner.rs`, `ShareSource::hold_grid`).

1. **A follower asks.** On a pass where the feed gives typed bytes, or while
   the follower holds a grid and the body of its panel fits another one, it
   sends `resize` with the grid that its widget wrote into the feed
   (`FollowFeed::set_fit`, `bounded_screen_grid`). It asks for one grid at most
   each 500 ms (`ASK_GRID`). It holds a grid while the newest screen brought
   that grid; a screen with another grid ends the hold, so the next key press
   asks again.
2. **The server checks the follow.** `resize` reaches the owner only when the
   computer follows that panel now and the list of the owner says
   `typing: true` for it. The server passes two requests a second for one
   follower and one panel, and it drops a third.
3. **The owner checks the panel.** `Run::resized` holds the grid only when four
   conditions are true: `share_terminals` is `true`, `remote_typing` is `true`,
   the list that the link has names the panel, and a terminal panel of this
   computer has that identifier and is no follow panel.
4. **The program takes the grid.** `ShareSource::hold_grid` resizes the grid of
   the terminal, sends the size to the program, and stops the outlet. The next
   take then overflows, so each follower gets a screen with the new grid, with
   `fresh: false` and `everyone: true`.
5. **The panel of the owner holds the grid.** `Terminal::resize_with_policy`
   returns while the grid is held, so the body of the panel pushes no size. The
   widget draws `Terminal::shown_grid` from the top left of the body and clips
   what does not fit (`Terminal::shows_foreign_grid`). The pointer and the
   scrollbar read that grid.
6. **The person of the owner takes the grid back.** A key or a mouse report in
   the panel calls `Terminal::write_input`, which calls `release_grid`: the grid
   of the body goes to the terminal and to the program again, the pump stops
   the hold on its next pass, and the chip stops naming the follower. Bytes
   that another computer sends do not take the grid back.

### The follower side

`follower.rs` is the follower side of the pump: the other computers of the list
and the follow panels of this window.

- **The list of this window holds the other computers.** The pump removes the
  entry of this computer.
- **A press on a row opens one follow panel.**
  `HorizonApp::open_shared_terminal` makes the panel in the active workspace
  with `Board::create_follow_panel`, and the pump calls `LiveHandle::follow`. A
  second press on the same row goes to the open panel. The kind of the list
  selects the keyboard protocol, and a kind that is no terminal kind is a
  shell.
- **The events of a stream go to the feed.** A `screen`, an `output` and an
  `ended` event go to the feed of the panel that follows that panel of that
  computer.
- **Typed bytes go out with their frame.** The frame sends the pump a notice
  after the panels took the keys, so `take_typed` gives the bytes of that frame
  to `LiveHandle::typed` at once. Bytes that no frame brought go out in 25 ms
  at most.
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
- **A closed follow panel ends its follow.** The pump calls
  `LiveHandle::unfollow`.
- **`typing` follows the list.** A panel that is open takes the `typing` value
  of the newest list in both directions (`FollowFeed::set_typing`). It drops
  the bytes of the person while the owner turns typing off, and it keeps them
  again when the owner turns typing on. The chip of the panel says which of the
  two it is.
- **A follower gives the grid of its body to the program of the panel that the
  person types at.** A key or a pointer button of the person asks for that
  grid, and so does the body of the panel that changed while the follower kept
  its grid. [The grid of the program](#the-grid-of-the-program) gives the rule
  and the checks.

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
  follower, and `seen by <n> computers` for more than one. It says
  `typed by <name>` for 2 seconds after the bytes of that computer arrived, and
  `sized by <name> · <the rest of the chip>` while the program of the panel
  holds the grid of that computer.
- **The chip of a follow panel.** Its title bar says `on <name>`. It says
  `on <name> · view only` while the list says that typing is off.
- **A panel shows one chip.** The chip of a shared terminal takes the place of
  two other chips. They are the chip of the agent that opened the panel and the
  chip of its machine (`crates/horizon-ui/src/app/panels.rs`).
- **Each text of another computer is cut.** The window shows a name, a title
  and a reason without control characters, at 80 characters at most (`shown`).
  The hover text of the name of a computer is the exception: it is the device
  name as the list gave it.

## The limits that are known

- **A link that goes quiet ends.** Each side sends a `heartbeat` each 10
  seconds and ends a link that read nothing for 30 seconds. Thus a computer
  that loses its network leaves the list of the others in about half a minute.
  A transport can stay open under a program that went away; the heartbeat ends
  that link. [The sign of life](#the-sign-of-life) gives the bounds.
- **One device name in two programs.** Two Horizon programs with one device
  name each end the live link of the other, because the newer link wins. The
  program that loses says `another Horizon of this computer holds the live
  link` and asks again after 60 seconds, and after 5 minutes for each attempt
  after that. The program that wins keeps its link, so the lists of the other
  computers stop to flap.
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
- **The grid of a panel follows the computer that types.** While another
  computer types in a panel of this computer, the program of that panel has the
  grid of the body of the other window. The window of the owner draws that grid
  from the top left of its body and clips what does not fit, until its own
  person types there. [The grid of the program](#the-grid-of-the-program) gives
  the rule.
- **A grid above the bounds.** One pair of bounds holds the grid of a `screen`,
  of a panel of a list and of the terminal of a follower:
  `horizon_core::bounded_screen_grid` gives 2 to 1000 columns and 1 to 500
  rows. The bytes of a screen draw the grid of the owner, so a follower of a
  larger grid draws them on a smaller grid.
- **A fresh screen replaces the history, and the other screens keep it.** A
  fresh screen starts with a reset and the lines above the viewport of the
  owner, so a follower that joins reads what the program wrote before it came.
  A screen that is not fresh draws each row at its place and leaves the
  history of each follower as it is, and each follower of the panel takes such
  a screen.
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
- **A screen above 8 MiB is not sent.** The pump keeps the bytes of one screen
  while the link takes it in parts of 48 KiB, and a refused part halves. A
  screen above `KEPT_SCREEN_BYTES` is not sent at all, so the followers keep
  the screen that they have. A screen that does not fit the free room of the
  1 MiB of a follower at the server puts that follower behind until its link
  is under 256 KiB again.
- **One switch for all terminals.** `share_terminals` shares each terminal
  panel of the computer or none. No setting shares one panel or keeps one panel
  out, and the list also holds the agent panels and the `ssh` panels.
- **The owner sees who types, for 2 seconds.** The chip of a watched panel
  says `typed by <name>` for 2 seconds after the bytes of that computer
  arrived, and then `seen by` again. The panel itself shows no other mark.
- **The name field starts the link again at each change.** A change of the
  field **Name of this computer** applies before **Save**. Each new value
  starts the live link again, so the followers of this computer lose their
  streams for a moment at each change.
- **A short loss of the sync link.** The join reads the state of the sync link
  one time in each frame. It does not see a sync link that is away for less
  than one frame, and the live link does not start again then.
- **A window with no frame can be 250 ms late on Wayland.** An event of a
  terminal gets its pass at once, up to a bound of about sixty a second. The
  pass of a repaint request comes 250 ms after the request, as
  [What a window with no frame does](#what-a-window-with-no-frame-does) says.
  eframe sets that time. The second build of the list in 250 ms and each event
  above the bound wait for that pass.
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
| The grid of a screen and of a panel | 2 to 1000 columns, 1 to 500 rows | `bounded_screen_grid` |
| The lines above the viewport of a fresh screen | 1000 lines, and 256 KiB | `HISTORY_LINES`, `HISTORY_BYTES` |
| A screen that the pump keeps | 8 MiB | `KEPT_SCREEN_BYTES` |
| A part of a screen | 48 KiB | `MAX_LIVE_OUTPUT_BYTES` |
| The tick of the sign of life | 1 second | `Limits::tick` |
| A heartbeat of a live link | 10 seconds | `Limits::heartbeat` |
| The silence that ends a live link | 30 seconds | `Limits::silence` |
| The next attempt of a program that lost its link to another | 60 seconds, then 5 minutes | `REPLACED_RETRY`, `REPLACED_RETRY_LONG` |
| One grid from a follower | 2 in a second | `RESIZE_EVERY` |
| The chip of a computer that typed | 2 seconds | `TYPED_CHIP` |
| One text of a follower | 200 characters | `MAX_TEXT` |
| The name of a computer in the settings | 60 characters | `MAX_COMPUTER_NAME_CHARS` |
| The shortest time between two builds of the list | 250 ms | `LIST_EVERY` |
| The longest time without a build of the list | 2 seconds | `LIST_CHECK` |
| The passes of the pump while it has a stream | Each 25 ms | `PERIOD` |
| The second screen of a new follower, and the next screen after a refused frame | 250 ms | `NEXT_SCREEN` |
| The next request of a follower for a screen | 1.2 seconds | `ASK_AGAIN` |
| The next grid that a follower gives to a program | 500 ms | `ASK_GRID` |
| One text of another computer in the window | 80 characters | `SHOWN_CHARS` |
