---
procedure: cloud-panels-o-offers
feature: Cloud panels smoke test, area O (offers and cost)
platforms: [linux]
cost: rents compute
destructive: no
secrets: [RunPod API key in Cloud settings, Hetzner Cloud API token in Cloud settings]
owner: peters
---

# Cloud panels test procedure, area O: offers and cost

## 1. Purpose

This area makes sure that the `cloud_offers` tool gives current prices on the
host and on a worker. It also makes sure that old prices give an error and that
a worker below the profile minimum cannot be selected.

## 2. Applicability

- Candidate: the frozen candidate from area S.
- Platforms: Linux. Providers: RunPod and Hetzner.
- Only O02 rents compute. It uses `smoke-a`. Do O02 after D01.
- This area does not test: the rent of an offer. `cloud_offers` only reads prices.

## 3. Safety

> **CAUTION:** DO NOT CLICK **Start cloud** IN O03. This button rents compute.

> **CAUTION:** PAUSE ONLY THE CANDIDATE CHILD IN O02. If you pause another
> process, you can stop the Horizon of the operator.

## 4. Equipment and preconditions

- The equipment in the [main procedure](../cloud-panels.md#4-equipment-and-preconditions).
- Cloud settings contain the RunPod key and the Hetzner token (area A).
- A local Claude Code panel in the candidate, outside every cloud.
- For O02: `smoke-a` shows **Ready**, and the process ID of the candidate child from task S04.
- The facts about the [worker choice](../../../cloud-workspaces.md#choosing-a-worker)
  and [Hetzner offers](../../../cloud-hetzner.md#offers).

## 5. Setup

1. Make sure that the candidate shows no open dialog.

   Result: The board shows no New cloud dialog.

## 6. Tasks

### 6.1 O01 — List offers on the host

1. In the local Claude Code panel, ask the agent to call `cloud_offers` with these requirements.

   ```json
   {"min_vcpu": 4, "hours": 10, "region": "Europe"}
   ```

   Result: The agent shows a list of offers, cheapest estimated total first.

2. Examine the RunPod offers.

   Result: Each offer has an hourly price, an estimate for 10 hours and its availability.

3. Examine `other_providers`.

   Result: It contains Hetzner offers with prices in euros.

4. Examine `comparison`.

   Result: `comparison.complete` is `true`. The offers have `estimated_total_usd`
   and a dated ECB rate.

5. Examine the age of the prices in the answer.

   Result: The prices are less than 15 minutes old.

### 6.2 O02 — List offers on a worker, and refuse old prices

1. In a Claude Code panel of `smoke-a`, ask the agent to call `cloud_offers` of the `horizon-cloud-companions` server.

   Result: The agent shows offers and their age. The age is less than 20 minutes.

2. In a worker shell of `smoke-a`, type this command. Do not press Enter yet.

   ```sh
   sleep 1500; printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}}}' '{"jsonrpc":"2.0","method":"notifications/initialized"}' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"cloud_offers","arguments":{}}}' | horizon-cloud-worker companions mcp
   ```

   Result: The worker shell shows the full command without a lost character.

3. Press Enter.

   Result: The command waits 25 minutes before the call.

   > **CAUTION:** START THE WATCHDOG BEFORE YOU PAUSE THE CANDIDATE. A paused
   > candidate does not stop idle clouds and cannot delete clouds.

4. On the host, start a watchdog that continues the candidate child after 27 minutes.

   ```sh
   (sleep 1620; kill -CONT <child-pid>) &
   ```

   Result: The watchdog runs in the background. It continues the candidate even if the run stops here.

   > **CAUTION:** PAUSE ONLY THE CANDIDATE CHILD OF THE FIXTURE. If you pause another
   > Horizon, the work of other people stops.

5. On the host, pause the candidate child.

   ```sh
   kill -STOP <child-pid>
   ```

   Result: The Device panel shows a static image. Horizon sends no new prices to the worker.

6. Wait 26 minutes.

   Result: The prices on the worker are more than 20 minutes old.

7. Let the candidate child continue.

   ```sh
   kill -CONT <child-pid>
   ```

   Result: The Device panel shows frames that advance again.

8. Examine the output in the worker shell.

   Result: The `cloud_offers` answer is an error about old prices. It contains no offer.

9. Wait 2 minutes.

   Result: Horizon sends fresh prices to the worker.

10. Ask the agent in `smoke-a` to call `cloud_offers` again.

    Result: The agent shows offers again. The age is less than 20 minutes.

### 6.3 O03 — Make sure that a worker below the minimum cannot be selected

1. Open **Cloud › New cloud…**.

   Result: The New cloud dialog opens.

2. Wait 3 seconds.

   Result: The New cloud dialog opens and does not move.

3. Select the profile `runpod-cpu` in **Profile**.

   Result: The **Machine** list shows a count of workers that the requirements hide.

4. Select **Show workers below requirements**.

   Result: The list shows the hidden workers, each with a reason.

5. Record the worker in the summary.

   Result: You have the selected worker before step 6.

6. Click a worker that is below the requirements.

   Result: The summary does not change. The worker from step 5 stays selected.

7. Click **Cancel**.

   Result: The dialog closes. No cloud starts.

## 7. Pass criteria

- The host `cloud_offers` lists RunPod offers and Hetzner in `other_providers`.
- The host comparison is complete, with a dated ECB rate.
- The worker `cloud_offers` gives current offers, and an error when its prices
  are more than 20 minutes old.
- A worker below the requirements shows a reason and cannot be selected.

## 8. Cleanup

1. Make sure that the candidate child runs.

   ```sh
   ps -o stat= -p <child-pid>
   ```

   Result: The state is not `T`.

2. Make sure that the board shows no New cloud dialog.

   Result: No dialog is open.

## 9. Record of results

Write the results in the report of the run. Use the
[report template](../../reports/TEMPLATE.md). Keep prices and account data in
the private evidence only.
