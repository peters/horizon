# The front desk

The front desk is one conversation of the orchestrator dock. It belongs to no
workspace. The person writes there, and the front desk turns the words into work
items. This page states what the front desk is, which actor it uses, which
program it runs and why that program has no built-in tool, and what it cannot do
in this part.

The dock itself keeps the rules of
[`../review/trust-boundaries/agent-bridge.md`](../review/trust-boundaries/agent-bridge.md).
The conversation of a workspace keeps the rules of
[`orchestrator-cards.md`](orchestrator-cards.md).

## One more conversation

The dock keeps its conversations by a `String` key.

- A workspace uses its `local_id` as the key. That value is a UUID.
- The front desk uses the reserved key `front-desk`
  (`orchestrator_dock::FRONT_DESK`). A UUID never equals that name, so the two
  kinds of key cannot meet. A test states this rule.
- The front desk keeps the same parts as a conversation of a workspace: a draft,
  a session, a bridge, a stored file and one record in the runtime state. It uses
  the same code paths. The dock copies none of them.
- The dock never forgets the front desk when a workspace leaves the board, because
  the front desk belongs to no workspace.
- The close of Horizon, a switch of the saved session and the stop control of the
  composer stop the program of the front desk as they stop the program of a
  workspace.

The record of the front desk is `RuntimeState.front_desk`. That field stands
beside `workspaces`, because the board holds no workspace for it. A state file of
an older build holds no such field, and it loads: the front desk then starts a new
session.

## Which conversation the dock shows

The dock shows one conversation at a time, and one flag of this run chooses it.

- The start of Horizon shows the front desk.
- A press on a row of the workspace list shows the workspace of that row. A press
  on the row that is already active shows it too.
- A change of the active workspace of the board shows that workspace.
- The back control of the header, and the command `Show Front Desk` of the
  palette, show the front desk.
- A board without a workspace shows the front desk.

While the front desk shows and the dock is visible, the workspace list draws no
row as active and opens no panel row under a workspace. This is a rule of the
drawing alone. The active workspace of the board and the focus of the canvas do
not change. A dock that is not visible leaves the list as it is.

## The actor

The actor of the front desk is `horizon:front-desk`
(`crates/horizon-core/src/agent_bridge/actor.rs`, `actor::FRONT_DESK`). It holds
no local id, and `actor::workspace_of` answers `None` for it.

`BridgeIdentity::front_desk()` builds the identity of that actor. The identity
holds the host instance, the home and the executable of this Horizon, as the
identity of a workspace does.

The host maps an actor to a caller before it applies a card request
(`app/block_requests/caller.rs`). The front desk owns no workspace, so it creates
no card, reads no card and opens no panel. Every request of that actor is refused
with `caller_unavailable`. This part adds no `CardCaller` variant for the front
desk, so the refusal grants nothing.

## The program

`agent_bridge::program::claude_for_front_desk` builds the program of the front
desk. Its command line is the line of `claude_for_workspace`, and then three
additions:

- `--tools ""` gives the program no built-in tool. The text of `claude --help`
  names the empty word as the form for none.
- `--model <model>` names the model of the front desk. The value is `haiku`, or
  the value of the environment variable `HORIZON_FRONT_DESK_MODEL`. That value is
  used only when it is a name of at most 64 bytes of the letters, digits, dot,
  hyphen and underscore, and when it does not start with a hyphen. Every other
  value gives `haiku`.
- `--append-system-prompt <text>` gives one constant text
  (`agent_bridge::program::FRONT_DESK_PROMPT`, about 120 words). The text says
  that the program is the front desk of Horizon, that the person writes one
  message with one task or more, that each task becomes a work item with its own
  orchestrator, and that the front desk never does the work itself.

The working directory of the program is `<HORIZON_HOME>/front-desk`. The worker of
the dock makes that folder before a start, so no frame reads a directory and no
frame makes one.

`HORIZON_ORCHESTRATOR_PROGRAM` replaces the path of the program of the front desk
as it replaces the path of the program of a workspace. The arguments do not
change.

## Why the program has no built-in tool

The front desk writes the words of the person to the work items. It does no work
itself.

A built-in tool would let the program read a file, run a command, or change this
computer, with the rights of the person and without a work item. The person does
not see such work in the card of a task. The front desk therefore holds no tool,
and its answer is a text only.

The identity of the front desk still holds the plugin directory of Claude Code
when that directory is there, because the next part of this work adds the tool
that makes the work items. A plugin that is not there leaves the program with no
tool at all.

## What the front desk cannot do in this part

- It starts no work item, no panel and no card. The words of the person stay in
  the conversation.
- The sidebar button `+ Rent a machine` is disabled while the front desk shows.
  Its hover text is `Open a work item first`, because a machine belongs to one
  work item.
- It has no chip under the field. The chips `Orchestrator`, `Workers` and `Place`
  say nothing about a conversation that belongs to no workspace.
- It has no tile and no back control in its header. The front desk is the place
  that a back control returns to.

## What the design of the parts states

The numbers of the header of the front desk, of its back control, of its empty
state and of its composer stand in
[the design guidelines](../design/guidelines.md#orchestrator-dock).
