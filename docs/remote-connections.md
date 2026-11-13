# Remote connections

Horizon starts one `ssh` child for each function that runs on another computer.
Each child is one TCP connection and one SSH session of its own.

| Function | The child |
|---|---|
| A terminal of a machine of the machine list | `ssh` with the key of the machine record, and the session command |
| A plain SSH panel of the Remote Hosts overlay | `ssh` with the command of that host |
| An upload over SSH | one `ssh` for the directory of the destination, then one `ssh` for each file |
| A Device panel over SSH | `ssh -W <endpoint>` |
| A browser panel of a browser host | one `ssh -N -L` browser tunnel, and short calls for the endpoint and the browser |
| The link to a Horizon server | `ssh <target> <command> --server-link`, or one TCP socket to the listener |

## What Horizon sets, and what your configuration decides

Horizon gives these options on the command line of each child:

| Child | Options |
|---|---|
| A terminal of a machine | `ConnectTimeout=10`, `ServerAliveInterval=5`, `ServerAliveCountMax=3` |
| A plain SSH panel, an upload, a `ssh -W` forward | `ConnectTimeout=10`, `ServerAliveInterval=15`, `ServerAliveCountMax=1` |
| The link to a server, and the calls of a browser host | `ConnectTimeout=5`, `ServerAliveInterval=5`, `ServerAliveCountMax=2`, `BatchMode=yes` |

`ssh` reads the first value of an option that it meets, and a value of the
command line comes before a value of a configuration file. A value in your file
therefore does not replace one of these options.

Horizon passes no `ControlMaster`, `ControlPath`, `ControlPersist` and no `-S`.
Multiplexing stays your decision, and the next section shows how to set it.

## A slow or unstable connection

At a round trip of 200 to 350 ms one handshake costs one to three seconds. Every
child of the table above makes its own handshake, so five panels cost five
handshakes, and a cut costs them again.

1. Add these lines to your `~/.ssh/config`:

   ```
   Host *
       ControlMaster auto
       ControlPath ~/.ssh/horizon-%C
       ControlPersist 10m
       ServerAliveInterval 15
       ServerAliveCountMax 3
   ```

   | Line | What it gives to a panel of Horizon |
   |---|---|
   | `ControlMaster auto` | The first child of a target opens one connection, the master. Each later child of the same target uses that connection and skips the handshake. |
   | `ControlPath ~/.ssh/horizon-%C` | The socket of the master. `%C` is a hash of the user, the host, the port and the jump host, so two targets never share one socket. |
   | `ControlPersist 10m` | The master stays for ten minutes after its last child. A panel that you close and open again uses it again. |
   | `ServerAliveInterval 15` | A keepalive for your own sessions. It gives nothing to a child of Horizon, because Horizon gives its own value first. |
   | `ServerAliveCountMax 3` | The number of keepalives without an answer that end a session of yours. It gives nothing to a child of Horizon, for the same reason. |

2. Make a new panel of a machine that you opened before, and examine the socket
   of the master:

   ```bash
   ssh -O check <target>
   ```

   Result: The command names the process of the master and says `Master running`.

3. Read the limits of the master:

   - One master is one connection. A master that hangs holds every child of that
     target. Horizon ends a child at its own bound, ten seconds for a connect and
     fifteen to thirty seconds for a link that went quiet, so the panels return,
     but they return together.
   - The first child opens the master, and its login, its key and its host key
     decide for every later child of that target. A machine of the machine list
     has its own key and its own `known_hosts` file. If such a machine must not
     share a login with your own sessions, put `ControlMaster` in the `Host`
     block of your own hosts and not in `Host *`.
   - `ssh -O exit <target>` ends the master and every child of it. Use it when a
     panel waits for a master that does not answer. A socket that a stopped
     master left behind is a file under `~/.ssh/`, and the next child of that
     target makes a new master.

4. If you use a Tailscale address or a name that moves, set the name of the
   server and not its address. A name is resolved again on each attempt, so a
   computer that changes its address is reached without an edit. An address keeps
   the same value, and the sidebar then says `Server away` with
   `The server did not answer at its address.` in its hover.

## Measure where the time of a press goes

Horizon can write one line for each key press of a terminal and for each input
action of a browser panel. Set `HORIZON_TERMINAL_LATENCY=1` or
`HORIZON_BROWSER_LATENCY=1` before you start Horizon. See
[measure the latency](latency-trace.md) for the fields of the lines and for the
way to compare two runs.

## What Horizon does by itself, and what only you can do

| Situation | Horizon | You |
|---|---|---|
| A cut during a terminal of a machine | The panel reconnects by itself after 3, 6, 12, 24 and then 30 seconds, and attaches the same tmux session. | Put `tmux new-session -A -s <name>` in the session command of the machine record. The session then holds the work across every cut. |
| A slow connect of a terminal of a machine | The badge reads `Connecting...`. A letter that you type goes into the attempt and reaches the shell when the link is up. Only `Esc` and `Enter` end the attempt. | Wait, or press `Esc` to start a new attempt. |
| A cut during a plain SSH panel | The panel ends as `Disconnected`, the body says that the text typed there is not sent, and `Reconnect` runs the same command again. | Press `Reconnect`. Horizon never runs the command by itself, because a command that is not a tmux session would start its work from the start. |
| A stall during an upload | `Cancel` ends the upload in about a tenth of a second, also when the write of the file waits for a far side that stopped reading. The window shows `Upload cancelled` and the number of files that were whole. | Press `Cancel`. There is no resume: send the file again after the link returns. |
| A cut during a Device panel of an SSH desktop | The panel reads `Disconnected`. | Press `Reconnect`. This build does not return by itself. |
| A cut during a browser panel of a browser host | The panel reads `Browser stopped`, and `Retry` opens a new page on the browser of that host. Text that you did not send is gone. | Press `Retry`. |
| The link to a server is away | The line reads `Server away`, with `· N pending` when changes wait. The changes are queued and sent by itself when the link returns. | Nothing. The link tries again after 3, 6, 12, 24 and then 30 seconds. |
| The link to a server did not open | The hover of the line and the Server tab name the reason of the last attempt in one sentence. | Read the reason, and edit the `Address` or the target in the settings when the address moved. |

## The link to a server

The settings offer two ways to reach a server:

- Over `ssh`, with a target and the command of the server. This way works
  through a jump host and through the ssh configuration that you already have.
- Over the TCP listener of the server, with its `host:port` and the token of a
  join. This way has no SSH handshake, so it reconnects in one round trip, and it
  is the faster way at a high round trip. It needs a port that the server
  exposes.

The queue of a computer keeps every change that the server did not decide, in
`machines/server-queue.json` of that computer. The queue has no cap: a change
that a person made is never refused or dropped because a server is away. A
queue of ten thousand intents is two and a half megabytes, a start reads it in a
few milliseconds, and it leaves in pushes of at most 256 intents.

## The bounds of one link

| Bound | Value |
|---|---|
| Connect of a child | 10 seconds, 5 seconds for the calls of a browser host |
| Keepalive that ends a quiet child | 5 seconds × 3 tries for a machine, 15 × 1 for the other children, 5 × 2 for a browser host |
| Answer of a push or a pull | 20 seconds |
| Answer of the idle wait | 21 seconds: 15 seconds of the server and a margin of 6 |
| Answer of the idle wait of an older server | 65 seconds: 50 seconds of that server and a margin of 15 |
| Next attempt of a lost link | 3, 6, 12, 24 and then 30 seconds; the delays start again after a link that lasted 10 seconds |
| Cancel of an upload | about a tenth of a second |
| Traffic of an idle computer | 348 bytes a minute, two small frames every 15 seconds |

The constants live in `crates/horizon-core/src/ssh/mod.rs`,
`crates/horizon-sync/src/supervisor.rs` and `crates/horizon-sync/src/server.rs`.
The design of the link is in
[the server link](architecture/horizon-server.md).
