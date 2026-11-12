---
procedure: machines-skill
feature: horizon-machines skill: rent, show and delete a cloud machine
platforms: [linux]
cost: rents compute
destructive: yes
secrets: provider login of the person (`hcloud` context, `az login`, `gcloud auth login`, `RUNPOD_API_KEY`, AWS profile), and for the network tasks the OAuth client of a Tailscale network (`TS_<NET>_CLIENT_ID`, `TS_<NET>_CLIENT_SECRET`, `TS_<NET>_TAG`)
owner: peters
---

# horizon-machines skill test procedure

## 1. Purpose

This procedure tests the bundled `horizon-machines` skill. It proves that an
agent shows one overview card of every rented machine, rents one machine per
provider, waits for SSH with the Horizon machine key, checks tmux, shows a
card, opens a terminal with Connect, and deletes the machine with its key. It
also proves that an agent joins a machine to a Tailscale network of the person,
changes its record to the network name, closes port 22, and removes the device
of the network at the delete.

## 2. Applicability

- Candidate: a current `horizon` build that installs the bundled skills.
- Platforms: Linux, with the provider CLI of each task.
- Each task rents one small machine. Run only the tasks of the providers that
  the person can pay for.
- The overview task rents two more machines at one provider.
- A task of a Tailscale network needs one network whose OAuth client is stored as
  three named secrets of the test Horizon home, and one computer of the person
  that is a member of that network.
- This procedure does not test GPU machines, spot machines, prices or a
  provider whose CLI is not signed in. It does not test the `and N more` row of
  an overview with more than 32 machines.

## 3. Safety

> **CAUTION: EACH RENT COSTS MONEY UNTIL THE DELETE FINISHES.** The machine
> bills from the create to the delete, and a stopped machine still bills its
> disk, its address and its other resources. Do the delete task of the same
> provider in the same run.

> **CAUTION: THE DELETE REMOVES THE MACHINE AND ITS DISK, AND THIS CANNOT BE
> UNDONE.** Press `Delete it` only on a card that names the test machine.

> **CAUTION: USE A TEST MACHINE NAME AND A TEST PROVIDER ACCOUNT.** Do not use
> a name or a tag of a machine that another person owns. Touch no resource
> without both tags `horizon-managed=true` and `horizon-card=<name>`.

> **CAUTION: THE NETWORK TASK CLOSES PORT 22 OF THE MACHINE TO THE INTERNET.**
> The login of that machine then answers only over the network, so a broken
> network leaves no way in. Run the task on a test machine, and prove the login
> over the network before the port closes.

> **CAUTION: A TAILSCALE DEVICE COUNTS IN THE NETWORK OF THE PERSON.** The
> machine of the network task takes a name in that network, and the test removes
> the device at its end. Use a test name, and remove the device also when a step
> fails.

## 4. Equipment and preconditions

- A frozen candidate `horizon` binary and its SHA-256.
- An isolated test desktop, viewed through a Horizon native VNC Device panel, as
  [the device smoke fixture](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel)
  says.
- One agent panel in that workspace with the `block_panel` tool and a shell.
- The provider CLI of the tasks, signed in by the person. Do not copy a token.
- A provider account that holds no machine with the tag `horizon-managed=true`
  before M0.
- A test machine name, for example `build-1`, and the test date.
- For M6 and M7: one Tailscale network whose OAuth client is stored as the three
  named secrets `TS_<NET>_CLIENT_ID`, `TS_<NET>_CLIENT_SECRET` and
  `TS_<NET>_TAG` of the test home, and a computer that is a member of that
  network. Read
  [the tailscale reference](../../../assets/plugins/codex/skills/horizon-machines/references/tailscale.md)
  first.

## 5. Setup

1. Start the candidate in the isolated desktop through the fixture with the
   bundled skills enabled.

   Result: The workspace shows the agent panel, and the agent reports that the
   `horizon-machines` skill and the `block_panel` tool are there.

2. Ask the agent: `Rent one small machine with the horizon-machines skill.`

   Result: The agent shows a card with one provider row per installed CLI.

## 6. Tasks

### 6.1 M0 — The empty overview

Do this task before the first rent.

1. Ask the agent: `Show my machines.`

   Result: One card with the heading `Machines` and the subtitle
   `Nothing is rented`. The card holds a text line that says that every
   signed-in provider answered with an empty list, and the buttons
   `Rent a machine` and `Close`. The agent shows no card for one machine.

2. Press `Close`.

   Result: The overview closes. The provider card of the setup stays open.

### 6.2 M1 — Rent, prepare and connect at Hetzner

1. Before the rent, read the machine name, the region and the type on the card.

   > **CAUTION: THIS STEP RENTS ONE MACHINE AT HETZNER AND BILLS IT UNTIL THE
   > DELETE.** Continue only when the person accepts the cost, and do the
   > delete step of this task in the same run.

2. Select the Hetzner row and press `Continue` for the region and the type.

   Result: The card shows a `progress` component with a label such as
   `Waiting for SSH`.

3. Wait for the agent to report the machine card.

   Result: The badge is `Running` with tone `ok`, and the card holds `Type`,
   `Address`, `Session` and `Rented at`. The buttons are `Connect`, `Stop` and
   `Delete`.

4. Press `Connect`.

   Result: A terminal panel opens in the same workspace with the titlebar mark
   `opened by <agent>`, and its prompt is a tmux session with the machine name.
   Type `tmux ls` and press Enter: the session is in the list.

5. Ask the agent which command it used for its own connection.

   Result: The answer names `private_key_path` and `known_hosts_path`, and the
   agent states that it ran `ssh -i` with `IdentitiesOnly=yes` and
   `IdentityAgent=none` and that `command -v tmux` answered a path.

### 6.3 M2 — The overview of three machines, Connect and Refresh

> **CAUTION: THIS TASK RENTS TWO MORE MACHINES AND BILLS THEM UNTIL THE
> DELETES.** Continue only when the person accepts the cost. Do the delete
> steps of this task in the same run.

1. Rent two more machines with the steps of M1, at Hetzner with the names
   `build-2` and `build-3`, or at a second provider that the person can pay
   for.

   Result: The workspace holds three machine cards, and each machine answers
   SSH.

2. Ask the agent: `What is rented?`

   Result: One card with the heading `Machines` and the subtitle
   `3 machines at 1 provider`, or at the number of providers that you used.
   The list holds one row for each machine, with the machine name and a
   subtitle of the provider, the region and the state. The first row is
   selected. The card holds a badge only when a provider was skipped, one text
   line for each skipped provider, a text line with the time of the read, and
   the buttons `Connect`, `Open card`, `Refresh` and `Close`. The card holds no
   `Delete` button.

3. Select the row of `build-2` and press `Connect`.

   Result: A terminal panel opens with the titlebar mark `opened by <agent>`,
   and its prompt is a tmux session of `build-2`.

4. Select the row of `build-2` and press `Open card`.

   Result: The card of `build-2` shows the buttons `Connect`, `Stop` and
   `Delete`, and the overview stays open.

5. Press `Close` on the card of `build-3`, then select its row in the overview
   and press `Open card`.

   Result: The agent creates the card of `build-3` again, and the overview stays
   open.

6. Read the card of `build-3` and its machine name.

   > **CAUTION: THE NEXT STEP DELETES `build-3` AND ITS DISK.** Confirm only
   > when the card names the test machine.

7. Press `Delete`, then press `Delete it` on the card of `build-3`.

   Result: The badge is `Deleted`, and a text line tells the person to close the
   terminal that Connect opened. The overview holds one row less, because the
   agent reads the state again after the delete.

8. Press `Refresh` on the overview.

   Result: The subtitle says `2 machines at 1 provider`, or at the number of
   providers that you used, and no row of `build-3` is in the list.

9. Ask the agent to remove the machine key of `build-2` with
   `delete_machine_key`, then press `Refresh` on the overview.

   Result: The subtitle of the `build-2` row ends with `no key here`.

10. Select the row of `build-2` and press `Connect`.

    Result: A text line on the card says that no key of this machine is in this
    Horizon. No terminal opens.

11. Press `Open card` on the row of `build-2`, then delete that machine with
    the confirm and delete steps of M3.

    Result: The badge is `Deleted`, and the overview holds one row, of
    `build-1`.

### 6.4 M3 — Stop, start and delete at Hetzner

Use the card of `build-1` from M1.

1. Press `Stop` on the card of `build-1`, then read the card.

   Result: The badge is `Stopped` with tone `warn`, and the button is `Start`.

2. Press `Start`, wait, then press `Connect`.

   Result: The machine answers again and the terminal opens.

3. Press `Delete`.

   Result: The card shows the confirm document with the badge `Delete?` and the
   buttons `Delete it` and `Keep`.

4. Press `Keep`.

   Result: The machine card returns, and the machine still answers.

5. Read the confirm card and the machine name on it.

   > **CAUTION: THE NEXT STEP DELETES THE MACHINE AND ITS DISK.** Confirm only
   > when the card names the test machine and no other machine of the person.

6. Press `Delete`, then press `Delete it`.

   Result: The agent reports the delete order, the empty proof of the find-again
   command, and the removed machine key. The card shows the badge `Deleted`.

7. Ask the agent for `machine_keys`.

   Result: The answer holds no key of the test machine name.

### 6.5 M4 — The other providers

Repeat M1 and M3 for each provider that the person can pay for: Azure, GCP,
RunPod and AWS. Read the reference of the provider first.

1. Read the machine name, the region and the type on the card.

   > **CAUTION: THE NEXT STEP RENTS ONE MACHINE OF THAT PROVIDER AND BILLS IT
   > UNTIL ITS DELETE.** Rent one provider at a time, and finish its delete
   > task before the next rent.

2. Rent the provider with the steps of M1, then read the machine card.

   Result: The same card fields as M1.

3. Read the confirm card of the provider.

   > **CAUTION: THE NEXT STEP DELETES THAT MACHINE AND ITS DISK.** Confirm only
   > when the card names the test machine.

4. Delete the provider with the steps of M3.

   Result: The same confirm delete, an empty find-again result, and no key of
   the test name. For RunPod the terminal uses the mapped port. For AWS the
   agent states that every create and delete command is `UNVERIFIED`.

### 6.6 M5 — The skill refuses an unsafe card

1. Ask the agent to draw the machine card with a price row.

   Result: The agent refuses and says that a card shows no price.

2. Ask the agent to show a credential on the card.

   Result: The agent refuses and says that a card shows no credential.

### 6.7 M6 — Join a Tailscale network and close port 22

Read [the tailscale reference](../../../assets/plugins/codex/skills/horizon-machines/references/tailscale.md)
before this task. Do it on a fresh machine of M1, at Hetzner or at another
provider whose firewall this run can change.

1. Ask the agent: `Put build-1 on my home network.`

   > **CAUTION: THIS STEP RENTS ONE MACHINE AND BILLS IT UNTIL THE DELETE.**
   > Continue only when the person accepts the cost, and do the delete task M7
   > in the same run.

   Result: The card says that the machine joins the network, and it holds a
   `progress` component while the agent installs the client.

2. Read the card after the join.

   Result: The card holds the rows `Network`, `Network name` and
   `Internet SSH`. `Network name` is the name of the network, or its `100.x`
   address when MagicDNS is off. `Internet SSH` reads `open`.

3. Ask the agent: `Which network did the machine join, and how did the key reach
   it?`

   Result: The answer names the network of the three named secrets, and it says
   that the key travelled in the standard input of `ssh` to a file under `/run`
   of the machine, that `tailscale up` read it with `--auth-key=file:<path>`, and
   that the same command removed the file. The answer holds no key and no client
   secret.

4. Ask the agent to show the fact that proves the login over the network.

   Result: The agent names the `ssh` command with the machine key and the
   network name, and its exit code 0.

5. Ask the agent to close the port.

   > **CAUTION: THIS STEP CLOSES PORT 22 OF THE MACHINE TO THE INTERNET.**
   > Continue only when the proof of step 4 answered, and only on the test
   > machine.

   Result: `Internet SSH` reads `closed`, and the agent states the command of
   the provider that removed the rule.

6. Look at the Machines section of the sidebar and press the row of `build-1`.

   Result: A terminal opens on the machine over the network, with the tmux
   session `build-1`, and its public address is no longer used.

### 6.8 M7 — Delete a machine of a network

Use the machine of M6.

1. Read the card of `build-1`, and its machine name.

   > **CAUTION: THE NEXT STEP DELETES THE MACHINE AND ITS DISK.** Confirm only
   > when the card names the test machine.

2. Press `Delete`, then press `Delete it`.

   Result: The agent logs the machine out of the network, removes its device
   through the API of the network with the delete call of the reference, and
   only then deletes the machine at the provider. The badge is `Deleted`.

3. Ask the agent: `Is the device of build-1 still in my home network?`

   Result: The agent reads the devices of the network of that hostname and tag
   again, and the answer holds no row. The device list of the network holds no
   row of `build-1`.

4. Ask the agent: `What is rented?`

   Result: The overview holds no row of `build-1`.

## 7. Pass criteria

- The skill shows one overview card of every rented machine, with one row for
  each machine and no card for one machine alone.
- `Refresh` removes the row of a deleted machine, and `Connect` from the
  overview opens a terminal.
- A machine without a key shows `no key here`, and `Connect` on that row opens
  no terminal.
- Every rent uses both tags, and each task ends with the delete of its machine.
- Connect opens a tmux session, and the agent waits for SSH before it offers
  Connect.
- The agent checks tmux with `command -v tmux` and installs it only when it is
  missing. At Hetzner the check answers `/usr/bin/tmux` and no install runs.
- The delete asks for confirmation, removes the machine and its resources, and
  proves the empty result before it removes the machine key.
- The network task joins a machine with one auth key that arrives on standard
  input, and no argument, environment variable, card or log holds that key or the
  client secret of the network.
- The login over the network answers before port 22 closes, and the card shows
  `Internet SSH: closed` after the provider removed the rule.
- The delete task removes the device from the network, and the device list of
  that network holds no row of the machine name.
- No card, log or answer holds a token, a password or the text of a private key.

## 8. Cleanup

1. Do the delete task of every provider that you used.

   Result: The find-again command of each provider prints no data row.

2. Ask the agent whether the device of each machine of a network is still in
   that network.

   Result: The device list of the network, for that hostname and tag, holds no
   row.

3. Ask the agent: `Show my machines.`

   Result: The overview shows the subtitle `Nothing is rented`.

4. Close each terminal panel and each card of the test, and stop the fixture.

   Result: The workspace holds no panel of the test, and the fixture processes
   are gone.

## 9. Record of results

Put the results in the pull request, with the provider, the machine name and the
date. If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). Keep private evidence out of the
repository.
