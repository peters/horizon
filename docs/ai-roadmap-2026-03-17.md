# Horizon AI Roadmap

Reviewed on 2026-03-17.

This note turns `docs/feature-research-2026-03-15.md` into a concrete delivery
plan for AI-specific product work.

The core decision is simple:

- Horizon should not become "a terminal with a chat sidebar".
- Horizon should become a spatial control plane for AI workers operating on
  terminals, notes, and environments.
- The board is the product. AI features should make the board more legible,
  more controllable, and more reusable.

## Product Position

The strongest product story is:

- visible agent state instead of hidden background work
- board-native attention and approvals instead of scattered notifications
- explicit context packs instead of vague automatic context
- scoped memory and rules instead of opaque personalization
- replayable operational boards instead of disposable terminal sessions

That product position fits the current codebase:

- agent-aware panel kinds already exist in `crates/horizon-core/src/panel.rs`
- attention state already exists in `crates/horizon-core/src/board/attention.rs`
- runtime save and restore already exist in `crates/horizon-core/src/runtime_state.rs`
- editor and usage panels already exist as non-shell surfaces

## Delivery Principles

1. Build visibility before autonomy.
   If a user cannot see what an agent is doing, why it stopped, and what it
   wants next, the rest of the AI stack will feel unreliable.

2. Keep state explicit.
   Context packs, permission profiles, memory, and model routing should be
   inspectable and editable. Hidden magic will be harder to trust in a terminal
   product.

3. Put domain state in `horizon-core`.
   Panel state, context-pack composition, task graphs, permission profiles,
   checkpoints, and memory scopes should live in core. `horizon-ui` should
   render and dispatch UI actions.

4. Do not keep extending the largest files.
   Future work should not continue piling logic into
   `crates/horizon-core/src/panel.rs` or
   `crates/horizon-ui/src/app/panels.rs`. Split by responsibility as the roadmap
   lands.

## P1

P1 is about making AI work visible, safe, and board-native.

### P1 Foundation Track

These are prerequisites for most higher-level AI UX:

- command-aware panel state
  - current working directory
  - running vs idle
  - last exit status
  - last prompt boundary
  - pending approval / waiting for input / ready state
- durable runtime state
  - restore board geometry and active workspace
  - restore agent resume bindings
  - persist enough panel metadata for context packs and future checkpoints

This is already aligned with `docs/feature-research-2026-03-15.md`.

### P1 Product Track

#### 1. Agent Status Badges And Command-Aware Chrome

Ship:

- status chips in panel chrome
- workspace-level rollups in the sidebar
- "focus next blocked agent" and "focus next ready agent"
- clear distinction between shell panels and agent panels

Why first:

- it makes the existing `Codex` and `Claude` panel kinds legible
- it gives the rest of the roadmap a stable state model to build on

UI sketch:

```text
+------------------------------------------------------------------+
| Codex / horizon | Write | /home/peters/github/horizon | Ready    |
| Task: fix failing CI                                 Resume: Last |
+------------------------------------------------------------------+
| terminal body                                                     |
| ...                                                               |
+------------------------------------------------------------------+
```

Ownership:

- `horizon-core`
  - panel command state model
  - derived panel status summary
  - board-level queries such as "next blocked panel"
- `horizon-ui`
  - panel badges and status strip
  - sidebar rollups
  - keyboard actions for state-based navigation

#### 2. Attention Commander And Permission Profiles

Ship:

- a richer attention center, not just a passive feed
- grouped attention by workspace and severity
- first-class approval requests
- permission profiles such as `Ask`, `Write`, `Prod`, and `YOLO`
- one-click actions such as focus, approve, deny, resolve, or spawn fixer

Why first:

- Horizon already has attention plumbing and a feed overlay
- approvals are one of the highest-friction AI interactions in terminal work
- this is a direct differentiator for multi-agent boards

UI sketch:

```text
+-------------------- Attention Commander -------------------------+
| High   Codex A      Waiting for approval      [Focus] [Approve]  |
| High   Build Shell  Tests failed             [Focus] [Spawn Fix] |
| Med    Claude B     Ready for input          [Focus] [Dismiss]   |
| Workspace: release                                               |
+-----------------------------------------------------------------+
```

Ownership:

- `horizon-core`
  - structured attention rules
  - permission profile definitions
  - mapping from panel state and notifications to attention items
- `horizon-ui`
  - commander panel / overlay
  - profile picker in spawn flows and panel menus
  - badge rendering in panel chrome and sidebar

#### 3. Spatial Context Packs

Ship:

- explicit context-pack builder
- add panels, pinned excerpts, notes, git changes, and board screenshot
- preview the exact context before sending
- send context packs to an existing panel or a newly spawned agent panel

Why first:

- this is the most Horizon-native AI feature
- it uses the board as the primary context primitive instead of a file tree

UI sketch:

```text
+----------------- Context Pack ----------------------------------+
| Panels: [Codex A] [Build] [Logs]                                |
| Pins:   [pytest failure] [stack trace]                          |
| Notes:  [release runbook]                                       |
| Scope:  Workspace                                                |
| Send to: [New Codex Panel v]                 [Preview] [Send]    |
+-----------------------------------------------------------------+
```

Ownership:

- `horizon-core`
  - serializable context-pack model
  - item references for panel snapshots, pins, notes, and diffs
  - deterministic pack rendering for prompts and exports
- `horizon-ui`
  - board selection affordances
  - context-pack inspector
  - target-panel picker and send flow

#### 4. Runbook Panels

Ship:

- a markdown-driven runbook panel type
- executable blocks targeting an existing panel or a new panel
- checklist items, links, and attached context-pack references
- workspace templates combining runbooks and live terminals

Why first:

- it turns Horizon from a board of terminals into a reusable operational canvas
- it pairs naturally with context packs and permission profiles

UI sketch:

```text
+---------------------- Release Runbook ---------------------------+
| [ ] Build release image                                          |
|     `cargo build --release`                 [Run in Build Panel]  |
| [ ] Run smoke tests                                              |
|     `cargo test smoke`                     [Run in Test Panel]    |
| [ ] Ask Codex to summarize failures        [Send Context Pack]    |
+-----------------------------------------------------------------+
```

Ownership:

- `horizon-core`
  - runbook document model
  - executable block metadata
  - template serialization
- `horizon-ui`
  - runbook rendering and editing
  - action buttons and target selectors
  - template creation flow

### P1 Exit Criteria

P1 is complete when:

- agent panels visibly communicate state without opening them
- approvals and blocked work are routed through a single board-native surface
- users can assemble and inspect context packs before sending them
- runbooks can sit next to live panels and drive repeatable workflows
- board restore keeps enough state that these features survive restart

## P2

P2 is about making the board a true multi-agent workspace rather than a set of
independent agent terminals.

### 1. Agent Teams And Task Graphs

Ship:

- workspace presets such as planner / implementer / verifier / reviewer
- explicit task graph per workspace
- task handoff between panels
- role-specific panel labels and state chips

UI sketch:

```text
[Planner] ---> [Implementer] ---> [Verifier]
     |                |                 |
   plan.md         patch.diff       test summary
```

Ownership:

- `horizon-core`
  - task model
  - task-to-panel assignment
  - task state transitions and history
- `horizon-ui`
  - workspace preset creation
  - task strip or task board
  - handoff affordances

### 2. Scoped Memory And Rules

Ship:

- panel, workspace, repo, and global memory scopes
- explicit review and delete flow
- rules for repo-specific behaviors and do-not-do items
- optional memory citations in prompts and summaries

Why P2:

- memory is highly useful, but it becomes much safer once context-pack and
  permission flows already exist

Ownership:

- `horizon-core`
  - memory store and scope rules
  - merge and conflict rules
  - export into prompt assembly
- `horizon-ui`
  - memory inspector
  - rules editor
  - scope selector

### 3. Background And Remote Agent Panels

Ship:

- panels that represent long-running remote or isolated agent work
- reconnect and resume flows
- status surfaces for queued, running, failed, and completed remote jobs

Why P2:

- this is powerful, but it depends on strong state, attention, and task
  orchestration first

Ownership:

- `horizon-core`
  - remote target metadata
  - background run state
  - reconnect logic and persistence
- `horizon-ui`
  - background panel visuals
  - remote-state badges
  - follow-up and resume flows

### 4. Model Routing And Cost Governance

Ship:

- model and provider defaults per workspace
- spend and usage budgets
- route summary work to cheaper models and implementation to stronger models
- alerts when a workspace is burning budget unusually fast

Why P2:

- Horizon already has a usage panel; this extends an existing concept instead of
  inventing a separate analytics product

Ownership:

- `horizon-core`
  - routing policy model
  - budget rules
  - usage aggregation by workspace and panel
- `horizon-ui`
  - policy editor
  - usage overlays and warning banners
  - budget widgets in the existing usage view

## P3

P3 is about replay, diagnosis, and higher-trust autonomous workflows.

### 1. Checkpoints And Board Replay

Ship:

- checkpoint panel state, task state, and key context artifacts
- replay command boundaries and major board changes
- fork from an earlier checkpoint
- export incident timelines

Ownership:

- `horizon-core`
  - checkpoint format
  - replay cursor and restore logic
  - compaction and retention
- `horizon-ui`
  - timeline scrubber
  - checkpoint browser
  - compare / fork actions

### 2. Multimodal Board Diagnosis

Ship:

- let an agent inspect a board screenshot plus selected context-pack items
- ask questions like "what is blocked?", "which panel owns this failure?", or
  "which agents are duplicating work?"

Why P3:

- it becomes much more useful once P1 and P2 provide structured state and
  explicit selection

Ownership:

- `horizon-core`
  - board snapshot assembly
  - multimodal prompt packaging
- `horizon-ui`
  - board snapshot trigger
  - diagnosis panel
  - "ask about board" flow

## Suggested Module Boundaries

These are the preferred module additions if the roadmap lands.

### `horizon-core`

Add small domain modules instead of extending existing large files:

- `command_state.rs`
  - prompt boundaries, cwd, running state, last exit status
- `permission_profile.rs`
  - named approval / capability presets
- `context_pack.rs`
  - context-pack items, rendering, and serialization
- `runbook.rs`
  - runbook documents, executable blocks, and templates
- `agent_task.rs`
  - workspace task graph and task-to-panel assignment
- `memory.rs`
  - scoped memory and repo/workspace rules
- `model_routing.rs`
  - provider and budget policy
- `checkpoint.rs`
  - checkpoint and replay metadata

Extend `board/` with focused orchestration helpers:

- `board/attention.rs`
  - keep attention reconciliation here
- `board/tasks.rs`
  - board-level task helpers
- `board/checkpoints.rs`
  - checkpoint integration points
- `board/context.rs`
  - board-level context-pack queries

Keep `panel.rs` as orchestration and spawn glue only. If roadmap work touches it
heavily, split spawn, resume, and status logic into dedicated helpers first.

### `horizon-ui`

Keep UI features in focused leaf modules:

- `app/attention_center.rs`
  - richer successor to the current feed overlay
- `app/panel_badges.rs`
  - status-chip and badge rendering
- `app/context_dock.rs`
  - context-pack builder and preview
- `app/permission_ui.rs`
  - profile picker and approval controls
- `app/task_strip.rs`
  - workspace task state surface
- `app/memory_editor.rs`
  - memory and rules UI
- `app/checkpoint_browser.rs`
  - replay and checkpoint navigation
- `runbook_widget.rs`
  - runbook rendering and interaction

Keep interaction-heavy selection logic close to the renderer:

- `terminal_widget/selection.rs`
  - terminal excerpt selection and pinning
- `terminal_widget/output_actions.rs`
  - quick-open and output-derived actions

Avoid putting board-domain mutations directly inside `app/panels.rs` or
`terminal_widget/render.rs`. Emit deferred UI actions and let core own the
state transitions.

## Recommended Order

The most defensible implementation order is:

1. command-aware panel state
2. board persistence upgrades needed by AI surfaces
3. attention commander
4. permission profiles
5. spatial context packs
6. runbook panels
7. agent teams and task graph
8. scoped memory and rules
9. background / remote agent panels
10. model routing and budgets
11. checkpoints and replay
12. multimodal board diagnosis

## What To Avoid

Do not spend the AI roadmap on these first:

- a generic always-open chat sidebar
- one-shot "generate command" helpers with no board context
- hidden autonomous edits with weak approval and visibility
- terminal-tab parity work that does not reinforce the board metaphor

## Source Signals

This roadmap is based on:

- local repo analysis
- `docs/feature-research-2026-03-15.md`
- current official docs across Warp, Wave Terminal, Claude Code, Cursor,
  GitHub Copilot, VS Code, and OpenAI Codex

The common signal from those products is stable:

- subagents and task delegation are becoming normal
- rules and memory are becoming first-class
- approvals and MCP/tool access are becoming more explicit
- background execution is becoming normal
- attention routing is becoming a product surface, not just a notification

Horizon should adopt those patterns in a board-native way rather than copying
their linear UI shells.
