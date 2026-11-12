---
name: horizon-worktree
description: Run each task of a repository in its own git worktree, show the worktrees of one repository on a Horizon card, and merge or remove one when the person presses a button. A task starts in a panel of a preset that the person saved, through the open_panel operation of block_panel. Use it when the person asks for a task in its own worktree, or asks to see the worktrees of a repository.
---

# One worktree for each task

A person who runs more than one agent on one repository keeps each task in its
own git worktree. One task cannot change the files of another task, and the
person sees each task, its branch and its changes on one card.

This skill uses two things that Horizon gives to an agent:

- `block_panel` draws the card, and its `open_panel` operation opens the panel of
  a task. Read the `horizon-block` skill for the document of a card and for
  `next_click`.
- The card sends each press of a button, and each selection of a row, to you as
  one click.

## When to use this skill

Use it in two cases only:

- The person asks for a task "in its own worktree", or asks for one worktree for
  each task of a repository.
- The person asks to see the worktrees of a repository.

Do not use it for a task that the person did not ask to isolate.

## The rules of the repository come first

Read `AGENTS.md`, `CLAUDE.md` or the same file of the repository before you
start. A rule of the repository comes before this skill:

- It may name the folder of the worktrees.
- It may name the pattern of a branch name.
- It may name the method of a merge, such as a squash or a fast-forward.

Use the value that the repository names. Use the value in this skill only when
the repository names none.

## Start a task in a new worktree

1. Find the root of the repository: `git rev-parse --show-toplevel`.
2. Find the default branch: `git symbolic-ref --short refs/remotes/origin/HEAD`.
   When it answers nothing, ask the person for the base branch.
3. Run `git fetch` in the root.
4. Make the slug of the task: letters, digits and `-`, at most 40 characters,
   for example `fix-login-redirect`.
5. Examine that the branch name is free: `git rev-parse --verify <branch>`
   must fail. Never use a branch name that exists.
6. Make the worktree:
   `git worktree add -b <branch> <root>-wt/<slug> <base>`.
   Use the folder that the repository names, when it names one.
7. Open the panel of the task with `open_panel`:

   ```json
   {"operation": "open_panel", "preset": "<the preset that the person named>",
    "directory": "/home/person/repo-wt/fix-login-redirect", "title": "fix login"}
   ```

   The folder is an absolute path. Horizon opens the panel in your workspace,
   beside your panel, and the panel runs the command of that preset in that
   folder. The answer is `opened` with the `panel_id`.
8. Tell the person on the card that the new panel waits for the text of the
   task. The person types the task in that panel.

Ask the person which preset to use when the request names none. The answer of
`unknown_preset` lists the names of the presets that the person saved; show that
list and ask again. Do not guess.

`open_panel` refuses a preset of a kind that opens no program of this computer,
such as a browser page or a native viewer. Ask for a shell, a command or an agent
preset instead.

One agent panel opens at most eight panels in ten minutes. When the answer is
`rate_limited`, tell the person the number of seconds to wait.

## Draw the card

Draw one card for the repository, not one card for each worktree. Read the
worktrees with `git worktree list --porcelain` and draw one row for each of them:

- the title: the name of the task, or the name of the folder when the worktree
  has no task;
- the subtitle: the branch, `ahead N` of its base, `M files changed` and the age
  of the last commit, for example
  `task/fix-login · ahead 3 · 2 files changed · last commit 4 hours ago`.

Count the changed files with `git status --porcelain` in that worktree. Count
the commits that are ahead of the base with
`git rev-list --count <base>..<branch>`.

Give each row an id that names the folder, for example `worktree.fix-login-redirect`,
and use `"selection": "single"`. Mark the main worktree in its subtitle, because
the person cannot remove it.

Under the list, show what the selected row holds:

- `git diff --stat <base>...<branch>` of that worktree, cut to the bounds of a
  card;
- the subjects of the last three commits of that branch.

Use the component `text` for that text, and cut it to 512 characters. A card
holds 32 rows in one list and 64 components. A repository with more worktrees
than 32 shows the first 32 and one line that says how many rows are not shown.

Give the card these buttons:

- `refresh`, tone neutral, label `Refresh`;
- `changes`, tone neutral, label `Show changes`;
- `merge`, tone primary, label `Merge`;
- `remove`, tone danger, label `Remove`.

Call `update` with the whole document after each change of the worktrees.

## Refresh and show changes

On `refresh`, read the worktrees again and call `update` with the new document.

On `changes`, run `git diff --stat <base>...<branch>` in the selected worktree.
When the person selects one file of that list, run
`git diff <base>...<branch> -- <file>` and show it, cut to the bounds of a card.
Never show a whole diff of a large change: cut it and say that you cut it.

## Merge a task

A merge changes the base branch, so ask before you merge.

1. On `merge`, examine the worktree of the selected row:
   - `git status --porcelain` must answer nothing, because a merge of a
     worktree with changes that are not committed loses nothing but is refused;
   - `git rev-list --count <base>..<branch>` must be more than zero.
   When one of the two rules fails, say why on the card and merge nothing.
2. Show on the card the commits that will arrive: the subjects of
   `git log --oneline <base>..<branch>`, cut to the bounds of a card.
3. Add one button with the label `Merge <branch> into <base>` and the id
   `merge.confirm`.
4. Wait for that button with `next_click`. A click with `stale: true` means the
   document changed after the person pressed: show the commits again and ask
   again.
5. Run the merge in the worktree of the base branch. Use the method that the
   repository names; when it names none, run `git merge --no-ff <branch>`.
6. Never push. A push is a separate request of the person.
7. When the merge reports a conflict, stop and run `git merge --abort` in the
   base worktree. Say on the card which files conflicted. Never force a merge.
8. After a merge, run `Refresh` and tell the person that the branch is merged.

## Remove a worktree

A remove deletes files, so ask before you remove.

1. On `remove`, show on the card what the person will lose:
   - the files that are not committed, from `git status --porcelain`;
   - the commits that are on no other branch, from
     `git log --oneline <base>..<branch>`.
2. Add one button with the label `Remove <folder>` and the id `remove.confirm`.
3. Wait for that button with `next_click`.
4. Run `git worktree remove <folder>` in the root. Add `--force` only when the
   worktree has files that are not committed, and only after the person pressed
   the second button.
5. Delete the branch with `git branch -d <branch>` only when it is merged into
   the base branch. Never delete a branch that is not merged.
6. Never remove the main worktree of the repository. Never remove a worktree
   that the card did not list.
7. Run `Refresh` after a remove.

## The listen loop

1. Draw or update the card with the choice that the person must make.
2. Call `next_click` without `panel_id`.
3. Act on the click.
4. Send the result with `update`.
5. Call `next_click` again.

A click that no `next_click` waits for is kept for five minutes, so a press of
the person is not lost. Do not wait for the wake line when you are in
`next_click`.

## When the card owner ends

The card stays in the workspace when your agent panel ends. Horizon then draws
its buttons and its rows disabled, and each badge with the prefix `Last seen: `.
Say that to the person before you end, so the person knows that a merge or a
remove needs a new agent.

## Errors

- `unknown_preset`: the settings of the person have no preset of that name. The
  message lists the names. Ask the person and use one of them.
- `directory_missing`: the folder is not there or is not a folder. Examine the
  path and make the worktree again.
- `rate_limited`: this agent panel opened eight panels in ten minutes. Tell the
  person the number of seconds to wait.
- `invalid_request`: a field breaks a rule, such as a relative folder, a preset
  of a kind that opens no program of this computer, or a control character. The
  message names the field.
- `panel_unavailable`: Horizon could not open the panel. Read the message and ask
  the person to open the panel from the sidebar.
- `caller_unavailable` or `wrong_host`: the request did not come from an agent
  panel of this Horizon. Start the agent from Horizon.
