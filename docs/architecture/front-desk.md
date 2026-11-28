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

## The default of the dock

The dock shows at the first start of Horizon. The setting
`features.orchestrator_dock` is on at first (`crates/horizon-core/src/config.rs`),
so a new window opens the front desk at its right edge.

- A store that holds `false` keeps the dock off. A store of an older build holds
  no row for the setting and reads the default, which is on.
- The environment switch `HORIZON_ORCHESTRATOR_DOCK=1` shows the dock for the run
  and `HORIZON_ORCHESTRATOR_DOCK=0` hides it for the run, whatever the setting
  says. No other value of the variable is a switch.
- A window that leaves less than 320 points of canvas beside the dock draws no
  dock, so a narrow window keeps its canvas.

While the dock is visible, the first start page sends the person to the front desk
([the design](../design/guidelines.md#the-page-of-the-front-desk)). While it is
not visible, the first start page keeps the four starts.

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
no card, reads no card and opens no panel. Its caller is `CardCaller::FrontDesk`,
and it reaches the three operations of
[the work items](#the-three-operations-of-the-front-desk) and nothing else. Every
other request of that actor answers `caller_unavailable`, and the front desk is
refused while the dock runs no program for it.

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
not see such work in the card of a task. The front desk therefore holds no
built-in tool, and its answer is a text only.

The identity of the front desk holds the plugin directory of Claude Code when that
directory is there, so the program has the Horizon MCP server and the skill
`horizon-front-desk`. The three operations below are the whole right of that
server for this actor. A plugin that is not there leaves the program with no tool
at all.

## The three operations of the front desk

The actor of the front desk reaches three operations of the `block_panel` tool,
and no other. The host accepts them only while the dock runs the program of the
front desk, as it accepts the operations of an orchestrator only while the dock
runs the program of that workspace. Every other caller gets `caller_unavailable`,
and the front desk gets the same refusal for every other operation.

| Operation | What it does |
| --------- | ------------ |
| `work_items` | Reads the work items of this Horizon |
| `work_item_propose` | Offers one proposal of work items to the person |
| `work_item_message` | Passes one message of the person to one work item |

A work item is a workspace with its orchestrator conversation. `work_items`
answers at most 50 work items, in the order of the sidebar: the local id of the
workspace, its name, the state word of its row (`working`, `needs_you`, `failed`,
`done` or `idle`), the text of its first question cut to 160 characters, the last
text of its orchestrator cut to 200 characters, the cost of its ended turns and
the machine that it runs on. It reads the state that a frame already holds, so it
reads no file and no process.

`work_item_message` sends the text to the orchestrator of that work item through
the one send of the dock, and the bubble of that message carries the caption
`via the front desk`. An id that no workspace holds answers `not_found`, a
conversation that runs a turn answers `busy`, and a second message to the same
work item within ten seconds answers `too_soon`. None of them sends anything.

## The proposal and its press

`work_item_propose` takes one to six items. Each item holds a title of 1 to 60
characters on one line, a task of 1 to 4 000 characters and, when the person
named a folder, an absolute path. A request outside those bounds is refused whole
with `invalid_request`, and it changes nothing.

Nothing starts at the request. The items become one entry `Proposal` of the
conversation of the front desk, with an id that Horizon makes and the state
`open`, and the dock draws them as one row with the two controls `Start <N> work
items` and `Not now`. An older proposal that is still `open` becomes `replaced`,
so the person decides one proposal at a time. The answer is the id of the
proposal and the words `The person decides. You get one message when they do.`

A press on `Start <N> work items`:

1. The worker of the dock checks the folder of each item with `is_dir`. No frame
   reads a directory.
2. On the frame that reads that answer, Horizon makes one workspace for each
   item, named by its title, and the board keeps that workspace while it is empty.
   The folder of an item becomes the folder of its workspace only when the check
   found a directory; when it did not, the workspace starts with no folder and
   the conversation names it: `<title>: the folder was not found, so it starts
   with none`.
3. The task of each item goes to the orchestrator of its workspace as the first
   message of the person there.
4. The conversation of the front desk gets the notice `You started <N> work
   items` and one reference row for each new work item.
5. The program of the front desk gets the line `The person started the work items
   of proposal <id>.` as one turn. A conversation that runs a turn keeps that
   line until the turn ends.
6. The active workspace and the conversation that the dock shows do not change.

A press on `Not now` moves the proposal to `declined`, writes the notice `Not
started`, and sends the line `The person did not start the work items of proposal
<id>.`

Horizon writes one notice in the conversation of the front desk for each change
of state of a work item that the front desk started: `<name> needs you`, `<name>
finished` and `<name> failed`. The notice is the words of Horizon, and no turn of
a program writes it.

## What the front desk can never do

- It creates no card, no panel and no terminal, and it reads no card: every other
  operation of the tool is refused for its actor.
- It starts no work item. Only the press of the person starts one.
- It does no work of a task: it has no built-in tool, and its answer is a text.
- It answers no question of a work item. The question of a work item is the
  person's to answer, and a message that the front desk passes carries the caption
  that says who passed it.
- It reads no file and runs no command, and it reads no value of a secret: the
  three operations above are the whole right of its actor.

## What the front desk cannot do in this part

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
