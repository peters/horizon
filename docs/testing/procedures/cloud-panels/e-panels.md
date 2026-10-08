---
procedure: cloud-panels-e-panels
feature: Cloud panels smoke test, area E (panels in a cloud)
platforms: [linux]
cost: rents compute
destructive: no
secrets: [Claude API key or subscription login, Codex API key or subscription login, cloud SSH identity in the private home]
owner: peters
---

# Cloud panels test procedure, area E: panels in a cloud

## 1. Purpose

This area makes sure that the panels in a Ready cloud work. It covers the Shell,
agent, Browser, desktop viewer, Editor and Usage panels, the cloud layout, the
worker MCP servers and the SSH route.

## 2. Applicability

- Candidate: the frozen candidate from area S.
- Platforms: Linux. Cloud: `smoke-a` from D01.
- This area does not test: tailnet traffic, companions or lifecycle actions.

## 3. Safety

> **CAUTION:** DO NOT PUT AN AGENT KEY OR A LOGIN CODE IN A SCREENSHOT OR A LOG.
> A person who gets the key can use the agent account.

> **CAUTION:** USE ONLY SYNTHETIC CONTENT IN THE PANELS. Screenshots and
> recordings can show the content of each panel.

`smoke-a` continues to cost money while this area runs.

## 4. Equipment and preconditions

- The equipment in the [main procedure](../cloud-panels.md#4-equipment-and-preconditions).
- `smoke-a` shows **Ready**.
- The [cloud agent panel start procedure](../cloud-agent-panel-start.md). E02
  uses its task H01.
- The image of `smoke-a` contains Claude Code, Codex, a browser and a desktop.
- Cloud settings select Claude and Codex in **Coding agents** (task A05).
- The facts in [Cloud workspaces](../../../cloud-workspaces.md#sessions-and-lifecycle).

## 5. Setup

1. Find the card of `smoke-a` on the board.

   Result: The card shows **Ready**.

2. Record the commit of the cloud from the card in the evidence.

   Result: You have the expected commit for E01.

## 6. Tasks

### 6.1 E01 — Open a Shell panel in the shared checkout

1. Hold Ctrl and double-click an empty area inside the cloud frame of `smoke-a`.

   ```sh
   DISPLAY=<display> xdotool mousemove <x> <y> keydown ctrl click --repeat 2 --delay 80 1 keyup ctrl
   ```

   Result: The panel picker opens and shows **Add panel**. Two separate device
   clicks do not open it.

2. Click **Shell**.

   Result: A Shell panel opens in the cloud and shows a prompt.

3. In the Shell panel, show the current directory and the commit.

   ```sh
   pwd; git rev-parse HEAD
   ```

   Result: The directory is the shared checkout below `/workspace`. The commit is
   the expected commit.

4. Show the user of the shell.

   ```sh
   id -u
   ```

   Result: The output is `10001`.

### 6.2 E02 — Start a Claude Code panel that is signed in

Device `type` actions can change a typed key
([issue #1301](https://github.com/peters/horizon/issues/1301)). A key that the
form shows as saved can still be wrong. This task examines the sign-in with a
real request.

1. In `smoke-a`, do task [H01](../cloud-agent-panel-start.md#61-h01--start-a-claude-code-panel) of the cloud agent panel start procedure.

   Result: The agent replies `ready`. The panel shows no login prompt and no `401` error.

   > **CAUTION:** ONLY THE OPERATOR ENTERS THE KEY. A device action can change
   > the key, and a screenshot can show it.

2. If the panel shows a `401` error, ask the operator to enter the key again in **Cloud settings…**.

   Result: The operator saves the key. Nobody types it with a device action.

   > **CAUTION:** THIS STEP SENDS THE NEW CLAUDE API KEY TO THE WORKER OF `smoke-a`.
   > Do this step only on the test cloud `smoke-a`, with the key of the test account.

3. If the operator entered the key again, click **Reconnect cloud** on the card of `smoke-a`.

   Result: The card shows Ready. The reconnect sends the new key to the worker of `smoke-a`.

4. Open a new Claude Code panel.

   Result: The agent starts.

5. Type the request of step 5 of H01 again.

   Result: The agent replies `ready`.

### 6.3 E03 — Start a Codex panel

1. In `smoke-a`, open the panel picker as in E01.

   Result: The panel picker opens.

2. Click **Codex**.

   Result: A Codex panel opens.

3. Type a short request to the agent.

   ```text
   Reply with the word ready.
   ```

   Result: The agent replies `ready`.

4. If Codex refuses to start, examine the message in the panel.

   Result: The message gives a clear reason, for example no login or no
   key.

5. Record the result and the reason in the evidence.

   Result: The evidence shows that Codex started or gave a clear reason.

### 6.4 E04 — Load a page in a Browser panel

1. In `smoke-a`, open the panel picker as in E01.

   Result: The panel picker opens.

2. Click **Browser**.

   Result: A Browser panel opens in the cloud.

3. In the Claude Code panel from E02, ask the agent to open `https://example.com` with its browser tools.

   Result: The Browser panel shows the Example Domain page.

4. Examine the controller label of the Browser panel.

   Result: The label names the agent that controls the page.

### 6.5 E05 — Open a live desktop viewer

1. On the card of `smoke-a`, open the details with the chevron, click **Connections**
   and then click **Add desktop viewer** in **Access**.

   Result: A Device panel opens in the cloud and shows the worker desktop.

2. In the Claude Code panel, ask the agent to take a device screenshot of the worker desktop.

   Result: The agent reports a screenshot. The Device panel continues to show the desktop.

3. Click on the image in the Device panel.

   Result: The worker desktop does not change. The Device panel only shows the desktop.

### 6.6 E06 — Open the Editor and Usage panels

1. In `smoke-a`, open the panel picker as in E01.

   Result: The panel picker opens.

2. Click **Markdown**.

   Result: An Editor panel opens in the cloud.

3. Open the panel picker again as in E01.

   Result: The panel picker opens.

4. Click **Usage**.

   Result: A Usage panel opens in the cloud and shows usage data or a clear message.

### 6.7 E07 — Change the layout and the full screen view

1. On the frame of `smoke-a`, click **Rows**.

   Result: The panels of the cloud fill the frame in rows. Other clouds do not change.

2. Click **Cols**.

   Result: The panels fill the frame in columns.

3. Click **Grid**.

   Result: The panels fill the frame in a grid.

4. Click **Default**.

   Result: Each panel goes back to its own position.

5. Click **Full screen** on the frame of `smoke-a`.

   Result: The cloud fills the window.

6. Click the Shell panel.

   Result: The Shell panel has the keyboard focus.

7. Press F11.

   Result: The Shell panel fills the window.

8. Press Escape.

   Result: The full screen view of the cloud shows again.

9. Press Escape again.

   Result: The board shows the same view as before step 5.

10. Click **Grid**.

    Result: The panels fill the frame in a grid.

11. Drag the bottom-right corner of the frame to make it smaller.

    Result: The frame becomes smaller. All panels become smaller together and do
    not overlap.

### 6.8 E08 — Make sure that the agents get the worker MCP servers

1. In the Shell panel, show the MCP servers that the worker configures for agents.

   ```sh
   jq '.mcpServers | keys' /workspace/agent-mcp.json
   ```

   Result: The list contains `horizon-cloud-companions` and `horizon-local-network`.

2. Examine the same list for the browser and desktop servers.

   Result: The list contains `horizon-browser` and `horizon-device`.

3. In the Claude Code panel, type `/mcp`.

   Result: The agent lists the same servers as connected.

### 6.9 E09 — Use the SSH route with a pinned host key

1. In the fixture terminal, find the cloud ID of `smoke-a` below `<home>/.horizon/cloud`.

   Result: You have `<cloud-id>`. The directory `<home>/.horizon/cloud/<cloud-id>` exists.

2. Get the endpoint of the worker.

   ```sh
   <run>/bin/cloud_deploy endpoint <home>/.horizon/cloud/settings.json <home>/.horizon/cloud/<cloud-id>
   ```

   Result: The output is JSON with `host`, `port`, `user`, `host_key_alias`,
   `known_hosts` and `identity_file`. `known_hosts` is the path of a file, not
   the key text. The output contains no secret.

3. If the output shows `Another controller owns this cloud operation`, do step 2 again after 10 seconds.

   Result: The command gives the JSON. The candidate continues to run.

4. Connect with the values from step 2 and the pinned host key.

   ```sh
   ssh -o StrictHostKeyChecking=yes -o HostKeyAlias="<host_key_alias>" \
     -o UserKnownHostsFile="<known_hosts>" -o GlobalKnownHostsFile=/dev/null \
     -o IdentitiesOnly=yes -i "<identity_file>" -p <port> <user>@<host> true
   ```

   Result: The command stops with exit code 0. SSH shows no host key prompt.

5. Make an empty known hosts file in the private home.

   ```sh
   : > <home>/smoke/empty-known-hosts
   ```

   Result: The file exists and is empty.

6. Do step 4 again with `<home>/smoke/empty-known-hosts` as `UserKnownHostsFile`.

   Result: SSH refuses the connection because it has no host key.

## 7. Pass criteria

- The Shell panel opens at the expected commit as UID 10001.
- Claude Code replies to a request without a `401` error.
- Codex replies, or it gives a clear reason.
- The Browser panel loads the page and names its controller.
- The desktop viewer shows the worker desktop and sends no input.
- The layout buttons, the full screen view, F11, Escape and the resize corner work.
- `agent-mcp.json` lists the four worker MCP servers.
- SSH connects with the pinned host key and refuses an unknown host key.

## 8. Cleanup

1. Close the Markdown and Usage panels.

   Result: The panels close. The Shell and agent panels stay open for area T.

2. Delete the empty known hosts file of E09 step 5.

   ```sh
   rm <data-home>/smoke/empty-known-hosts
   ```

   Result: The file does not exist.

## 9. Record of results

Write the results in the report of the run. Use the
[report template](../../reports/TEMPLATE.md). Keep agent output, provider IDs
and SSH endpoints in the private evidence only.
