---
applyTo: "crates/horizon-core/src/agent_work/**,crates/horizon-core/src/attention.rs,crates/horizon-core/src/attention/**,crates/horizon-core/src/workspace_list.rs,crates/horizon-ui/src/agent_work_hook.rs,crates/horizon-ui/src/plugin_install/work_hooks.rs,crates/horizon-ui/src/app/attention.rs,crates/horizon-ui/src/app/attention/**,crates/horizon-ui/src/app/sidebar/list.rs,crates/horizon-ui/src/app/sidebar/rows.rs,crates/horizon-ui/src/app/sidebar/panel_rows.rs"
---

# Trust boundary of the agent work ledger and of the attention state

Horizon installs lifecycle hooks in an opted-in agent panel. Each hook event
reaches Horizon as one line of JSON on the standard input of
`<candidate> --agent-work-hook`, with the panel identity in the environment of
the launch. The hook writes one **work ledger** per panel in the Horizon home.
The sidebar reads that ledger to say which agent panel needs the person, which
one finished while the person looked elsewhere, and which one works.

The module comment of
[`agent_work/ledger.rs`](../../../crates/horizon-core/src/agent_work/ledger.rs)
and of [`store.rs`](../../../crates/horizon-core/src/agent_work/store.rs) state
the same rules for the record. [`attention.rs`](../../../crates/horizon-core/src/attention.rs)
holds the only code that turns a record into a state.

## Untrusted: every lifecycle event of an agent process

The event comes from the agent process, which runs as the same local user as
Horizon. The person runs that agent, and its output is not Horizon's text.

- **The launch identity gates the event.** The environment names the Horizon
  root, the panel and the launch token. The store accepts an event only for a
  panel that `register_owner` registered, only while that launch's health marker
  holds `1`, and only while the record names the same owner token and panel kind.
  An event of another launch is refused, and a refused hook ends the launch's
  evidence instead of writing it.
- **The session must match.** An event carries a session identity. A ledger
  accepts it only when the ledger holds that session or no session yet, so a
  delayed event of an older session cannot reopen or close a later turn.
- **An event of another panel changes nothing.** The panel and the launch token
  come from the environment that Horizon set at the launch, not from the event.
  One panel therefore cannot write the state of another panel.
- **A clock that goes back is not a turn.** An event whose time is before the
  record leaves the ledger `unknown`, and an unknown turn is idle.
- **The event is bounded and checked.** The input is at most 1 MiB, the session
  identity at most 512 bytes, and the working directory and the transcript path
  must be absolute. A record is at most 256 KiB, and a larger or a corrupt one
  raises an error instead of a state.
- **An older read is dropped.** A read of a record that is older than the one
  the state already holds changes nothing, so two reads that arrive out of order
  cannot move the state backwards.
- **A relaunch starts its own timeline.** A record of another session or of
  another launch of the panel is read from its own start, and it does not show
  the turn that ended before it as unread.

## Horizon guarantees: metadata only, and one panel at a time

- **No prompt and no tool content is stored.** A record holds the session, the
  turn, the event name, the tool identity, the working directory, the transcript
  path and, for a stop, the count of the background work. A prompt, an answer,
  the arguments of a tool and the text of the transcript are not part of it. The
  attention state reads the record and nothing else.
- **The state is not a claim about the work.** `Needs you` says that a hook
  reported an open question or a failed turn. It does not say that the question is
  still open in the agent, and it is not a permission. A permission that the
  person grants in the agent panel is not granted to any other panel.
- **The screen is the fallback, not a second rule.** An agent kind whose launch
  registered no ledger takes its state from the working indicator of its own
  terminal. A panel that has a ledger never falls back to the screen, so two
  rules never decide one panel.
- **The unread mark is memory.** Whether the person has looked at a finished
  turn lives in the process and is not saved. A restart shows the state of the
  ledger, and every panel starts as looked at.
- **The files stay private.** The work directory is created `0700`, and a record
  and a health marker are written `0600` through a temporary file and a rename.
- **No frame waits.** A frame reads a value that the attention worker published.
  The worker reads the records of the home, and it wakes the window only when its
  answer differs from the one before it.

## Does not guarantee

- **A hook is evidence, not proof.** A process of the person that can write the
  hook input or the record files can make any state appear. The boundary keeps
  one panel out of another panel, and it keeps content out of the record; it does
  not authenticate the agent.
- **The state can lag.** The records are read at most every 500 ms, and an event
  that arrives after that read shows on the next one.
- **A failed turn and an open question look alike in the sidebar.** Both say
  `Needs you`. The record separates them, and the hover text does not.
- **The statement of an adapter.** Only the Claude plugin ships with Horizon.
  The stand-in of the fixture and an adapter that a person adds send their own
  events, and Horizon cannot tell how such an adapter counted its work.
