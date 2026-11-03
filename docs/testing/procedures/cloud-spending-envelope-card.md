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

### 6.3 ENVELOPE-3 — A person revokes it

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

## 8. Cleanup

1. Leave the envelope revoked unless the person wants it.

   Result: No agent can rent on this machine, and no cloud was started.

## 9. Record of results

Put the results in the pull request.
