# Cards of the orchestrator of a workspace

The orchestrator of a workspace is the agent program that the orchestrator dock
runs with no terminal (`crates/horizon-ui/src/app/orchestrator_dock/`). This page
states its identity, what its program gets at a start, how the host accepts it,
which operations of `block_panel` it reaches, and what this part does not build.

The card itself keeps the rules of
[`../review/trust-boundaries/block-panel.md`](../review/trust-boundaries/block-panel.md).
The program and its line format keep the rules of
[`agent-bridge.md`](agent-bridge.md).

The front desk is the second conversation of the dock. It belongs to no
workspace, so it owns no card and no workspace. [`front-desk.md`](front-desk.md)
states its actor, its program and what it cannot do.

## The actor

The actor of the orchestrator of the workspace with local id `W` is
`horizon:orchestrator/<W>`. The module
[`crates/horizon-core/src/agent_bridge/actor.rs`](../../../crates/horizon-core/src/agent_bridge/actor.rs)
builds it and parses it.

- Only a local id of the form that `new_local_id` makes is accepted: 36 bytes of
  lower-case hexadecimal in groups of 8, 4, 4, 4 and 12, divided by hyphens.
- The form refuses every other text. An actor therefore holds no slash, no dot,
  no space and no control character, so the name of a person or of a file cannot
  be spelled as an actor.
- The prefix differs from the actor of a panel, `horizon:<local id>`, and from
  the actor of a session of a machine, `horizon:machine/<machine>/<session>`.
  One actor names one caller.
- A workspace whose local id this form refuses gets no actor, and its
  orchestrator draws no card: the host answers `caller_unavailable`.

## What the child gets at a start

`agent_bridge::program::claude_for_workspace` builds the program of one
conversation. It is `agent_bridge::program::claude` plus the identity of the
workspace:

- the `--plugin-dir` of Claude Code, which is the directory that an agent panel
  of that kind gets, so the program has the Horizon MCP server and the Horizon
  skills. The argument is added only while that directory is a directory of this
  computer;
- the environment variables that an agent panel gets, from the one function that
  an agent panel uses (`agent_identity_vars`): `HORIZON=1`,
  `HORIZON_BROWSER_ACTOR=horizon:orchestrator/<W>`,
  `HORIZON_BROWSER_HOST_INSTANCE` and `HORIZON_EXECUTABLE`.

The `BridgeIdentity` of a start holds the actor, the host instance, the home of
this Horizon, its executable and the plugin directory. `HORIZON_HOME` is not
written into the environment of the child: the child inherits it from this
Horizon as a panel does, so the tools of the orchestrator read the folder that
the host reads and keep the rule that a panel's home keeps.

The start adds no work-resume plugin, no `HORIZON_WORK_*` variable and no named
secret, because this part has no turn ledger, no panel of the board and no key
of the person.

## The caller rule

The host maps an actor to a caller before it applies a request
(`crates/horizon-ui/src/app/block_requests.rs`, `request_caller`).

- A panel actor is the caller `Panel`, as before.
- An actor that parses as an orchestrator actor is the caller `Orchestrator`, and
  only while the dock holds a running bridge for the workspace with that local
  id (`OrchestratorDock::running`). The identity belongs to a program that
  Horizon started, and not to the workspace: a workspace on the board whose
  orchestrator ended, or that never started one, refuses the actor with
  `caller_unavailable`. A read of a card of that owner follows the same rule, and
  the answer names the caller and no workspace.
- A machine session actor is the caller `Session`, as before.
- The actor of the front desk, `horizon:front-desk`, is the caller `FrontDesk`,
  and only while the dock runs the program of the front desk. It reaches the three
  operations of the work items, and every other operation of the tool answers
  `caller_unavailable` for it. `owner_workspace` answers `None` for it, because it
  owns no workspace: it creates no card, reads no card and opens no panel. See
  [`front-desk.md`](front-desk.md#the-actor).

A caller of `Orchestrator` has no panel, so:

- its card is placed by the rule that a card of a machine session uses: the card
  is a `CardPlace::Tile` of that workspace, and the board puts it on a free tile
  that covers no panel of the workspace
  (`crates/horizon-core/src/board/arrangement.rs`, `free_tile_position`). The
  column of an agent panel and the `beside` anchor of that column are not
  offered, because no panel of this caller exists;
- an operation that needs the panel of the caller is refused with the refusal
  that a caller without a panel gets, `caller_unavailable`.

The owner of a card of an orchestrator is the actor, so `update`, `visibility`,
`close` and `next_click` require that exact actor, and another agent of the same
workspace gets `not_owner`.

## The presence of the owner

`card_owner_present` answers `true` for a card of an orchestrator while the dock
holds a running bridge for that workspace (`OrchestratorDock::running`) and the
card stands in that workspace. A card that the person moved to another workspace
has no owner there, as a card of a panel owner has none once that panel is in
another workspace: the program of the dock reads and closes only the cards of
the workspace that its actor names. The state chip of the card names the program
of the dock and says whether it listens. While no bridge runs, the chip says
`No agent` and every control of the card is drawn disabled.

## The wake of the orchestrator

A press on a card whose owner holds no open `next_click` wakes the owner. An agent
panel reads that wake in its terminal; an orchestrator has no terminal, so the
same line reaches its program as one turn of its bridge
(`crates/horizon-ui/src/app/block_clicks.rs`, `WakeTarget`).

- The line is the constant [`WAKE_LINE`](../../../crates/horizon-core/src/block_panel/wake.rs)
  of the terminal wake. The rules of that file decide whether it goes out:
  `wake_verdict` holds it while the owner is absent or works on a turn, while a
  line already went out for this streak of presses, and for 30 seconds after the
  last line to that owner. Horizon copies no rule and adds none.
- The turn of the wake makes the conversation busy, and the conversation names it
  with the notice `A press on a card woke the orchestrator`. It adds no message
  of the person: the wake line is Horizon's own sentence, and the program answers
  it as an ordinary turn.
- `Conversation::system_said` adds that notice and the busy state with no
  [`Entry::Person`](../../../crates/horizon-core/src/agent_bridge/conversation.rs).
  A conversation that runs a turn is `Working` for `wake_verdict`, so a second
  press of the same streak waits for a later frame.
- An orchestrator should still keep one `next_click` open while a card of its own
  waits for the person: a press then reaches the program at once, and no wake
  line is needed. A press that no wait and no wake reads stays in the queue of the
  card for five minutes, at most 32 presses per card.

## The reference rows of the cards of a conversation

The dock draws one row for each card that its program drew in the workspace of the
conversation (`Entry::Card` of `horizon-core`).

- A create and an update of a card add the row of that card, or move the row that
  the card already has to the heading of its document. One card keeps one row,
  and a second create of the same panel adds none.
- The row holds the local id of the card and the heading of its document, cut to
  80 characters, with each control character replaced. The heading is text of the
  program, so the dock draws it as plain text, with an ellipsis at the end of one
  line (`crates/horizon-ui/src/app/orchestrator_dock/list.rs`).
- A press on the row brings the card into view, through the same path that the row
  of the sidebar uses (`reveal_selected_panel`). The row sends nothing to the
  program.
- A new card of an orchestrator comes into the view of the canvas by itself while
  the dock shows the conversation of that orchestrator: the person wrote to it,
  and the card is its answer. The keyboard stays where it is. A card of a
  conversation that the dock does not show, and a card of an agent panel or of a
  machine, moves no view (`show_card_of_shown_conversation` in
  `crates/horizon-ui/src/app/block_requests/caller.rs`).
- A row whose card left the board is drawn dim, says `closed` at its right end,
  and takes no press. The stored conversation keeps that row, so the row of a card
  of an earlier run reads as a closed row after a restart: Horizon loads the local
  id of the card, and no panel of the board answers to it.
- The person reads the press of a button of such a card in the conversation too:
  the notice `You pressed <label> on <heading>` names it, with the label and the
  heading cut to 40 characters each. The notice is the words of Horizon and it
  holds no other field of the card.

## A card that asks a question stands in the sidebar

A callout with buttons is the way that a card asks the person something. The
sidebar keeps the questions that the cards of the board ask in its group
`Needs you`, at the top of the window above the workspaces, so the person
answers a question from wherever the person looks
(`crates/horizon-ui/src/app/sidebar/inbox/`).

- **What a question is.** A card panel that is open, whose document is not
  `finished`, and that holds at least one `callout` with at least one button that
  the person did not press. The first such callout of the card is its question.
  The record of an answered callout is the one that
  [`CardRuntime`](../../../crates/horizon-core/src/block_panel/card.rs) keeps, as
  the attention mark of the card reads it: the press of this run, or the answer
  that the saved card holds. The owner of a card may be an orchestrator or an
  agent panel; a card of Horizon's own asks for a setting on its own path and
  stands in no group.
- **The order is the order of the cards.** The oldest first, by the time of the
  last update of a card, which is the time that the document with that callout
  arrived, and then by the order of the board. The group shows three questions
  and one line `and <N> more`.
- **A press on a button of the group is the press on that button of the card.**
  The group calls the one path of the clicks of a card
  ([`crate::app::block_clicks`](../../../crates/horizon-ui/src/app/block_clicks.rs),
  `press_card_button`), which records the click, the note of the press and the
  answer that it is in the runtime state of the card. The same path answers a
  `next_click` that waits, sends the wake line when nobody listens, names the
  press in the conversation of an orchestrator and saves the answer with the
  card. The group copies none of those steps, so a question ends at the press
  that ends the mark of its row. A press beside the buttons, and a press on
  `Open`, reveals the card on the path of a workspace row (`reveal_new_panel`):
  the workspace becomes the active one and the canvas shows the panel.
- **The text is text of a program.** The text of a callout and the label of a
  button are untrusted, so the group reads them as plain text, with each control
  character replaced by a space, and cuts the text to 160 characters and a label
  to 24 before any layout reads its length.
- **One read for the group and the rows.** The questions are read with the rows
  of the workspace list, at most once a second
  ([`crates/horizon-ui/src/app/sidebar/list.rs`](../../../crates/horizon-ui/src/app/sidebar/list.rs),
  `ListCache`), and at once when the version of the attention states changes or
  a card of the board comes, goes or holds another document. A frame lays out
  the texts of the cache, so a frame that changes nothing builds no string, and
  the sidebar asks for the frame that is due, so a question shows without a move
  of the pointer.
- **The line of a workspace row names its orchestrator.** The same read takes
  the turn of each conversation (`OrchestratorDock::turn_of`) and makes one word
  of the row, in the order of weight of
  [`Orchestrator`](../../../crates/horizon-core/src/workspace_list.rs):
  `Needs you`, `Failed`, `Working`, `Done`. A turn that ended is news while the
  dock does not show that conversation; a conversation that the dock draws, in
  the active workspace, clears it. A failed turn is one of the lines that the
  conversation writes for it, so the count of those lines tells a turn that
  failed from one that ended well.

## The operations of this caller

| Operation | This caller | Reason |
| --------- | ----------- | ------ |
| `create`, `update`, `list`, `inspect`, `next_click`, `visibility`, `close` | works | the card operations need the workspace of the caller and no panel |
| `open_terminal` | `caller_unavailable` | the terminal stands beside the panel of the caller and carries the mark of that panel |
| `open_panel` | `caller_unavailable` | the panel of a preset stands beside the panel of the caller, and no panel of this caller exists |
| `presets` | works | a read of the names of the settings, which needs no actor |
| `machine_set`, `machine_delete`, `machines`, `machine_key`, `machine_keys`, `delete_machine_key` | works | the machine list of this Horizon takes no actor, and the orchestrator rents machines of it |
| `secret_names` | works | the answer is the names of the stored secrets and no value |
| `server_status`, `browser_host_status` | works | a read of the entry that the settings hold |
| `server_link_request`, `browser_host_request` | `caller_unavailable` | the card of a setting request belongs to a panel of the workspace, and its answer reaches that panel |
| `work_items`, `work_item_propose`, `work_item_message` | `caller_unavailable` | the three operations of the front desk, which belongs to no workspace |
| the browser page tools, `browser_*` | unchanged, no panel | the actor of an orchestrator is in no workspace stamp, so `browser_list` names no panel, `browser_create` answers `workspace_unavailable`, and every other page operation answers the refusal of a caller outside the workspace of the panel |
| `device_panel` | unchanged, `caller_unavailable` | a Device request resolves its actor to an agent panel of the board |

Nothing in this table is a right that an agent panel does not have.

## What is not built

- **No named secret.** A later part decides whether the orchestrator gets a key
  of the person.
- **No work ledger.** The program gets no work-resume plugin and no
  `HORIZON_WORK_*` variable, so the attention of the dock is the state of the
  conversation and not the state of a turn of a panel.
- **No persistence.** Nothing here is saved: the conversation, the bridge and the
  presence of the owner end with this run of Horizon.
