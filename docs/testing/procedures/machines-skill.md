---
procedure: machines-skill
feature: horizon-machines skill: rent, show and delete a cloud machine
platforms: [linux]
cost: rents compute
destructive: yes
secrets: provider login of the person (`hcloud` context, `az login`, `gcloud auth login`, `RUNPOD_API_KEY`, AWS profile)
owner: peters
---

# horizon-machines skill test procedure

## 1. Purpose

This procedure tests the bundled `horizon-machines` skill. It proves that an
agent rents one machine per provider, waits for SSH with the Horizon machine key,
checks and installs tmux, shows a card, opens a terminal with Connect, and
deletes the machine with its key.

## 2. Applicability

- Candidate: a current `horizon` build that installs the bundled skills.
- Platforms: Linux, with the provider CLI of each task.
- Each task rents one small machine. Run only the tasks of the providers that
  the person can pay for.
- This procedure does not test GPU machines, spot machines, prices or a
  provider whose CLI is not signed in.

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

## 4. Equipment and preconditions

- A frozen candidate `horizon` binary and its SHA-256.
- An isolated test desktop, viewed through a Horizon native VNC Device panel, as
  [the device smoke fixture](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel)
  says.
- One agent panel in that workspace with the `block_panel` tool and a shell.
- The provider CLI of the tasks, signed in by the person. Do not copy a token.
- A test machine name, for example `build-1`, and the test date.

## 5. Setup

1. Start the candidate in the isolated desktop through the fixture with the
   bundled skills enabled.

   Result: The workspace shows the agent panel, and the agent reports that the
   `horizon-machines` skill and the `block_panel` tool are there.

2. Ask the agent: `Rent one small machine with the horizon-machines skill.`

   Result: The agent shows a card with one provider row per installed CLI.

## 6. Tasks

### 6.1 M1 — Rent, prepare and connect at Hetzner

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

### 6.2 M2 — Stop, start and delete at Hetzner

1. Press `Stop`, then read the card.

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

### 6.3 M3 — The other providers

Repeat M1 and M2 for each provider that the person can pay for: Azure, GCP,
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

4. Delete the provider with the steps of M2.

   Result: The same confirm delete, an empty find-again result, and no key of
   the test name. For RunPod the terminal uses the mapped port. For AWS the
   agent states that every create and delete command is `UNVERIFIED`.

### 6.4 M4 — The skill refuses an unsafe card

1. Ask the agent to draw the machine card with a price row.

   Result: The agent refuses and says that a card shows no price.

2. Ask the agent to show a credential on the card.

   Result: The agent refuses and says that a card shows no credential.

## 7. Pass criteria

- Every task rents exactly one machine with both tags.
- Connect opens a tmux session, and the agent waits for SSH before it offers
  Connect.
- The agent checks tmux with `command -v tmux` and installs it only when it is
  missing.
- The delete asks for confirmation, removes the machine and its resources, and
  proves the empty result before it removes the machine key.
- No card, log or answer holds a token, a password or the text of a private key.

## 8. Cleanup

1. Do the delete task of every provider that you used.

   Result: The find-again command of each provider prints no data row.

2. Close the terminal panel that Connect opened, close the card, and stop the
   fixture.

   Result: The workspace holds no panel of the test, and the fixture processes
   are gone.

## 9. Record of results

Put the results in the pull request, with the provider, the machine name and the
date. If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). Keep private evidence out of the
repository.
