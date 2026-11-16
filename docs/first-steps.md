# First steps

This guide tells you how to install Horizon and use it for the first time. It
describes Horizon as it is on `main`. Some functions work on Linux only. Read
[the platform support](platform-support.md) before you start.

The latest release is v0.2.7 from 2 August 2026. The `main` branch is more
than 800 commits newer. The release does not have browser panels or Device
panels. Some steps in this guide can also be different in the release. To do
all procedures in this guide, build from source.

## What Horizon does

Horizon shows all of your terminals, coding agents, browsers and remote
desktops on one board. The board is a large surface that you can pan and zoom.
A workspace is a group of panels with one working directory. Horizon saves the
board when you close it.

These functions are available:

| Function | What it does | Platforms | In v0.2.7 |
|---|---|---|---|
| Shell panels | A terminal in the workspace directory. | Linux and macOS | Yes |
| Agent panels | Claude Code, Codex, Grok and other coding agents in a panel. | Linux and macOS | Yes |
| Browser panels | Chromium, Firefox or Safari on the board. You and an agent use the same page. | Linux and macOS. Safari on macOS only | No |
| Device panels | A VNC view of a desktop on this computer or on an SSH host. | Linux and macOS | No |
| Agent input to an isolated desktop | An agent clicks and types on an Xvfb desktop for an application test. | Linux with X11 | No |
| Remote Hosts | SSH panels for your hosts and Tailscale devices. | Linux and macOS | Yes |

## Select an install route

| Route | Use it when |
|---|---|
| Release binary | You want to start quickly, and v0.2.7 has the functions that you need. |
| Homebrew (macOS, Linux x64) | You use Homebrew. |
| Source build | You want the functions in this guide. |

Release builds use the default features. All release routes install v0.2.7.

## Install a release binary on Linux or macOS

1. Open the [latest release](https://github.com/peters/horizon/releases/latest).
2. Download the raw binary for your platform:

   | Platform | File |
   |---|---|
   | Linux x64 | `horizon-linux-x64.tar.gz` |
   | macOS arm64 | `horizon-osx-arm64.tar.gz` |
   | macOS x64 | `horizon-osx-x64.tar.gz` |

3. Download `SHA256SUMS.txt` from the same release to the same directory.

> **CAUTION:** DO NOT START A FILE WITH AN INCORRECT CHECKSUM. The checksum
> finds an incomplete or damaged download. It does not prove who made the
> file, because the release and `SHA256SUMS.txt` are not signed.

4. If you use Linux, examine the checksum:

   ```bash
   sha256sum -c --ignore-missing SHA256SUMS.txt
   ```

5. If you use macOS, examine the checksum:

   ```bash
   shasum -a 256 -c --ignore-missing SHA256SUMS.txt
   ```

   Result: The command shows `OK` for the file that you downloaded.

6. If the command does not show `OK`, delete the file.
7. Extract the file. Replace `<file>` with its name, for example
   `horizon-osx-arm64.tar.gz`:

   ```bash
   tar -xzf <file>
   ```

8. Make the binary executable:

   ```bash
   chmod +x horizon
   ```

9. Start Horizon:

   ```bash
   ./horizon
   ```

   Result: Horizon opens an empty board.

10. If macOS blocks the start, remove the quarantine attribute:

    ```bash
    xattr -d com.apple.quarantine horizon
    ```

11. If you did step 10, start Horizon again:

    ```bash
    ./horizon
    ```

    Result: Horizon opens an empty board.

A raw binary does not update itself. Download a new release to update it.

On macOS the release file is a bare binary, so the Dock shows a generic icon.
The script `packaging/macos/make_app_bundle.sh` makes `Horizon.app` from a
release build, with the icon, the name and the `Info.plist` that gives the
microphone reason of speech input. Its `--install` copies the bundle to the
applications folder.

## Install with a package manager

On macOS or Linux x64, install Horizon with Homebrew:

```bash
brew install peters/horizon/horizon
```

Use the same package manager to update Horizon. Horizon has no in-app update prompt.

## Update an older installation

Horizon has no in-app updater. Use your package manager to update a package installation.
For a release binary, close Horizon and replace the executable with the new release binary.

Older Surge installations do not get new update packages.
Close the old application and install a release binary in a separate directory, or use a package manager.
Keep the files in `~/.horizon` or the corresponding user configuration directory.
The new executable uses the same configuration and session formats.
Do not put the new executable inside the old managed application directory.

Examine the old application shortcuts before you start Horizon again.

CAUTION: Remove only the shortcut. Keep the configuration and session files.

Change each shortcut to use the new installation, or remove the shortcut.
On macOS, examine the `Horizon.app` links in `~/Applications` and `~/Desktop`.
On Linux, examine desktop shortcuts and pinned launchers.
Start Horizon from the new installation to make sure you use the new executable.

## Build from source

1. Install the tools for your platform:
   - All platforms: Git, Git LFS and Rust 1.95 or later from [rustup](https://rustup.rs).
   - Linux: the system headers in [AGENTS.md](../AGENTS.md#prerequisites).
   - macOS: the Xcode Command Line Tools.
   - x86_64: NASM 2.15 or later on `PATH`.
2. If you use macOS, install the Xcode Command Line Tools:

   ```bash
   xcode-select --install
   ```

3. Clone the repository:

   ```bash
   git clone https://github.com/peters/horizon.git
   ```

4. Go to the repository directory:

   ```bash
   cd horizon
   ```

5. Set up Git LFS:

   ```bash
   git lfs install
   ```

6. Get the Git LFS files:

   ```bash
   git lfs pull
   ```

7. Build and start Horizon:

   ```bash
   cargo run --release
   ```

   Result: Horizon opens an empty board.

If the build stops with a missing font, do step 6 again. On Linux, a
`pkg-config` or linker error usually tells you that a `-dev` package is missing.

## The first start page and Set up

The first start shows one page with four starts. Each tile is a button: click it,
or push Tab until it has the focus ring and then push Enter or Space.

| Tile | What it does |
|---|---|
| `Run agents side by side` | Makes a workspace and opens a panel of the first agent program that your computer has, in the order of your settings. A computer with no agent program gets a terminal, and one line in the sidebar says so. |
| `Work on a bigger machine` | Starts Set up with the goal `rent`. |
| `Use Horizon on all your computers` | Starts Set up with the goal `server`. |
| `A browser that stays signed in` | Starts Set up with the goal `browser`. |

Under the tiles is the row of shortcuts of this computer: New terminal,
Quick Nav and Machines you have. On macOS, the primary key is Cmd.

### The page at any window size

The page follows the size of the window. On a wide screen the block of the page
keeps a width that is easy to read, and it stands in the middle of the canvas.

A window that is too short for the whole page first draws the small picture of a
tile smaller, and then removes the picture. A page that is still taller than the
canvas moves:

- Use the wheel, the trackpad, or Page Down and Page Up to move the page.
- Push Tab to move to a tile below the canvas. The page moves to that tile.

### Pick up where you left off

A row of the section is a conversation of Claude Code or Codex that an agent
program keeps on this computer. The section shows the newest conversation of
each folder, six folders at most. The control `Show all`, and the row
**Recent agent sessions** in the Menu of the titlebar, open the full list: 24
conversations at most, of the last 14 days, grouped by folder.

Each row shows the title of the conversation, its program, its folder, the time
of its last activity and the state of the conversation. The home folder is `~`.
A title that is longer than the row is cut at the end of the row, with an
ellipsis; a wide window shows the whole title.

1. Click `Open` on a row, or push Tab until the row has the focus ring and then
   push Enter.

   Result: Horizon shows the workspace of that folder, or makes it when the board
   has none, and opens a panel of that program in it. The panel continues that
   conversation, and the keyboard is in the panel.

2. Click `Open all` on a folder of the list.

   Result: One panel opens for each conversation of that folder, four at most.
   The other conversations stay in the list.

A row says `folder is gone` when its folder is not there: the row cannot open.
A row says `program is not installed` when Horizon cannot find the program of the
conversation: install the program, then push `Show all` again.

A row says `open in another program` when a program has that conversation open
now. Two programs in one conversation is a known hazard, so the row opens only
after a second press on `Open anyway`.

Horizon reads the conversations of Claude Code from `~/.claude/projects` and the
conversations of Codex from `~/.codex`. It writes nothing there. Another agent
program, such as Gemini or KiloCode, keeps no conversation that Horizon can
continue, so it has no row. A conversation that an open panel of Horizon already
has is not a row.

Set up opens a panel of your agent and gives it one sentence: `Use the
horizon-setup skill. The person pressed Set up. Goal: <goal>.` The agent then
does the work and shows each step as a card.

You can start Set up in three places:

1. The control **Set up** in the header of the **Machines** section of the
   sidebar. It has no goal, so the agent asks which one you want.
2. The row **Set up** in the Menu of the titlebar, above Settings. It has no
   goal either.
3. One of the three Set up tiles of the first start page. Each tile gives its
   own goal.

Set up starts the agent preset that your settings name first among the agent
programs that this computer has. To change the choice, open **Settings**,
**Presets**, and change the order of the presets or the command of one preset.
A preset of Gemini or KiloCode is never chosen, because those programs take no
first message on the command line.

A second press of Set up shows the panel that the first press opened. It starts
no second panel. When you close that panel, the next press opens a new one.

When no agent can start, Horizon draws its own card, **Set up needs an agent**.
The card names each agent program that Horizon looked for, says whether it
found one, and says whether the agent integrations of this run are on. Its
buttons are **Look again** and **Open Settings, Presets**.

If you start Horizon with `HORIZON_AGENT_INTEGRATIONS=off`, the agent has no
Horizon skills, so Set up does not start. The card then says: `Agent
integrations are off, so the agent has no Horizon skills. Start Horizon without
HORIZON_AGENT_INTEGRATIONS=off.`

### What each choice of Set up does, and what it costs

Set up draws one card beside the panel of your agent and changes that card at
each step. You press a row or a button on the card, or you write what you want in
the panel of the agent.

| Your choice | What Set up does | What it costs |
|---|---|---|
| `Work on a bigger machine` | Rents a machine in your own account at your provider, opens a terminal on it, and adds it to Machines in the sidebar. | The price of the provider, from the press on `Rent this machine` until `Delete`. A stopped machine still costs its disk. |
| `Use Horizon on all your computers` | Puts the Horizon server on this computer or on a machine, and links this computer to it. | Nothing on this computer. On a rented machine, the price of that machine. |
| `A browser that stays signed in` | Installs one Chromium on a machine that stays on, with one profile, and makes it the browser of your new browser panels. | Nothing on this computer. On a rented machine, the price of that machine. |
| `Join a server that you already have` | Links this computer to a Horizon server that runs, so your machine list, your machine keys and your settings follow it. | Nothing. |

Nothing is rented and no setting changes before you press the button that says
so. A price on a card is the value that the provider answered when the card was
drawn, and a row says `price not read` when the provider answered no price.

Two changes need your press on a card that Horizon draws itself: the server
entry and the browser host. An agent cannot press those cards. When you press
`Link`, Horizon writes the entry and starts the link at once, so you do not start
Horizon again. A browser host entry serves each browser panel that you open after
the press, so that change needs no start either. A browser panel that is open
keeps its browser until you close it.

When a step fails, the card says what failed, what was undone and what is left,
with `Try again` and `Stop`. A secret, such as a token or a private key, never
appears on a card or in the panel of the agent.

## Start Horizon in a folder

1. Open a terminal in the repository or the folder that you want to work in.
2. Type `horizon .`

   Result: Horizon starts with the board of the last run, and then shows the
   workspace of that folder with the keyboard in it. When the board has no
   workspace of that folder, Horizon makes one with the name of the folder and
   opens one panel in it: the first agent preset of your settings whose program
   this computer has, or a terminal in that folder when no agent program is there.

3. Type `horizon ~/github/my-repo` to start in another folder.
4. Type `horizon <path> --ephemeral` to start a board that keeps nothing.

A path that is not a folder, or a path that is not there, ends the start with one
sentence and status 2, and no window opens.

When a Horizon of this home already runs, `horizon <path>` shows the folder in
that window, and no second window opens. The running window brings itself to the
front and shows the workspace of the folder, or makes it, as a start on an empty
window does. The start writes one line on the standard output and ends with
status 0:

- `Horizon shows the folder <path>.` for a workspace that the window already had.
- `Horizon made a workspace for the folder <path>.` for a workspace that it made.
- `Horizon runs for this home, but it did not answer, so the folder <path> was not
  opened there.` when the window answers nothing in five seconds. The window may be
  minimized, busy or gone. Start again, or open the folder in the window.
A window that runs with `--ephemeral` keeps no session of this home. A folder
start does not see such a window, so it starts its own window, and the ephemeral
window takes no folder.

## Use the board

On macOS, use Cmd instead of Ctrl in each step of this procedure. A shell
panel uses the shell in the `SHELL` variable. If `SHELL` is not set, Horizon
starts `/bin/zsh` on macOS and `/bin/bash` on Linux. See
[the platform support](platform-support.md).

1. Hold Ctrl on Linux, or Cmd on macOS, and double-click an empty area of the
   board.

   Result: A list of presets opens.

2. Select **Shell**.
3. Select a working directory.

   Result: Horizon makes a workspace with one shell panel in that directory.

4. Push Ctrl+Shift+N.

   Result: A second panel of the first preset opens.

5. Push Ctrl+Shift+K.

   Result: The command palette opens.

6. Type a workspace name, a panel title or `>`.

   Result: The command palette shows the matches.

7. On the workspace header, click **Rows**, **Cols** or **Grid**.

   Result: The panels move into that layout.

8. Close Horizon.
9. Start Horizon again.

   Result: The board, the layout and the terminal history are the same.

The README lists all keyboard and mouse shortcuts. A shortcut with Ctrl uses
Cmd on macOS. A panel becomes fullscreen with F11 on Linux and with
Cmd+Shift+Enter on macOS, because F11 alone shows the desktop on macOS.

## Open an agent panel

1. Install the CLI of the agent, for example Claude Code or Codex.
2. Hold Ctrl on Linux, or Cmd on macOS, and double-click an empty area of the
   board.
3. Select the preset of the agent.

   Result: The agent starts in a new panel.

A thin line runs from an agent panel to each panel that its agent controls: a
card that it drew, a page that it drives, and a terminal or a preset panel that
it opened for you. The lines of the agent that you point at or that holds the
keyboard are clear, and the lines of the other agents are faint. You can turn
the lines off in Settings, General, Canvas.

## Read the state of an agent panel

The sidebar says **Needs you**, **Done**, **Working** or **Background** for an
agent panel. Three agent programs tell Horizon that state themselves, on Linux
and on macOS, with the default command or with a command of its own:

- **Claude Code** reports its turns through the plugin folder that Horizon gives
  it at the launch. **Continue work after restart** works on both systems for
  such a panel: a clean suspension continues after the restart, and every other
  case asks first.
- **pi** reports its turns through one extension file that Horizon gives it at
  the launch. The file is Horizon's own, it goes beside the extensions of the
  person instead of replacing them, and it sends no text.
- **Codex** reports its turns through the program status protocol, the open
  sequence `ESC ] 7501 ; state=...` that Codex writes with no setting, no
  argument and no file. Horizon reads it in the reader of the panel's terminal,
  beside the title of that terminal: the report answers first, and the title
  answers when no report arrives, for example inside a multiplexer that drops the
  sequence. Codex writes a spinner and the name of the folder into the title
  while it works, and a line with `Action Required` while an approval waits.

The name of such a panel is the folder, and the word of the row is the state: the
spinner of a working turn and the two frames of an approval line change the title
about ten times a second, and the name of the row, of the titlebar and of the
minimap stays one name. A name that you gave a panel stays in front of it, and a
title that leaves no name keeps the name that the panel had before.

Every other agent panel takes the state from the text on its screen, and it
keeps that signal until its agent reports its first event. A panel in which you
start an agent by hand in a shell keeps the screen rule too.

A program starts before it is ready for you, and Horizon says nothing about its
panel while it starts. Codex writes frames of its spinner, and it can report that
it works, while it loads: no turn of yours runs in that time, so the row shows no
state and the workspace line counts no finished turn. The first plain title of
the program, or its first idle report, is the moment it is ready. A question that
Codex asks before it is ready, such as a trust question, still says **Needs
you**, because you must answer it; after your answer the row returns to no state
until your first turn.

The three reports have limits that their programs set:

- A Claude turn can name the work that continues in the background, so
  **Background** comes from a Claude panel. pi and Codex report no such count.
- pi asks you for no permission. Its **Needs you** means that a dialog of an
  extension waits for you, or that the message ended with a question in its
  text, which the screen still guesses.
- A turn of pi that fails ends as a finished turn, and so does a turn that you
  interrupt with Escape.
- A Codex turn that fails reports the error state of the protocol, so its row
  says **Needs you**. A turn that you interrupt reports the idle state, and its
  row reaches **Done** as a finished turn does: the record keeps a finished turn
  and an interrupted turn apart, and the row says the same word for both.
- An approval that you deny ends its turn as an interruption too, and its row
  leaves **Needs you**: the request that you refused closes with the turn. A turn
  that you deny is not a finished turn, so Horizon offers no continuation for it.
- A Codex panel whose title claims a state reports only while the title takes one
  of the two forms: a person who changes the title setting of Codex, and another
  version of Codex with another title, keep the rule of the screen. Under `tmux`
  the report of the protocol does not arrive, so such a panel needs that `tmux`
  to set `set-titles on` and `set-titles-string '#{pane_title}'`.
- A hostile program in any agent panel can claim a state for its own panel. See
  [the boundary of the work ledger](review/trust-boundaries/agent-work-ledger.md).

## See the agents of a machine

Horizon asks each machine of your machine list which agents run there in `tmux`,
and it shows them under the row of that machine in the sidebar, with no panel
open. This works while you close every panel, and while you close the lid of the
laptop.

```
Machines
● build-1
  Hetzner, hel1
  └ CX shop        Needs you
  └ CC notes       Working ●
```

1. Start one or more agents in `tmux` sessions on the machine. You can start them
   in a terminal of Horizon or by hand over `ssh`.
2. Read the machine section of the sidebar.

   Result: one row stands under the machine for each agent, with the two letters
   of its kind, the name of the last part of the folder of its pane, and the same
   word and dot that a local agent panel shows.

A press on the row of an agent goes to the terminal that already shows its
session, or opens one in the workspace that carries the name of the machine. The
press is your look at the agent, so a finished turn stops saying **Done**.

Press the chevron in front of the name of a machine to fold its agents. A folded
machine says how many of its agents wait for you, or how many work. Close the lid
of the laptop, and the row of the machine says `link lost just now` until the
link comes back; the place of the agents then says how many it saw.

When an agent of a machine changes to **Needs you** or to **Done** while no
Horizon window has the focus, Horizon sends one notification that names the agent
and its machine, as it does for a local panel.

The check box **Show the agents of each machine** in Settings, General,
**Machines** turns this off. The shortcut for the next agent that waits for you
also reaches the agents of your machines, after the panels of this window.

## Open a browser panel

Browser panels need a source build from `main`. The **Browser** preset uses a
Chromium browser. A usual macOS installation has only Safari.

1. Install Google Chrome, Chromium or Microsoft Edge.
2. Hold Ctrl on Linux, or Cmd on macOS, and double-click an empty area of the
   board.
3. Select **Browser**.

   Result: A browser panel opens.

Horizon gives the `horizon-browser` MCP tools automatically to Claude Code,
Codex and Grok panels.

## Open a Device panel

Device panels need a source build from `main`. A Device panel needs a VNC target in the configuration. See
[Watch an app over VNC](../README.md#watch-an-app-over-vnc).

## Work from a place far away

A link with a round trip of 200 to 350 ms, packet loss or a changing address
changes what Horizon can do without your help. Read
[remote connections](remote-connections.md): the `~/.ssh/config` lines that share
one connection between panels, what each line gives, the limits of a shared
connection, and what Horizon does by itself after a cut.

## Get help

- [Platform support](platform-support.md)
- [Remote connections](remote-connections.md)
- [README](../README.md)
