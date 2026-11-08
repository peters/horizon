---
procedure: cloud-spending-envelope-card
feature: Cloud settings, the spending envelope
platforms: [linux, macos]
cost: none
destructive: yes
secrets: [provider keys in Cloud settings]
owner: peters
---

# Cloud spending envelope card test procedure

## 1. Purpose

This procedure makes sure that a person approves and revokes the spending
envelope of an agent's clouds in **Cloud settings**, and that the card says
plainly when there is none.

## 2. Applicability

- Candidate: a build whose **Cloud settings** holds a **Spending envelope** card.
- Platforms: Linux and macOS.
- This procedure does not test: what an agent's rent costs, and the stop of a
  cloud when the envelope ends.

## 3. Safety

> **CAUTION:** AN APPROVED ENVELOPE LETS AN AGENT RENT COMPUTE WITHOUT A CLICK.
> Approve it only with limits the person agreed to, and revoke it when the task
> ends. No step starts a cloud here.

## 4. Equipment and preconditions

- One Horizon window.
- Cloud settings that open.

## 5. Setup

1. Open **Cloud settings**.

   Result: The **Spending envelope** card shows **None** with **RunPod**,
   **Hetzner** and **Azure** and two empty fields, **Hours** and **Total**.

## 6. Tasks

### 6.1 ENVELOPE-1 — A person approves one

1. Click **Hetzner**.
2. Type `6` in **Hours** and `2` in **Total**.

   Result: The currency beside the fields reads `EUR`, and **Approve** becomes
   enabled.

3. Click **Approve**.

   Result: The card shows **Approved**, the line `hetzner · up to 6 h · up to 2.00
   EUR`, and the account and time it was approved.

4. Read `~/.horizon/cloud/settings.json`.

   Result: It holds a `spending_envelope` section that names `hetzner`, `EUR`, 6
   hours and 2.

### 6.2 ENVELOPE-2 — A second window cannot overwrite it

1. Open a second Horizon window on the same computer, and open **Cloud settings**
   there before step 6.1.
2. In the first window, revoke the envelope and approve it again.
3. In the second window, press **Approve**.

   Result: The second window refuses with a message that the envelope was changed
   in another window, and the file keeps the approval of the first window.

### 6.3 ENVELOPE-3 — The card shows what the envelope already paid for

> **CAUTION:** THIS STEP WRITES SYNTHETIC RENTS INTO THE RECORD OF THIS COMPUTER.
> Copy `HORIZON_HOME/cloud/envelope-rents.jsonl` first, and put the copy back in
> step 8. A wrong record changes how much of the envelope Horizon allows.

1. Stop Horizon, and add two rents to `HORIZON_HOME/cloud/envelope-rents.jsonl`
   that name the `provider`, the `currency` and the `approved_at` of the
   approved envelope, with 1.5 and 0.5 hours and the amounts 0.21 and 0.13.

   Result: The file holds two lines, each one rent.

2. Start Horizon, open **Cloud settings**, and read the approved card.

   Result: The card shows `Rented so far: 2 of 6 h · 0.34 of 2.00 EUR. Left: 4 h ·
   1.66 EUR.` under the approval line.

3. Remove the record file, and open **Cloud settings** again.

   Result: The card shows the limits and the approval line, and no rented line,
   because nothing has been rented.

### 6.4 ENVELOPE-4 — The card says an approval is used up

> **CAUTION:** THIS STEP WRITES SYNTHETIC RENTS INTO THE RECORD OF THIS COMPUTER.
> Put the copy of `HORIZON_HOME/cloud/envelope-rents.jsonl` back in step 8.

1. Stop Horizon, and make the record hold rents of this approval whose hours add
   up to the `max_hours` of the envelope **and** whose amounts add up to its
   `max_total`, for example 6 h and 2.00 EUR of a 6 h and 2.00 EUR approval.

   Result: The file holds rents of this one approval that reach both bounds.

2. Start Horizon, open **Cloud settings**, and read the approved card.

   Result: The badge says **Used up** in the attention colour, the card shows
   `This approval is used up, so no agent rents anything until a person approves a
   new one.`, and the rented line reads `Rented so far: 6 of 6 h · 2.00 of 2.00
   EUR. Left: 0 h · 0.00 EUR.`

3. Change the record so that only one bound is reached, for example 6 h and 1.00
   EUR, and open **Cloud settings** again.

   Result: The badge still says **Used up**, because the first bound to reach zero
   ends the envelope, and the rented line keeps the figure that is left of the
   other bound: `Left: 0 h · 1.00 EUR.`

4. Call `cloud_create` with `operation submit` and a repository of the
   `agent_lifecycle` policy.

   Result: The create is refused, and no cloud card appears.

### 6.5 ENVELOPE-5 — A person revokes it

1. Click **Revoke**.

   Result: The card shows **None**, the fields are empty, and the file no longer
   holds a `spending_envelope` section.

2. Read every other section of the file.

   Result: Nothing else changed.

## 7. Pass criteria

- The card approves a provider, hours and a total, and writes them to the
  settings.
- The card revokes them and leaves every other setting as it was.
- A window that showed another envelope never overwrites a newer one.
- The card shows what the envelope has paid for and what is left of it, and shows
  no rented line when the record holds nothing for this approval.
- A used-up approval says so, and an agent rents nothing under it.

## 8. Cleanup

1. Put the copy of `envelope-rents.jsonl` back, or remove the file when there was
   none.

2. Leave the envelope revoked unless the person wants it.

   Result: No agent can rent on this machine, and no cloud was started.

## 9. Record of results

Put the results in the pull request.
