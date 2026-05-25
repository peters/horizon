# Agent Pair Review Queue macOS Smoke

## Setup

- Build with `cargo build`.
- Launch `target/debug/horizon` in a normal macOS desktop session.
- Use an isolated home:
  `HOME=/tmp/horizon-agent-pair-macos-smoke-home target/debug/horizon`.
- Scope screenshots or video to the launched Horizon PID if multiple Horizon windows are open.

## Primary Flow

- Create two test agent panels.
- Use a `/bin/cat` command-backed performer panel if needed to prove stdin dispatch visibly.
- Open Review Queue from the toolbar.
- Close and reopen Review Queue from the command palette.
- Link Researcher and Performer.
- Confirm both connected-agent chips show role, kind, title, workspace, and enabled focus buttons.
- Create a candidate card with long title text, long evidence text, long suspected file paths, and multiple suggested tests.
- Confirm dispatch is disabled before acceptance.
- Accept the card.
- Dispatch to the performer.
- Confirm the performer terminal receives the generated prompt and Enter.
- Fill the regression evidence packet.
- Mark the card verified.

## Persistence

- Close Horizon cleanly.
- Relaunch with the same isolated `HOME`.
- Confirm the queue state restores.
- Confirm linked-agent state restores by stable panel local id where restored panels still exist.

## Resize And Motion

- Resize the window narrow and wide.
- Confirm the Review Queue panel, role chips, card title, suspected file paths, and evidence controls do not overlap.
- If open, resize, or dispatch behavior looks motion-sensitive, capture a short video with:
  `screencapture -V 5 /tmp/horizon-agent-pair-review-queue.mov`.

## Evidence

- Capture screenshot evidence with:
  `screencapture /tmp/horizon-agent-pair-review-queue-open.png`
  and
  `screencapture /tmp/horizon-agent-pair-review-queue-verified.png`.
- Record the Horizon PID, branch, commit, and exact `HOME` path used.
