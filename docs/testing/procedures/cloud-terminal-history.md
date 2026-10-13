---
procedure: cloud-terminal-history
feature: Scrollable history of cloud terminal panels
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [provider API key in Cloud settings]
owner: peters
---

# Cloud terminal history test procedure

## 1. Purpose

This procedure makes sure that the scrollback of a cloud shell panel and a cloud
agent panel operates as in a local panel. The history indicator, the scroll
bar, the mouse wheel and the selection must show the output of the session. The
output that the session writes while no panel is attached must come back at the
next attach, one time only.

## 2. Applicability

- Candidate: a worker image with `horizon-worker-attach`, and a Horizon build
  that attaches cloud panels through `horizon-worker-session`.
- Platforms: Linux. Provider: Hetzner. RunPod is optional.
- This procedure does not test these functions:
  - Browser panels and Device panels of a cloud. They have no terminal.
  - The parking rules. The [park procedure](cloud-park-attach.md) tests them.

## 3. Safety

> **CAUTION:** DELETE THE CLOUD OF THIS RUN AT THE END. The provider charges
> money until somebody deletes the worker and its storage.

> **CAUTION:** DO NOT PUT THE PROVIDER API KEY, THE WORKER ADDRESS OR A TOKEN
> IN A SCREENSHOT OR A LOG. Treat all evidence as public.

> **CAUTION:** USE ONLY SYNTHETIC CONTENT IN THE PANELS. Screenshots and
> recordings can show the content of each panel.

## 4. Equipment and preconditions

- A frozen Horizon candidate and its SHA-256.
- The [local device smoke fixture](../../../scripts/device-smoke/README.md),
  `scripts/device-smoke/serve.py`, with `--native-view`, and a Device panel
  that shows it.
- A provider API key in the Cloud settings of the test account.
- A synthetic repository with one commit and no `.horizon/cloud.yml`.
- A worker image with the helper. Use one of these:
  - A quick-start base image that contains `horizon-worker-attach`.
  - Before such an image is published: a quick-start worker on which you copy
    `examples/cloud-worker/horizon-worker-attach` and
    `examples/cloud-worker/horizon-worker-session` of the candidate commit to
    `/usr/local/bin` as root, with mode 755. A session that started before the
    copy keeps the tmux default of 2000 lines of history.

## 5. Setup

1. Start the fixture with the frozen candidate and a new state directory.

   Result: The fixture output shows a `vnc_address`, and the Device panel shows
   a live view of the candidate.

2. In a local shell panel of the fixture, type `seq 1 5000` and push Enter.
   Turn the mouse wheel up 10 steps on the panel.

   Result: The history indicator shows about 4.9k lines. The panel shows lines
   that are 20 lines older. This is the baseline of a local panel.

3. Open **Cloud › New cloud…**. Type the path of the synthetic repository and a
   title. Click **Quick start on the public base image**.

   Result: The dialog shows a Hetzner offer and three passed checks.

   > **CAUTION:** START ONLY ONE CLOUD FOR THIS RUN. The provider charges money
   > from the next step until the cleanup.

4. Click **Start cloud**. Wait until the cloud card shows **Ready**.

   Result: The cloud shows its first agent panel.

5. Hold Ctrl and double-click the empty part of the cloud card. Click **Shell**.

   Result: The cloud shows a shell panel with a prompt. No tmux status line
   shows at the bottom of the panel.

## 6. Tasks

### 6.1 SCROLL — Scroll the output of a cloud shell

1. In the cloud shell panel, type `seq 1 5000` and push Enter.

   Result: The history indicator shows about 4.9k lines.

2. Turn the mouse wheel up 10 steps on the panel.

   Result: The panel shows older lines. The prompt shows no recalled command.

3. Drag the thumb of the scroll bar to the top.

   Result: The first row of the panel is `1`.

4. Drag the thumb of the scroll bar to the bottom. Type `echo live` and push
   Enter.

   Result: The panel shows the end of the output again, then `live`.

### 6.2 SELECT — Select and copy the history

1. In the cloud shell panel, type this command. Then push Enter.

   ```sh
   (for i in $(seq 1 60); do echo "new output $i"; sleep 1; done) &
   ```

   Result: The panel shows a new line each second.

2. Turn the mouse wheel up until the panel shows the lines `100` to `110`.
   Drag the pointer from `100` to `110`. Copy the selection.

   Result: The selection and the shown lines stay where they are while the new
   lines come.

3. Paste the selection in the local shell panel.

   Result: The local panel shows the lines `100` to `110`.

### 6.3 DETACHED — Show output from a time with no attach

1. In the cloud shell panel, type this command. Then push Enter.

   ```sh
   clear; for i in $(seq 1 300); do echo "detached $i"; sleep 0.5; done
   ```

2. In the sidebar, click the name of a local panel. Move the canvas until no
   panel of the cloud is on the screen. Wait 2 minutes and 15 seconds.

   Result: The cloud panels park. The [park procedure](cloud-park-attach.md)
   gives the details.

3. In the sidebar, click the name of the cloud shell panel.

   Result: The panel attaches again and shows new `detached` lines.

4. Drag the thumb of the scroll bar to the top.

   Result: The first row is `detached 1`. Each number from 1 to the last line
   shows one time only.

### 6.4 ALTERNATE — Attach to a program on the alternate screen

1. In the cloud shell panel, type this command. Then push Enter.

   ```sh
   seq -f 'normal %g' 1 100; printf '\033[?1049h\033[Hfull screen'; read x; printf '\033[?1049l'
   ```

   Result: The panel shows only `full screen`.

2. Do steps 2 and 3 of task 6.3 again.

   Result: The panel attaches again and shows `full screen`.

3. Push Enter in the panel. Drag the thumb of the scroll bar to the top.

   Result: The panel shows the normal screen again, and the history starts
   before `normal 1`.

### 6.5 PASTE — Paste text in the session

1. Copy these two lines in the local shell panel: `echo one` and `echo two`.
   Paste them at the prompt of the cloud shell panel.

   Result: The prompt shows the two lines and runs nothing before you push
   Enter. The shell asked for bracketed paste.

2. Type `cat`, push Enter and paste the two lines again. Push Ctrl+D.

   Result: `cat` shows each line two times. The pasted text has no `[200~`
   marker, because `cat` did not ask for bracketed paste.

### 6.6 AGENT — Scroll the output of a cloud agent panel

1. In the cloud agent panel, let the agent run `seq 1 3000`, or use an agent
   stand-in that writes 3000 numbered lines.

   Result: The history indicator shows about 3k lines.

2. Turn the mouse wheel up on the panel. Drag the thumb of the scroll bar to
   the top.

   Result: The panel shows the older lines. The agent prompt shows no recalled
   input.

3. Make the panel narrower and wider again.

   Result: The agent draws its screen again at the new width. The history
   indicator still shows the history.

### 6.7 LOCAL — Keep the scrolling of other panels

1. In a local Claude panel and in an SSH panel of **Remote Hosts**, write more
   than one screen of output. Turn the mouse wheel up.

   Result: Each panel scrolls its history as before.

## 7. Pass criteria

- 6.1 scrolls to the first line of the output, and the mouse wheel sends no
  keys to the session.
- 6.2 selects and copies the history.
- 6.3 shows the output of a time with no attach, one time only.
- 6.4 shows the alternate screen after an attach, and the normal history after
  the program ends.
- 6.5 brackets a paste only for a program that asked for it.
- 6.6 scrolls the history of an agent panel.
- 6.7 shows no change for local and SSH panels.

## 8. Cleanup

> **CAUTION:** THIS STEP DELETES THE WORKER AND ITS STORAGE.

1. On the cloud card, open **Manage** and delete the cloud.

   Result: The card shows that the worker and the managed storage are deleted.
   The provider console or API shows no server, volume or SSH key of the run.

2. Close the Device panel. Stop the fixture with Ctrl+C in its terminal.

   Result: The fixture stops its child processes and removes its target file.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
