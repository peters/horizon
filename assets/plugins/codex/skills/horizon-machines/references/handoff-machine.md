# Continue a task on a machine

Use this page when the person asks you to continue your task on a machine of the
machine list, and that machine holds no copy of the work or an older copy. The
person asks you in your own panel. No button of Horizon moves a task.

## Stop when the folder is not clean

Run `git status --porcelain` in the folder of the task. Stop and tell the person
when it prints a line and the person did not say what to do with those changes.
Name the changes. Never stash, reset, remove or delete a change on your own, and
never commit a change that the person did not name.

## The steps

1. Make a branch: the name that the person gives, or `work/<short task name>`.
   Commit the work of the task on that branch. Do not commit build output, a
   `.env` file or a local secret.
2. Write `HANDOFF.md` at the root of the repository on that branch. It holds
   five parts: what the task is, what is done, what is next, the first command to
   run, and the check of the repository.
3. Bring the branch to the machine. Make the bundle with `git bundle create` and
   the branch, and copy it with `scp -p`, as
   [repository-machine](repository-machine.md) shows. Then read the folder of the
   field `folder` of the record of the machine:
   - The machine holds no such folder: `git clone` the bundle into it.
   - The machine holds the folder: do not move, rename or replace it. A program
     or a terminal of the person can run in it. Stop when
     `git -C <folder> status --porcelain` prints a line, and tell the person.
     If it prints nothing, run `git -C <folder> fetch <bundle> <branch>:<branch>`
     and `git -C <folder> switch <branch>`.
   Remove the bundle file from the machine after the step.
4. Draw one card. Never draw a second card for the same work.

## The card

```json
{"components": [
  {"type": "heading", "text": "The machine holds work/example",
   "badge": {"text": "Moved", "tone": "ok"}},
  {"type": "text", "text": "The machine holds the branch at 1a2b3c4. This folder is unchanged; nothing was deleted."},
  {"type": "kv", "rows": [{"label": "Not moved",
    "value": "unsaved edits, build output, a .env file, a database or container, a localhost port, registry credentials, the conversation, your login"}]},
  {"type": "buttons", "items": [
    {"id": "start_claude_here", "label": "Start Claude here"},
    {"id": "close", "label": "Close"}]}]}
```

The heading names the branch. The text line names the commit and says that this
folder is unchanged and that nothing was deleted. The row `Not moved` names what
did not move: unsaved edits, build output, a `.env` file and local secrets, a
database or a container, a `localhost` port, registry credentials, the
conversation and the login. The button `Start <program> here` is the button of
[repository-machine](repository-machine.md), with the same rule for the session
name and the same folder; draw it for each agent program that the machine has.
`Close` closes the card.

## The limits, to the person and on the card

- The conversation does not move. The new agent on the machine reads
  `HANDOFF.md` and starts a new conversation.
- The note is a claim. Its first command can fail on the machine, because the
  machine has another toolchain or no `.env` file. Say so, and promise nothing
  that you did not run there.
- The local panel keeps running until the person closes it. Nothing of this
  computer stops.
- No login file, token, keyring, cookie or `~/.ssh` of this computer is copied to
  the machine. The person signs in on the machine, as the skill says.
