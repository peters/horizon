---
procedure: agent-lines
feature: The lines between an agent panel and the panels that it controls
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Agent lines test procedure

## 1. Purpose

This procedure proves that Horizon draws one thin line between an agent panel
and each panel that the agent controls. It also proves the strength of the
lines, the colour of each agent, the zoom threshold and the setting that turns
the lines off.

## 2. Applicability

- Candidate: a build that has `horizon_core::AgentLink` and the canvas setting
  **Lines between an agent and its panels**.
- Platforms: Linux, because the fixture needs an isolated X11 desktop.
- This procedure does not test: a line to a Device viewer or to a card that
  Horizon wrote itself, because those panels carry no owner fact.
- This procedure also tests two agents of one kind in one workspace, because an
  opened panel saves the actor of the panel that opened it beside the mark of its
  kind. Section 6.2 names that case.
- This procedure does not test: a panel that a session of an older build saved
  without the actor of its opener. Such a panel keeps the older rule: its mark
  names one panel only when one agent panel of that kind stands in its
  workspace.

## 3. Safety

No step rents compute, deletes data, sends a secret or changes access. The
fixture keeps its state in its own folder and touches no real Horizon home.

## 4. Equipment and preconditions

- A Linux computer with the prerequisites of
  [the device smoke fixture](../../../scripts/device-smoke/README.md).
- A Horizon native VNC Device panel in the current workspace, for the live view.
- Two agent presets of different kinds in the settings, for example Claude Code
  and Codex. Two panels of one preset also work, for section 6.2.
- The value of the setting **Lines between an agent and its panels** is on, which
  is its default.

## 5. Setup

1. Start the fixture with the native view.

   `python3 scripts/device-smoke/serve.py --native-view`

   Result: the fixture prints a `vnc_address`.

2. Open the panel of the fixture in a Device panel.

   Result: `connection` says connected, `image_received` and `image_displayed`
   are true, and `frame_sequence` advances.

3. Open a Claude Code panel in a workspace.

   Result: the board shows one agent panel and no line.

4. Open a Codex panel in the same workspace, in a free place beside the Claude
   panel.

   Result: the board shows two agent panels and no line.

## 6. Tasks

### 6.1 AL-1 — A card of an agent

1. In the Claude panel, write: `Draw a card with the title Build`.

   Result: a card panel appears beside the Claude panel.

2. Look at the board.

   Result: one thin line runs from the edge of the Claude panel to the edge of
   the card. The line leaves the side of the Claude panel that faces the card,
   it is 1.5 points wide, and it carries a filled dot at the Claude end and a
   small ring at the card end. The line is drawn under both panels.

3. In the Codex panel, write: `Draw a card with the title Review`.

   Result: a second card appears, and a line runs from the Codex panel to that
   card in a second colour. The line of the Claude panel and its card is faint,
   because the pointer rests in the panel of Codex.

### 6.1b AL-1b — A card that stands beside its agent

A card that the agent draws in a workspace with the grid layout takes a place
directly right of the agent panel. The gap between the two panels is 12 to 20
points. The line of that pair is short.

1. Arrange the workspace with the grid layout.

   Result: the agent panel keeps its place.

2. In the agent panel, write: `Draw a card with the title Build`, and then
   `Draw a second card with the title Review`.

   Result: two cards appear. Card A stands directly right of the agent panel,
   and card B stands right of card A. Two lines are drawn: a short line with its
   ring at the left edge of card A, and a line to card B that passes behind
   card A.

3. Drag card B with the pointer to a free place below and to the right of
   card A. Release the pointer.

   Result: the line to card B is a smooth curve to the left edge of card B. The
   line to card A is still drawn: it keeps its dot on the agent panel and its
   ring at the left edge of card A. A manual move returns the workspace to free
   placement, so the cards then take the size of their content.

4. Click the body of the agent panel, and then click an empty area of the board.

   Result: the two lines stay in both cases.

5. Drag card A until it touches the agent panel, and then drag it 14 points away
   from the agent panel.

   Result: a card at 0 to 7 points from the agent panel carries no line, because
   the dot and the ring would merge. A card at 14 points carries a line again.

The line to card B passes behind card A in step 2 and behind no panel after
step 3. A line that passes behind another panel is hidden by that panel: the
line is drawn below every panel and above the canvas. Do not report that as a
defect, and do not expect the line to go around a panel.

### 6.2 AL-2 — The panel of a page and of a terminal

1. In the Claude panel, write: `Open a browser panel and load https://example.com`.

   Result: a browser panel appears, and a line runs from the Claude panel to it.

2. In the Claude panel, write: `Open an SSH terminal on
   build@192.0.2.10`.

   Result: an SSH terminal appears with the mark `Opened by Claude`, and a line
   runs from the Claude panel to it. The three lines of the Claude panel leave
   its side at three different places, and they do not cross each other near
   the panel.

### 6.2b AL-2b — Two agents of one kind

1. Open a second Claude Code panel in the same workspace, in a free place.

   Result: the board shows three agent panels and no new line.

2. In the first Claude panel, write: `Draw a card with the title First`. In the
   second Claude panel, write: `Draw a card with the title Second`.

   Result: two cards appear. Two lines run, and each one starts at the panel that
   asked for its card: the line of `First` starts at the first Claude panel and
   the line of `Second` starts at the second one. Both lines carry the accent of
   their own agent.

3. In each Claude panel, write: `Open an SSH terminal on build@192.0.2.11` and
   `Open an SSH terminal on build@192.0.2.12`.

   Result: two SSH terminals appear, and each line starts at the Claude panel that
   asked for its terminal, also though both terminals carry the mark
   `Opened by Claude`.

### 6.3 AL-3 — The agent in focus

1. Click the body of the Claude panel.

   Result: the two lines of the Claude panel are clear. The line of the Codex
   panel is faint.

2. Click the body of the card of Claude.

   Result: the two lines of the Claude panel stay clear, because the agent that
   controls the panel in focus is in focus.

3. Move the pointer over the Codex panel and keep it there.

   Result: the line of the Codex panel becomes clear and the lines of the
   Claude panel become faint. The pointer wins while it rests on the panel.

4. Move the pointer to an empty area of the board.

   Result: the lines of the panel in focus become clear again.

5. Click an empty area of the board.

   Result: no agent has the focus, and every line is faint.

### 6.4 AL-4 — A zoomed out canvas

1. Click the body of the Claude panel.

   Result: the two lines of the Claude panel are clear.

2. Zoom out with the wheel until the zoom of the canvas is 0.6 or less.

   Result: every line has the same middle strength. No line is clear and no
   line is faint.

3. Zoom in to 1.0.

   Result: the lines of the agent in focus are clear again.

### 6.5 AL-5 — A moved panel and a closed panel

1. Drag the card of Claude to the other side of the Claude panel.

   Result: the line follows the card while it moves. It leaves the opposite side
   of the Claude panel and the dot stays on that side.

2. Resize the Claude panel.

   Result: the line starts at the new edge of the panel.

3. Close the card of Claude.

   Result: the line to that card goes at once, and the line to the other panel
   of the Claude agent stays.

4. Close the Codex panel.

   Result: the line of the Codex panel goes, and its card has no line.

### 6.6 AL-6 — The setting

1. Open Settings, General, Canvas.

   Result: the check box **Lines between an agent and its panels** is on.

2. Clear that check box.

   Result: every line goes at once. The board draws no line while the check box
   is clear.

3. Select the check box again.

   Result: the lines come back.

## 7. Pass criteria

- A line runs from an agent panel to each card, page and terminal panel that the
  agent controls, and to no other panel. Each line starts at the panel that
  asked, also when two agent panels of one kind stand in one workspace.
- The line leaves the side of the panel that faces the other panel, with a dot at
  the agent end and a ring at the other end, at 1.5 points wide.
- Two agents of one workspace have two colours, and the lines of the agent in
  focus are clear while the other lines are faint.
- At a zoom of 0.6 or less, every line has the same middle strength.
- A line that passes behind a panel is hidden by the panel.
- A card that stands beside its agent keeps its line and its ring at the edge of
  the card, also after the person moves another card of the same agent.
- Two panels that stand closer than 8 points carry no line, and two panels that
  stand 14 points apart carry one.
- The clear check box draws no line.
- All of this holds in the dark theme and in the light theme, and after a restart
  the lines of a card appear again while the line of a page appears only after an
  agent drives that page again.

## 8. Cleanup

1. Close the panels that this procedure opened.

   Result: the board shows only the panels that existed before the procedure.

2. Stop the fixture with Ctrl+C in its terminal.

   Result: the fixture stops, and the Device panel says that its connection
   ended.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
