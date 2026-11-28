---
procedure: sidebar-inbox
feature: The group Needs you of the sidebar and the state word of a workspace row
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Sidebar inbox test procedure

## 1. Purpose

This procedure proves that the sidebar shows the questions that the cards of the
board ask, in the group `Needs you` at the top of the window, with the buttons
that answer them. It proves that a press on a button of the group is the press on
that button of the card, that the question then leaves the group, that a press
beside the buttons shows the card, and that the line of a workspace row names the
state of its orchestrator.

## 2. Applicability

- Candidate: a Linux build that has the group `Needs you`, the word of the
  orchestrator in the row of a workspace, and the stand-in
  `scripts/device-smoke/orchestrator_standin.py` with its word `question`.
- Platforms: Linux, because the fixture needs an isolated X11 desktop.
- This procedure does not test: a real agent program. The fixture starts the
  stand-in as the orchestrator of the workspace, and the stand-in draws the card
  and reads the press.

## 3. Safety

No step rents compute, deletes data, sends a secret or changes access. The
fixture keeps its state in its own folder and touches no real Horizon home.

## 4. Equipment and preconditions

- A Linux computer with the fixture prerequisites of
  [the fixture guide](../../../scripts/device-smoke/README.md).
- A debug build of the candidate, `cargo build`, and a frozen copy of the
  `horizon` and `horizon-device` binaries in one folder `<bin>`.
- A free TCP port for the loopback VNC server, which the fixture chooses itself.
- Two new evidence folders below `/var/tmp`: `/var/tmp/sidebar-inbox` and
  `/var/tmp/sidebar-inbox-light`.
- An agent panel of your own Horizon that can call `device_panel`.

## 5. Setup

1. Build the fixture binaries and freeze them.

   ```sh
   cargo build -p horizon-ui --bin horizon
   cargo build -p horizon-device --features cli
   mkdir -p <bin>
   cp target/debug/horizon target/debug/horizon-device <bin>/
   sha256sum <bin>/horizon
   ```

   Result: The command prints the hash of the candidate. Keep it for the report.

2. Start the fixture with the dock on and the stand-in as the orchestrator of
   every workspace.

   ```sh
   HORIZON_ORCHESTRATOR_DOCK=1 \
   HORIZON_ORCHESTRATOR_PROGRAM=<repo>/scripts/device-smoke/orchestrator_standin.py \
     python3 scripts/device-smoke/serve.py --horizon <bin>/horizon --native-view \
       --state /var/tmp/sidebar-inbox --screen 2400x1100 --theme dark
   ```

   Result: The fixture prints a `vnc_address`, its `pids` and a display.

3. From your agent panel, call `device_panel` with
   `{"operation":"create","endpoint":"<vnc_address>"}` and inspect it.

   Result: The live view shows the Horizon window of the fixture, with the dock
   at its right edge.

4. Define the input helpers. `<state>/target.json` names the fixture display.

   ```sh
   state=/var/tmp/sidebar-inbox
   device() { <bin>/horizon-device --target "$state"/target.json "$@"; }
   geometry() { device doctor | jq -c .result.geometry; }
   send_type() {
     jq -Rsc --argjson g "$(geometry)" '{geometry: $g, action: {kind: "type", text: .}}' \
       | device act -
   }
   send_key() {
     jq -nc --argjson g "$(geometry)" --arg k "$1" \
       '{geometry: $g, action: {kind: "key", key: $k, modifiers: []}}' | device act -
   }
   send_click() {
     jq -nc --argjson g "$(geometry)" --argjson x "$1" --argjson y "$2" \
       '{geometry: $g, action: {kind: "click", at: {x: $x, y: $y}, button: "left"}}' \
       | device act -
   }
   shoot() { rm -f "$1"; device screenshot "$1"; }
   ```

   Result: Each helper prints `"ok":true` for its action.

5. Read one picture of the display, and note the points of the field of the
   composer, of the row `CARD` of the sidebar and of the first button of an item
   of the group.

   ```sh
   shoot <state>/points.png
   ```

   Result: The picture shows the sidebar, the canvas and the dock. The field of
   the composer stands at the foot of the dock.

## 6. Tasks

### 6.1 SI-01 — The group shows the question of a card

1. Click the field of the composer, type `question`, and press Enter.

   ```sh
   send_click <field-x> <field-y>
   send_type question
   send_key enter
   ```

   Result: The dock shows the message `question`, the row of the card `Cold
   start` and the text `I drew the card <panel> with one question.`

2. Wait until the canvas shows the card `Cold start`, then click the row `CARD
   Cold start` of the sidebar, so the canvas reveals the card.

   Result: The card `Cold start` stands on the canvas with the callout `Two ways
   cut the start time. Pick one.` and its buttons `Lazy fonts` and `New format`.

3. Read the top of the sidebar, the row of the workspace and the card.

   ```sh
   shoot <state>/si01.png
   ```

   Result:
   - The group stands at the top of the sidebar, above the heading
     `Workspaces`, with the heading `Needs you` and the count `1`.
   - One item shows the name of the workspace, the text of the callout in two
     rows, and the buttons `Lazy fonts` and `New format` on one row. The first
     button is filled with the accent colour and the second is quiet.
   - The line of the workspace row reads `Needs you`, and the tile of the row
     shows the needs-you badge.
   - The card on the canvas holds the same callout and the same buttons.

### 6.2 SI-02 — A press in the group answers the question

1. Click the button `Lazy fonts` in the group of the sidebar, not in the card.

   ```sh
   send_click <lazy-x> <lazy-y>
   ```

   Result: The sidebar draws no group and no heading `Needs you`. The line of the
   workspace row holds no word. The dock shows the notice
   `You pressed Lazy fonts on Cold start` and the text `You picked Lazy fonts`.
   The card folds to one line.

2. Read the top of the sidebar and the dock.

   ```sh
   shoot <state>/si02.png
   ```

   Result: The picture shows the frames of the step above, and the card in one
   line on the canvas.

### 6.3 SI-03 — A press beside the buttons shows the card

1. Press the field of the composer, type `question`, and press Enter again.

   ```sh
   send_click <field-x> <field-y>
   send_type question
   send_key enter
   ```

   Result: A new card asks the same question, and the group holds one item again.

2. Pan the canvas away from the card, then press the text of the item of the
   group, and not one of its buttons.

   ```sh
   send_click <item-text-x> <item-text-y>
   ```

   Result: The workspace of the card becomes the active one and the canvas
   reveals the card. The dock keeps its conversation.

### 6.4 SI-04 — The row names the state of its orchestrator

1. Press the field of the composer, type `question`, and press Enter, then read
   the row of the workspace while the turn of the stand-in runs.

   Result: The line of the row reads `Working`, and the group shows the question
   when the card arrives.

2. Read the row of the workspace four seconds after the turn ended, while the
   dock shows the conversation of that workspace.

   Result: The line of the row holds no word, because the dock showed the
   conversation.

3. Select another workspace in the sidebar, then press the field of the composer
   of that workspace, type `question`, and press Enter. Then select the first
   workspace again.

   Result: The line of the first workspace reads `Needs you` while its card asks
   a question. A turn that ends while the person looks at another workspace
   leaves the word `Done` or `Failed` in the row of its workspace, and the word
   goes away when the person selects that workspace and the dock shows the
   conversation.

### 6.5 SI-05 — More than three questions

1. Press the field of the composer, type `question`, and press Enter four times
   more, with a wait of three seconds between two turns.

   ```sh
   send_type question
   send_key enter
   ```

   Result: Four cards ask the same question.

2. Read the group.

   ```sh
   shoot <state>/si05.png
   ```

   Result: The heading holds the count of the questions, the group holds three
   items, and the line `and <N> more` follows the third item.

### 6.6 SI-06 — The answer stays after a restart

1. Answer the question of the first card, as in SI-02, and close the fixture
   Horizon normally, as
   [the fixture guide](../../../scripts/device-smoke/README.md) describes.

   Result: The fixture stops its own processes.

2. Start the fixture again on the same state and resume the saved board.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <bin>/horizon --native-view \
     --state /var/tmp/sidebar-inbox --reuse-state --resume-session
   ```

   Result: The fixture prints a new `vnc_address`. The private home of the
   earlier run is kept.

3. Inspect the new endpoint in a native Device panel, as in setup step 3, and
   read the top of the sidebar.

   Result: The group holds no question for the card that the person answered,
   and the questions of the other cards are still there.

### 6.7 SI-07 — The light theme

1. Start a second fixture with the light theme, and repeat SI-01.

   ```sh
   HORIZON_ORCHESTRATOR_DOCK=1 \
   HORIZON_ORCHESTRATOR_PROGRAM=<repo>/scripts/device-smoke/orchestrator_standin.py \
     python3 scripts/device-smoke/serve.py --horizon <bin>/horizon --native-view \
       --state /var/tmp/sidebar-inbox-light --screen 2400x1100 --theme light
   ```

   Result: The group keeps its form and its numbers in the light palette: the
   fill and the outline of an item are the needs-you colour of that theme, the
   label of the first button is the colour of the window background, and the
   two lines of the item stay readable.

2. Read the picture of the group.

   ```sh
   shoot /var/tmp/sidebar-inbox-light/si07.png
   ```

   Result: The picture shows the group and the row exactly as SI-01 describes
   them, in the light palette.

## 7. Pass criteria

- The group stands only while a card asks a question, and its heading holds the
  count of the questions.
- A press on a button of the group answers the question on the path of the card:
  the group leaves, the dock names the press, and the stand-in answers with the
  label of the button.
- A press beside the buttons shows the card of the question.
- The line of a workspace row names the state of its orchestrator, and it holds
  one word at most.
- The answer of a callout survives a restart of Horizon.
- No text of a callout draws a control character, and no line leaves the
  sidebar.

## 8. Cleanup

1. Close the exact fixture windows normally, and stop the fixtures with Ctrl+C
   in the terminal that started each one.

   Result: Each fixture prints its summary and stops its own processes.

2. Remove the evidence folders that this run made.

   ```sh
   rm -rf /var/tmp/sidebar-inbox /var/tmp/sidebar-inbox-light
   ```

   Result: The folders are gone. Keep the pictures that the report needs.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
