---
procedure: repository-machine-recipe
feature: A machine for a repository
platforms: [linux, macos]
cost: none with the stand-in provider
destructive: no
secrets: none
owner: peters
---

# Machine recipe procedure

## 1. Purpose

This procedure makes sure that an agent reads the recipe of a repository, that
the rent card states the resources, the count and the price of all machines
together, that one press rents them, that each machine gets a steps card of six
steps, that a machine card says `Ready for <repository>` with the check of the
recipe as evidence, and that a step that failed keeps its card with the last
lines of the output and the buttons `Try again` and `Delete the machine`. It also
makes sure that the ready card of a machine holds one `Start <program> here`
button for each agent program of that machine, that a press starts the program in
the folder of the repository, that the row of the agent in the sidebar opens its
panel, and that a second press starts no second copy. It also
makes sure that a repository without the section `## Horizon machine` gets one
question on a card and no machine. It also makes sure that an agent continues
its task on a machine in words only: it stops on a change that is not committed,
writes the note `HANDOFF.md` on a branch, brings the branch with the bundle
steps, and draws one card that says what moved and what did not.

No step of this procedure reaches a provider: the program `hcloud` of the
fixture is the stand-in of `scripts/device-smoke/hcloud-standin.sh`, and the
machine that it reports does not answer SSH. The steps that install a tool,
bring a repository and trust its folder therefore fail at the SSH step, which is
the failure card of task 6.

## 2. Applicability

- Candidate: a frozen build of one exact commit, `<candidate-commit>`.
- Platforms: Linux and macOS. On macOS, use Cmd instead of Ctrl in each step.
- Watch the candidate live through a native VNC Device panel in your own Horizon
  workspace, as `AGENTS.md` requires.
- The repository of the test is a small repository of this run, `<example-repo>`.
  It holds an `AGENTS.md` with the section of task 3 and one test that passes.

## 3. Safety

No step of this procedure rents compute, deletes data, sends a secret or changes
access. The stand-in provider makes nothing: it writes three small files beside
itself and opens no socket. Task 10 removes them.

## 4. Equipment and preconditions

- A Linux computer with Xvfb, openbox, x11vnc, xdotool, ImageMagick, Python 3
  and dbus, as the [local device smoke fixture](../../../scripts/device-smoke/README.md)
  uses. On macOS, an isolated desktop that a native VNC Device panel can show.
- Your own Horizon, with an agent panel that can call `device_panel`.
- The candidate binary, `<candidate>`, and its repository, `<repo>`.
- An empty state folder `<state>` for the fixture, and an empty folder
  `<work-agent>` for the workspace of the agent and its `bin` folder.
- A frozen copy of the candidate in `<smoke_bin>`, made as the fixture README
  says.

## 5. Prepare the test repository

1. Make the repository and one commit in it.

   ```sh
   git init <example-repo> && cd <example-repo>
   printf 'fn main() {}\n' > main.rs
   git add main.rs && git -c user.name=test -c user.email=test@example.invalid commit -m "one file"
   ```

   Result: `git log --oneline` shows one commit.

2. Write the recipe of task 3 into `AGENTS.md`, and commit the file.

   Result: `rg -n '^## Horizon machine' AGENTS.md` answers line 1 and the eight
   entries follow it.

3. Start the fixture with the agent panel and the stand-in provider.

   ```sh
   cp <repo>/scripts/device-smoke/hcloud-standin.sh <work-agent>/bin/hcloud
   chmod +x <work-agent>/bin/hcloud
   PATH=<work-agent>/bin:$PATH python3 <repo>/scripts/device-smoke/serve.py \
     --horizon <smoke_bin>/horizon --native-view --state <state>
   ```

   Result: the native VNC Device panel of the fixture shows the Horizon board,
   and the command `hcloud server list --quiet` in a terminal of that desktop
   answers nothing and exits 0.

## 6. The recipe run

1. In the agent panel, write:
   `Use the horizon-machines skill. Rent one machine for <example-repo>; its AGENTS.md holds the recipe.`

   Result: the agent reads `AGENTS.md` and `references/repository-machine.md`,
   and it draws one rent card.

2. Read the rent card. It holds the rows `Resources`, `Type`, `Machines` and
   `Price`, one `text` line that says that one press rents the machines, and the
   buttons `Rent 1 machine` and `Cancel`.

   Result: the `Resources` row holds the pair of the recipe, the `Machines` row
   holds `1`, and the `Price` row holds one price for each hour, or
   `price not read`.

3. Press `Rent 1 machine`.

   Result: the run card changes to the steps card of that machine, with the six
   steps `Prepare the system`, `Install the tools`, `Bring the repository`,
   `Trust the repository folder`, `Warm up` and `Run the check`, and the first
   step is `now` or `done`. The step `Trust the repository folder` follows
   `Bring the repository`.

4. Wait until the steps card reaches the SSH step.

   Result: the card shows the step that failed, one `text` line with the last
   lines of the output of that step, and the buttons `Try again` and
   `Delete the machine`. The badge is `Stopped` with tone `warn`, and no second
   machine was rented.

5. Press `Try again`.

   Result: the card repeats the failed step, and it fails again with the same
   stand-in machine. The card keeps both buttons.

## 7. A repository without a section

1. Remove the section from `AGENTS.md` of `<example-repo>` and commit the change.
2. In the agent panel, write:
   `Use the horizon-machines skill. Rent one machine for <example-repo>.`

   Result: the agent draws one card with the heading
   `No machine recipe in this repository`, the rows that it read with their
   source, the buttons `Write the section` and `Not now`, and one `caption` that
   names `AGENTS.md`. No machine is rented.

3. Press `Write the section`.

   Result: the agent appends the section to `AGENTS.md` in the worktree of the
   task, draws it on the card, and rents nothing until the person presses the
   rent of a run.

## 8. The machine card of the ready state

This task needs a machine that answers SSH, so it does not run with the stand-in
provider. Run it only when the person agrees to the cost of one small machine at
the provider of the person, and follow the caution of
[set-up-wizard](set-up-wizard.md).

1. Rent one machine for `<example-repo>` at the provider of the person, with the
   recipe of task 3.
2. Wait for the check of the recipe.

   Result: the heading of the machine card is `Ready for example-repo`, the badge
   is `Ready` with tone `ok`, one `text` line holds the check command and its
   result, and the card holds `Connect` and `Delete the machine`.
3. Read the row of the machine in the Machines section of the sidebar.

   Result: the second line of the row reads `<repository> · <provider>`, such as
   `example-org/example-repo · Hetzner, hel1`. Make the window narrow until the
   line does not fit, and read it again: the end of the line is cut with an
   ellipsis, and the repository keeps its place at the start of the line.
4. Ask the agent to run `machines`.

   Result: the record of the machine holds `repository` with the same text as
   the row.

5. Read the buttons of the card, directly after `Connect`.

   Result: one `Start <program> here` button stands there for each agent program
   that the machine has, for example `Start Claude here` and `Start Codex here`.
   A machine with no agent program holds no such button, and the card says
   `No agent program on this machine.`

> **CAUTION:** STEP 6 STARTS A PROGRAM ON A RENTED MACHINE. The machine costs
> money from the rent until the delete of step 10, and the program runs in the
> folder of the repository. No login of this computer reaches the machine.

6. Press `Start Claude here`.

   Result: the card holds one line that names `Claude`, the folder of the
   repository and the row of the sidebar. The Machines section of the sidebar
   holds a row of the agent under the row of the machine, named with the last part
   of that folder, for example `example-repo`, and with the mark of Claude.

7. Read the session on the machine.

   ```sh
   ssh <options of the machine key> <user>@<address> 'tmux ls'
   ```

   Result: one session whose name starts with `hz-agent-claude-`.

8. Press the row of the agent in the sidebar.

   Result: a panel of that machine opens and shows the screen of `claude` in the
   folder of the repository. The person signs in there, and no login file, token,
   keyring, cookie or `~/.ssh` of this computer is on the machine.

9. Press `Start Claude here` again, and read `tmux ls` on the machine again.

   Result: the second press starts no second copy: the card still names one line
   of `Claude`, and `tmux ls` names the same one session.

10. Press `Delete the machine`, and finish the delete as the skill says.

   Result: the card of the machine goes, and the machine list of the sidebar
   holds no record of it.

## 9. Continue a task on a machine

This task needs the machine of task 8, so it does not run with the stand-in
provider.

1. Read the page `references/handoff-machine.md` of the skill `horizon-machines`,
   and ask the agent in a local panel of `<example-repo>` to continue its task on
   the machine of task 8. Change one file of `<example-repo>` first, and do not
   commit it.

   Result: the agent stops and names the change that is not committed. It asks
   what to do with that change, and it changes no file.

2. Say `commit it on the branch work/example`.

> **CAUTION:** STEP 3 COPIES A BRANCH TO A RENTED MACHINE. The machine costs
> money from the rent of task 8 until the delete of step 10, and the branch of
> `<example-repo>` lands in the folder of the machine. No login of this computer
> reaches the machine.

3. Read the card that the agent draws.

   Result: one card. Its heading is `The machine holds work/example` with the
   badge `Moved`, one `text` line names the commit and says that the folder of
   this computer is unchanged, one `kv` row `Not moved` names what did not move,
   and the card holds `Start Claude here` and `Close`.

   ```sh
   ssh <options of the machine key> <user>@<address> \
       'git -C <folder> branch --show-current; head -20 <folder>/HANDOFF.md'
   ```

   Result: the branch is `work/example`, and `HANDOFF.md` names the task, what is
   done, what is next, the first command to run and the check of the repository.

4. Read the rows of the machine card and of the sidebar.

   Result: the second line of the row of the machine in the sidebar reads the
   repository, then the folder of the record, then the provider words, and the
   card holds the row `Folder` with the same folder in `mono`.

5. Press `Start Claude here`, and ask the new agent on the machine to read
   `HANDOFF.md` and to continue the task.

   Result: the program runs in the folder of the record, and the conversation of
   the new agent is new: it read the note and no message of the conversation of
   this computer.

6. Press `Close`.

   Result: the card goes from the workspace, and the panel of this computer keeps
   running.

## 10. Clean up

1. Stop the fixture with the window manager of the fixture desktop, and stop the
   agent panel.
2. Remove the record of the stand-in provider.

   ```sh
   rm -f <work-agent>/bin/hcloud-standin.record <work-agent>/bin/hcloud-standin.key \
         <work-agent>/bin/hcloud-standin.firewall
   ```

   Result: `ls <work-agent>/bin` names the stand-in program and no record.

3. Remove `<state>`, `<work-agent>` and `<example-repo>`.

   Result: the three paths do not exist.
