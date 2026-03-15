# Orbiterm Feature Research

Reviewed on 2026-03-15.

This note combines:

- Current official-product research across modern terminals and terminal-adjacent tools.
- A repo-specific read of Orbiterm's current architecture.
- A prioritized recommendation set for features that fit this codebase instead of generic terminal parity work.

## Orbiterm Baseline

Orbiterm already has the beginnings of a differentiated product:

- A freeform canvas with draggable and resizable terminal panels in `crates/orbiterm-app/src/app.rs`.
- Always-visible workspace clusters in `crates/orbiterm-core/src/board.rs` and `crates/orbiterm-core/src/workspace.rs`.
- Config-backed startup layouts in `crates/orbiterm-core/src/config.rs`.
- PTY-backed panels with shell, `codex`, `claude`, and generic command modes in `crates/orbiterm-core/src/panel.rs`.
- A custom terminal renderer and input layer in `crates/orbiterm-app/src/terminal_widget.rs` and `crates/orbiterm-app/src/input.rs`.
- An existing but currently unused attention model in `crates/orbiterm-core/src/attention.rs` and `crates/orbiterm-core/src/board.rs`.

That means Orbiterm should not spend its next cycle copying tabbed terminals. The better path is to turn the board into a command-aware operational workspace.

## What Current Products Suggest

Official docs across Ghostty, WezTerm, VS Code, iTerm2, Kitty, Warp, and Zellij point to a few stable patterns:

1. Command awareness is now table stakes.
   Products increasingly track prompts, commands, current working directory, command completion, and failures.

2. Terminal output is being treated as structured content.
   Quick-select, semantic history, links, file paths, and error navigation are now expected.

3. Persistence matters.
   Sessions, layouts, workspaces, and "resume where I was" workflows are a major quality multiplier.

4. Documentation and execution are converging.
   Runbooks, workflows, notebooks, and reusable command blocks are becoming first-class terminal features.

5. Automation and attention routing are differentiators.
   Triggers, alerts, and workflow actions increasingly sit on top of terminal output.

6. Replay and collaboration are emerging, but they are best built on top of persistence and metadata first.

## Recommended Features

### 1. Command-Aware Shell Integration

Priority: `P1`

Why it matters:

- This is the foundation for most of the higher-level UX now seen in modern terminals.
- Without command boundaries and cwd tracking, Orbiterm cannot reliably build search, attention, replay, or task-aware workspace features.

What to build:

- Prompt and command boundary markers.
- Current working directory tracking per panel.
- Exit status tracking for the last command.
- Running vs idle panel state.
- "Jump to last failure" and "jump to last command" actions.
- Optional sticky header showing cwd, last exit code, and running state.

Why Orbiterm fits it:

- Panels already have independent PTYs and titles.
- The canvas metaphor becomes much more useful when each panel can advertise operational state at a glance.
- Agent panels (`Codex`, `Claude`) become much more intelligible if their command/session state is visible.

Implementation shape:

- Add command-state metadata to `Panel`.
- Start with shell integration scripts and OSC-based markers rather than trying to infer prompts purely from raw output.
- Surface lightweight badges in the panel chrome in `crates/orbiterm-app/src/app.rs`.

### 2. Attention Inbox, Rules, and Workspace Badges

Priority: `P1`

Why it matters:

- Orbiterm already has an attention model but does not currently emit or display it.
- This is a high-value feature with relatively little conceptual risk because the domain model exists already.

What to build:

- Regex and state-based triggers, such as build failure, test failure, long-running command completion, or custom text matches.
- A global inbox panel listing unresolved attention items.
- Workspace and panel badges showing unresolved counts and severity.
- Click-through navigation from an attention item to the owning panel.
- "Resolve on focus" or explicit resolve actions.

Why Orbiterm fits it:

- Spatial workspaces are a natural place for attention routing.
- The board model already tracks workspace ownership and unresolved attention.

Implementation shape:

- Extend `Board::process_output()` or panel output handling so parsed output can emit `AttentionItem`s.
- Render severity chips in the toolbar and panel chrome.
- Store per-workspace open counts so users can scan the board quickly.

### 3. Saved Boards and Session Resurrection

Priority: `P1`

Why it matters:

- Layout persistence compounds the value of every other feature.
- Users will invest more in a spatial board once it reliably comes back exactly as they left it.

What to build:

- Save and restore workspaces, panel geometry, panel kind, cwd, launch command, and active workspace.
- Explicit "Save board", "Save as template", and autosave.
- Resume policy per panel: fresh, last session, or exact session if the backend supports it.
- Optional lightweight scrollback checkpointing for non-agent panels.

Why Orbiterm fits it:

- The config layer already serializes a board-shaped model.
- Panel resume behavior already exists for agent-oriented backends.

Implementation shape:

- Introduce a persisted runtime session format alongside the existing startup config.
- Keep handwritten YAML templates separate from autosaved runtime state.
- Reuse the existing `Config` concepts where it helps, but do not force runtime state into the current minimal config schema unchanged.

### 4. Runbook / Notebook Panels

Priority: `P1` to `P2`

Why it matters:

- Modern terminals are converging with lightweight documentation systems.
- Orbiterm can do better than linear notebooks because notes can sit spatially next to the exact live terminals they control.

What to build:

- A markdown or rich-text "note panel" type.
- Executable code blocks that can target a selected terminal or spawn a new panel.
- Saved workspace templates that combine notes and live terminals.
- File links, issue links, and checklist state for operational workflows.

Why Orbiterm fits it:

- The board metaphor is already better suited to runbooks than traditional tabbed terminals.
- This creates a differentiated product story: operational canvases instead of just windows with shells.

Implementation shape:

- Add a non-PTY panel kind.
- Keep the first version local and markdown-based.
- Wire command blocks into existing panel spawn and input paths.

### 5. Smart Output Actions

Priority: `P2`

Why it matters:

- Once command boundaries exist, users expect output to be navigable and reusable.
- This improves day-to-day terminal productivity without changing the core product model.

What to build:

- Search within current panel scrollback.
- Quick-open for paths, URLs, `path:line`, and known error formats.
- "Copy last command output" and "select output block".
- Optional pinned excerpts that can be kept visible on the canvas.

Why Orbiterm fits it:

- The custom renderer means Orbiterm controls its own interaction layer.
- Spatial pinning of output excerpts is a feature traditional terminals do not naturally support.

Implementation shape:

- Add structured matchers over terminal text snapshots.
- Start with keyboard-driven quick actions and later add hover affordances.

### 6. Agent-Oriented Workspace Controls

Priority: `P2`

Why it matters:

- Orbiterm already has explicit `Codex` and `Claude` panel kinds.
- Most terminals are not yet truly good at agent-heavy workflows, which is a credible place for differentiation.

What to build:

- Panel state chips for agent type, resume mode, unresolved attention, and task status.
- Bulk actions across selected panels, such as broadcast prompt, stop, or focus next needing-attention panel.
- Workspace presets like "planner + implementer + verifier".
- A lightweight activity strip showing which agent panel produced output recently.

Why Orbiterm fits it:

- The current panel model already encodes agent-oriented launches and resume rules.
- The canvas makes multi-agent orchestration much more legible than a stack of tabs.

Implementation shape:

- Start with read-only status and bulk focus/navigation.
- Add multi-select and broadcast input only after command-state and attention models are solid.

### 7. Remote Targets and Reconnectable Domains

Priority: `P2`

Why it matters:

- Users increasingly work across SSH boxes, containers, and ephemeral dev environments.
- This becomes much more valuable once persistence exists.

What to build:

- First-class remote target metadata for SSH, container, or custom command-based backends.
- Distinct visual identity for target environments.
- Reconnect and reopen flows for boards containing remote panels.

Why Orbiterm fits it:

- Remote environments map naturally to workspaces.
- Spatial grouping by environment is more useful than flat tab lists.

Implementation shape:

- Treat remote launch targets as panel templates first.
- Avoid deep protocol work early; start with command-backed targets and saved metadata.

### 8. Board Replay

Priority: `P3`

Why it matters:

- Replay is genuinely useful, but it becomes much stronger once the app knows command boundaries and has persistence primitives.

What to build:

- Time-window snapshots of panel output and board state.
- A scrubber that rewinds panel content and focus state.
- Exportable incident timeline views for a workspace.

Why Orbiterm fits it:

- Replay across a whole board is more novel than replay within a single terminal buffer.

Implementation shape:

- Record compact snapshots keyed to command boundaries and major layout changes.
- Do not attempt pixel-perfect replay first.

## What Not To Prioritize First

- Conventional tabs/splits parity.
  Orbiterm already has the more interesting spatial model.

- Inline graphics protocol support.
  Useful eventually, but lower leverage than command metadata, attention, and persistence.

- Live collaboration/session sharing as a first major investment.
  Save/restore and runbooks should come first.

## Infrastructure Note: Async PTY I/O

This is a sensible scaling investment, but it is not a user-facing feature and should not be presented as one.

Current state:

- Orbiterm currently creates one PTY reader thread per panel in `crates/orbiterm-core/src/panel.rs`.
- Output is forwarded through an `mpsc` channel and then drained on the main thread during `Board::process_output()`.

Recommendation:

- Treat "replace 50 reader threads with one readiness loop" as `P2` infrastructure work, not as a headline roadmap item.
- It becomes worth doing when profiling shows thread count, wakeups, or memory overhead are actually limiting multi-panel scale.
- If the goal is portability, prefer a selector abstraction rather than hard-coding raw `epoll`; `epoll` is fine only if Orbiterm is intentionally Linux-only for PTY internals.

Why it still matters:

- Better I/O scalability will make large boards and agent-heavy workspaces more reliable.
- It pairs well with future persistence, replay, and attention features because those all increase pressure on output handling.

Suggested framing:

- "Scalability work for dense boards" is more accurate than "cool feature".
- Build it after command metadata and attention, unless profiling shows the current thread-per-panel model is already a bottleneck.

## Suggested Build Order

1. Command-aware shell integration.
2. Attention inbox and badges.
3. Saved boards and autosave.
4. Runbook/notebook panels.
5. Smart output actions.
6. Agent workspace controls.
7. Remote targets.
8. Board replay.

## Short-Term 90 Day Plan

### Phase 1

- Add per-panel command metadata.
- Add visible state chips in panel chrome.
- Add a minimal attention center using the existing attention model.

### Phase 2

- Persist board state and autosave it.
- Add restore-on-launch and saved templates.
- Add basic smart output actions: search, quick-open, and copy last output block.

### Phase 3

- Add note/runbook panels.
- Add agent-oriented presets and bulk navigation.
- Evaluate remote-target abstractions after the persistence format settles.

## Source Notes

The recommendations above were informed by current official docs reviewed on 2026-03-15:

- Ghostty shell integration: https://ghostty.org/docs/help/shell-integration
- WezTerm shell integration: https://wezterm.org/shell-integration.html
- WezTerm Quick Select: https://wezterm.org/quickselect.html
- VS Code shell integration: https://code.visualstudio.com/docs/terminal/shell-integration
- Kitty hints: https://sw.kovidgoyal.net/kitty/kittens/hints/
- iTerm2 shell integration: https://iterm2.com/documentation-shell-integration.html
- iTerm2 semantic history: https://iterm2.com/documentation-semantic-history.html
- iTerm2 triggers: https://iterm2.com/documentation-triggers.html
- iTerm2 instant replay: https://iterm2.com/documentation-instant-replay.html
- Warp workflows: https://warp.dev/features/workflows
- Warp notebooks: https://docs.warp.dev/features/notebooks
- Warp session management: https://docs.warp.dev/terminal/getting-started/session-management
- Warp session sharing: https://docs.warp.dev/terminal/features/session-sharing
- Zellij layouts: https://zellij.dev/documentation/layouts
- Zellij session management: https://zellij.dev/documentation/session-management

## Bottom Line

The most defensible product direction is:

Build Orbiterm into a command-aware, attention-routing terminal board with persistence and runbooks.

That direction uses the product's existing spatial strengths and avoids wasting time on generic terminal parity features that stronger incumbents already cover.
