---
procedure: worktree-card
feature: one git worktree for each task, shown on a card, with the panel of a preset
platforms: [linux, macos]
cost: none
destructive: yes
secrets: none
owner: peters
---

# Worktree card test procedure

## 1. Purpose

This procedure tests the bundled `horizon-worktree` skill and the `open_panel`
operation of `block_panel`. It proves that an agent starts a task in its own git
worktree, opens the panel of a preset in that folder, draws one card for the
repository, shows the changes of a worktree, merges a branch after a second
press, and removes a worktree after a second press.

Section 7 proves the same behavior with the scripted card agent of the fixture,
so a person watches the operation `open_panel` and the card with real git and no
model and no login.

## 2. Applicability

- Candidate: a current `horizon` build that installs the bundled skills.
- Platforms: Linux and macOS, with the isolated desktop of the device smoke
  fixture and a native VNC Device panel.
- This procedure does not test the panel of an agent preset with a live model.
  It uses a shell preset as the preset that a task opens.
- The tasks of section 6 need an agent panel with a model. Section 7 needs no
  model: the fixture starts the scripted card agent.
- This procedure does not test a push, a remote branch, a repository with more
  than 32 worktrees, or a paid resource.

## 3. Safety

> **CAUTION: RUN EVERY GIT COMMAND IN THE THROWAWAY REPOSITORY OF THE FIXTURE
> STATE FOLDER.** A `git worktree remove` and a `git branch -d` delete files.
> Run this procedure only on the repository that step 5.1 makes, and examine
> the folder name before each of those commands.

> **CAUTION: DO NOT RUN THIS PROCEDURE IN A REPOSITORY OF THE PERSON.** The
> commands of the agent change branches and folders. Use the throwaway
> repository only, and stop the procedure if the agent names another root.

> **CAUTION: THE REMOVE TASK DELETES FILES AND A BRANCH, AND THIS CANNOT BE
> UNDONE.** Press `Remove <folder>` only on the card of the throwaway
> repository, after you read the list of files and commits that it shows.

## 4. Equipment and preconditions

- A frozen candidate `horizon` binary and its SHA-256.
- An isolated test desktop, viewed through a Horizon native VNC Device panel, as
  [the device smoke fixture](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel)
  says.
- One agent panel in that workspace, with the `block_panel` tool and a shell. The
  procedure calls this panel *the agent*. Section 7 needs no such panel.
- `git` on the computer of the fixture.
- A state folder for this run, for example `/var/tmp/worktree-card-1`.

## 5. Setup

1. Make the state folder and the throwaway repository:

   ```bash
   mkdir -p /var/tmp/worktree-card-1/repo
   cd /var/tmp/worktree-card-1/repo
   git init -b main
   git config user.email test@example.invalid
   git config user.name Test
   printf 'one\n' > app.txt
   git add app.txt
   git commit -m "first commit"
   ```

   Result: `git log --oneline` shows one commit on `main`, and
   `git status --porcelain` answers nothing.

2. Write the presets of the settings to a file, so the settings of the fixture
   hold the shell preset that a task opens:

   ```json
   [
     {"name": "Task shell", "alias": "ts", "kind": "shell", "command": "/bin/sh"},
     {"name": "Page", "kind": "browser"}
   ]
   ```

3. Start the fixture with that file, the isolated desktop and the viewer:

   ```bash
   python3 scripts/device-smoke/serve.py --horizon /path/to/horizon \
     --state /var/tmp/worktree-card-1 --native-view \
     --presets /var/tmp/worktree-card-1/presets.json
   ```

   Result: the fixture prints `vnc_address`. Make the native VNC Device panel
   with the `device_panel` tool and that address, and examine `connection`,
   `image_received`, `image_displayed` and an advancing `frame_sequence`.

4. Start the agent panel in the workspace, and give it the repository root of
   step 5.1 and the request: "start this task in its own worktree with the
   `horizon-worktree` skill". Name the preset `Task shell`.

   Result: the agent reads the skill and starts the worktree task.

## 6. Tasks

### 6.1 W1 — Start a task in its own worktree

1. Examine the answer of the agent.

   Result: the agent writes the folder `/var/tmp/worktree-card-1/repo-wt/<slug>`
   and the branch name, and `git worktree list` in the repository shows the new
   worktree. The slug holds letters, digits and `-`, at most 40 characters, and
   the branch name did not exist before.

2. Examine the board of the fixture.

   Result: one new panel stands beside the agent panel, in the same workspace.
   Its titlebar shows the chip `opened by <agent>` and the title of the task. It
   does not take the keyboard focus. A shell runs in
   `/var/tmp/worktree-card-1/repo-wt/<slug>`: type `pwd` in that panel and press
   Enter.

3. Examine the answer of `block_panel` for the `open_panel` request.

   Result: the answer is `opened` with a `panel_id`. The agent sends no
   `command`, no `args` and no `environment`.

### 6.2 W2 — Examine the card of the repository

1. Examine the card that the agent draws.

   Result: one card for the repository, and one row for each worktree of
   `git worktree list --porcelain`. Each row shows the task or folder name, the
   branch, `ahead N`, `M files changed` and the age of the last commit. The row
   of the main worktree says that it is the main worktree.

2. Select the row of the new worktree.

   Result: under the list the card shows `git diff --stat` of that branch and
   the subjects of its last three commits. The text fits in the card.

3. Type one line in `app.txt` in the task panel, and press `Refresh` on the card.

   Result: the row shows one more file changed after the refresh, and no change
   before it.

### 6.3 W3 — Show the changes of one file

1. Press `Show changes` on the card of the selected row, and select one file of
   the list.

   Result: the card shows the diff of that file, cut to the bounds of the card,
   and a line that says that the agent cut the diff when it is long.

### 6.4 W4 — Confirm a merge

1. In the task panel, commit the change of W2 with
   `git add app.txt && git commit -m "second commit"`.

2. Press `Merge` on the card.

   Result: the card shows the commits that will arrive, and one button with the
   label `Merge <branch> into <base>`. No merge runs before the second press.

3. Make a worktree with a file that is not committed, select its row and press
   `Merge`.

   Result: the card says why the merge is refused, and no merge runs.

4. Press the button `Merge <branch> into <base>`.

   Result: `git log --oneline main` in the repository shows the commits of the
   branch. No remote receives a push: `git status` in the base worktree answers
   nothing, and the card shows the new state after a refresh.

5. Make a conflict in the throwaway repository: change the last line of
   `app.txt` on `main` in a new commit, then change the same line in another
   worktree and press `Merge` and its second button.

   Result: the card reports the conflict and the names of the files. The base
   worktree has no conflict marker and no merge in progress: `git status`
   answers nothing, because the agent aborted the merge.

### 6.5 W5 — Remove a worktree

> **CAUTION: THE REMOVE TASK DELETES FILES AND A BRANCH, AND THIS CANNOT BE
> UNDONE.** Press `Remove <folder>` only on the card of the throwaway
> repository, after you read the list of files and commits that it shows.

1. Select the row of a worktree that has one file that is not committed, and
   press `Remove`.

   Result: the card lists the files that are not committed and the commits that
   are on no other branch. One button with the label `Remove <folder>` waits.

2. Press that button.

   Result: the folder is gone from `git worktree list`, and the branch is
   deleted only when it is merged.

3. Press `Remove` on the row of the main worktree, when the card shows one.

   Result: the card refuses the remove and says that the main worktree stays.

### 6.6 W6 — Examine the card after its owner ends

1. Close the agent panel with its close button.

   Result: the card stays in the workspace. Its buttons and its rows are drawn
   disabled, and each badge shows the prefix `Last seen: `.

### 6.7 W7 — Examine the refusals of `open_panel`

1. From an agent panel, call `block_panel` with
   `{"operation":"open_panel","preset":"No such preset","directory":"/var/tmp/worktree-card-1/repo"}`.

   Result: the answer is `unknown_preset`, and the message lists the names of the
   presets, such as `Task shell (ts)` and `Page`. The message holds no command.

2. Call it with the preset `Task shell` and the folder
   `/var/tmp/worktree-card-1/gone`.

   Result: the answer is `directory_missing`, and no panel opens.

3. Call it with the preset `Page` and the folder
   `/var/tmp/worktree-card-1/repo`.

   Result: the answer is `invalid_request`, and the message says that the preset
   opens no program of this computer.

4. Call it eight times with the preset `Task shell` and the folder
   `/var/tmp/worktree-card-1/repo`.

   Result: the first eight answers are `opened`, and the ninth answer is
   `rate_limited` with a number of seconds.

## 7. The card without a model

This section uses `scripts/device-smoke/card_agent.py` with the scenario
`worktree`. It shows `open_panel` and the worktree card with real git, and it
needs no model and no login. The tasks of section 6 use a real agent, and this
section does not replace them.

### 7.1 Start the card

1. Make a new state folder, and start the fixture with the worktree card:

   ```bash
   python3 scripts/device-smoke/serve.py --horizon /path/to/horizon \
     --state /var/tmp/worktree-card-2 --native-view --worktree-card
   ```

   Result: the fixture makes the repository `/var/tmp/worktree-card-2/data/repo`
   with one commit on `main`, writes the presets `Task shell` and `Page`, and
   opens the agent panel `Worktree card agent`. The card agent makes the task
   worktree `/var/tmp/worktree-card-2/data/repo-wt/first-change` on the branch
   `task/first-change`, and opens the panel `first change` there with the preset
   `Task shell`.

2. Examine the log lines of the agent panel.

   Result: they hold these lines:

   ```
   card-agent: worktree: branch task/first-change in .../repo-wt/first-change from main
   card-agent: worktree: open_panel with 'Task shell' answered opened (panel <panel>)
   card-agent: worktree: open_panel with an unknown preset answered unknown_preset (Task shell (ts), Page)
   card-agent: worktree: open_panel with a folder that is not there answered directory_missing (...)
   card-agent: worktree: open_panel with the browser preset 'Page' answered invalid_request (...)
   ```

   The last line names a preset that opens no program of this computer. The card
   itself shows no refusal of the start.

### 7.2 The card of the repository

1. Examine the card `Worktrees of repo`.

   Result: one selectable row for each worktree of
   `git -C /var/tmp/worktree-card-2/data/repo worktree list --porcelain`. The row
   `repo` is the main worktree, and its subtitle holds `main worktree`. The row
   `first-change` holds the branch `task/first-change`, `ahead 0`,
   `0 files changed` and the age of the last commit.

2. Select the row `first-change`.

   Result: under the list the card shows the text of `git diff --stat main...` of
   that branch and the subjects of its last three commits. It holds the buttons
   `Refresh`, `Show changes`, `Merge` and `Remove`.

### 7.3 Refresh and Show changes

1. Type this line in the panel `first change`, and press Enter:

   ```bash
   printf 'two\n' >> app.txt
   ```

2. Press `Refresh`.

   Result: the log line `card-agent: worktree: Read 2 worktree(s) again.` appears,
   and the row `first-change` shows `1 file changed`.

3. Press `Show changes`.

   Result: the card lists the file `app.txt` as a selectable row, and it shows
   the diff of that file. The card holds the button `Worktrees`.

4. Select the row `app.txt`, then press `Worktrees`.

   Result: the card shows the worktrees again.

### 7.4 Merge

1. In the panel `first change`, commit the change:

   ```bash
   git add app.txt && git commit -m "second change"
   ```

2. Select the row `first-change`, and press `Merge`.

   Result: the card shows the commits that arrive in `main`, and it holds the
   button `Merge task/first-change into main`. No merge runs yet.

3. Press the button of step 2.

   Result: the card says `Merged task/first-change into main. Nothing was pushed.`
   The row shows `ahead 0`. In the repository, `git log --oneline main` holds the
   commits of the branch, and `git status --porcelain` answers nothing.

4. Type one line in a new file `dirty.txt` in the panel `first change`, press
   `Refresh`, and press `Merge`.

   Result: the card refuses the merge and says that the worktree has items that
   are not committed. The card holds no second button.

5. Select the row `repo`, and press `Merge`.

   Result: the card says that the row is the main worktree, and that a merge
   arrives there. The card holds no second button.

6. Make a conflict in the throwaway repository only. In `/var/tmp/worktree-card-2/data/repo`,
   change `app.txt` on `main` and commit it, then start a second worktree on
   another branch, change the same line there and commit it:

   ```bash
   cd /var/tmp/worktree-card-2/data/repo
   printf 'the base\n' > app.txt
   git commit -am "the base change"
   git worktree add -b task/conflict ../repo-wt/conflict main
   cd ../repo-wt/conflict
   printf 'the branch\n' > app.txt
   git commit -am "the branch change"
   ```

   Press `Refresh`, select the row `conflict`, press `Merge`, and press the second
   button.

   Result: the card names `app.txt` and says that the merge was aborted.
   `git status --porcelain` in the base worktree answers nothing, and
   `git log --format=%s -1 main` answers `the base change`.

### 7.5 Remove

> **CAUTION: PRESS `Remove <folder>` ONLY AFTER YOU READ THE LIST ON THE CARD.**
> The second press deletes the folder and its files, and it deletes the branch
> when the branch is merged. This cannot be undone.

1. Select the row `first-change`, and press `Remove`.

   Result: the card lists the files that are not committed, which holds
   `dirty.txt`, and the commits that are on no other branch. It holds the button
   `Remove first-change`, and the folder is still there.

2. Press `Refresh` instead of that button.

   Result: the second button is gone, and the folder is still there.

3. Press `Remove`, then press `Remove first-change`.

   Result: the folder is gone from `git worktree list`. The branch is merged into
   `main`, so the card says that it is deleted, and
   `git -C /var/tmp/worktree-card-2/data/repo branch --list task/first-change`
   answers nothing.

4. Select the row `conflict`, press `Remove`, and press `Remove conflict`.

   Result: the folder is gone. The branch holds a commit that `main` does not
   have, so the card says that it stays, and
   `git -C /var/tmp/worktree-card-2/data/repo branch --list task/conflict` names
   `task/conflict`.

5. Select the row `repo`, and press `Remove`.

   Result: the card says that the row is the main worktree, so it stays. The card
   holds no second button.

### 7.6 The pass criteria of this section

- The start makes one task worktree, opens its panel, and prints the three
  refusals of `open_panel`.
- Each row shows the branch, `ahead N`, the changed files, the age of the last
  commit, and the main worktree.
- `Refresh` and `Show changes` read git again and show the cut diff.
- A merge needs two presses, refuses a dirty worktree and a branch that is not
  ahead, aborts a conflict and pushes nothing.
- A remove needs two presses, deletes a branch only when it is merged, and
  refuses the main worktree.

## 8. Pass criteria

- W1 opens one panel in the new worktree, with the mark and without the keyboard
  focus.
- W2 and W3 show the state of each worktree and the changes of one file, inside
  the bounds of the card.
- W4 merges only after the second press, reports a conflict, aborts it, and
  pushes nothing.
- W5 removes only after the second press, keeps the main worktree, and deletes a
  branch only when it is merged.
- W6 leaves the card with disabled buttons after its owner ends.
- W7 answers each refusal with its own code, and the `unknown_preset` message
  holds names only.

## 9. Cleanup

1. Close the task panels and the agent panel with their close buttons.

2. Close the native VNC Device panel through `device_panel`. The viewer closes
   and the app keeps running.

3. Stop the fixture with the stop command that it printed.

   Result: the fixture ends its own display, window manager, VNC server and
   Horizon process. The desktop of the person keeps its own Horizon.

4. Remove the state folder after you keep the evidence that you need, and the
   state folder of section 7 when you ran it:

   ```bash
   rm -rf /var/tmp/worktree-card-1 /var/tmp/worktree-card-2
   ```

   Result: the throwaway repository, its worktrees and the private home of the
   fixture are gone.

## 10. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
