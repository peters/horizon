---
procedure: cloud-worker-cost
feature: The hourly rate and run cost of a cloud worker
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [Hetzner API token in Cloud settings, RunPod API key in Cloud settings]
owner: peters
---

# Cloud worker cost test procedure

## 1. Purpose

This procedure makes sure that a cloud card shows the hourly rate and the cost
of the current run of its worker in the currency of the provider. It also makes
sure that the run starts at the correct time, and that a reconnect, a restart,
a stop and a resume keep that start correct.

## 2. Applicability

- Candidate: a build that shows Hetzner rates in euros.
- Platforms: Linux. Providers: Hetzner and RunPod.
- Task 6.1 uses a synthetic cloud of a debug build and rents no compute.
  Tasks 6.2 to 6.6 rent compute.
- This procedure does not test these functions:
  - The total since creation of a Hetzner cloud. Hetzner reports no billing
    for each server.
  - Totals from RunPod billing. The unit tests of the billing monitor and of
    the cost module cover them.

## 3. Safety

> **CAUTION:** DELETE THE CLOUDS OF THIS RUN AT THE END. The provider charges
> money until somebody deletes the worker and its storage.

> **CAUTION:** DO NOT PUT A PROVIDER API TOKEN IN A SCREENSHOT OR A LOG. A
> person who gets the token can use the account.

> **CAUTION:** USE ONLY SYNTHETIC CONTENT AND GENERIC WORKSPACE NAMES.
> Screenshots and recordings show the sidebar and the cloud cards.

## 4. Equipment and preconditions

- A frozen debug candidate and its SHA-256.
- The [local device smoke fixture](../../../scripts/device-smoke/README.md),
  `scripts/device-smoke/serve.py`, with `--native-view`.
- For tasks 6.2 to 6.6: a Hetzner API token and a RunPod API key in the Cloud
  settings of the test account, and a synthetic repository with one commit.
- For task 6.5: an agent panel kind and `horizon-browser` from the candidate on
  the `PATH` of Horizon.

## 5. Setup

1. Write a launcher script that sets `HORIZON_CLOUD_COST_PREVIEW=32` and then
   runs the candidate with its arguments. Start the fixture with the launcher
   as `--horizon`.

   Result: The fixture prints its manifest with `vnc_address`.

2. Use `device_panel` to show the fixture desktop in a native Device panel.

   Result: The Device panel shows the fixture desktop. Its `frame_sequence`
   advances.

## 6. Tasks

### 6.1 PREVIEW: A synthetic Ready Hetzner cloud

1. Look at the sidebar.

   Result: The group **Cloud** shows the workspace **Synthetic Hetzner cost**
   with `Ready`. The header of the group shows `€0.0240/h`.

2. Click the row **Synthetic Hetzner cost**.

   Result: The canvas moves to the card **Synthetic Hetzner cost**. The card
   shows **Ready** and `Hetzner / cpu`. The header shows
   `€0.0240/h · €0.01 run`. No amount on the card shows `$`.

3. Wait 3 minutes. Look at the status line of the card.

   Result: The run time after `up` increases. The run started 32 minutes
   before the launch of Horizon.

4. Open the drawer of the card.

   Result: The tab **Machine** shows `4 vCPU · €0.0240/h` next to its name.

5. Scroll the tab **Machine** to the section **Cost**.

   Result: **Rate** shows `€0.0240/h` and the note `compute, net of VAT`.
   **This run** shows an amount in euros and the note
   `estimate since the last start`.

6. Hold the pointer on the rate.

   Result: A tooltip says that the rate is compute only, in euros net of VAT.
   It names the workspace volume, the primary IPv4 address, traffic and VAT as
   not included.

### 6.2 LIVE: A Ready Hetzner worker

> **CAUTION:** THIS TASK RENTS A HETZNER SERVER. Do the cleanup at the end.

1. Use a candidate without `HORIZON_CLOUD_COST_PREVIEW`. Deploy a Hetzner
   CPU cloud from the synthetic repository. Wait for **Ready**.

   Result: The header shows the hourly rate in euros with four decimals, and
   a run amount in euros.

2. In the Hetzner Console, find the server of the cloud and its creation time.

   Result: The hourly price of the server type in its location, without VAT,
   is the rate on the card. The run started at the creation time of the server.

### 6.3 RECONNECT: A reconnect keeps the run

1. Write down the run time on the card. On the card, click **Reconnect**.

   Result: The card shows the steps of the reconnect. Then it shows **Ready**.

2. Look at the run time and the run amount.

   Result: The run continues from the time that you wrote down. It does not
   start again from zero.

### 6.4 RESTART: A restart keeps the run

1. Close Horizon normally. Start the same candidate again.

   Result: The cloud reconnects and shows **Ready**.

2. Look at the run time.

   Result: The run continues from the creation time of the server.

### 6.5 STOP: A stop and a resume

1. On the card, click **Stop worker…** and accept.

   Result: The card shows that the worker stopped. The header shows
   `compute stopped`. The sidebar shows no hourly rate for the cloud.

2. On the card, click **Resume worker**. Wait for **Ready**.

   Result: The run starts at the creation time of the new server, not at the
   creation time of the server that the stop deleted.

3. In the agent panel of the cloud workspace, run this command.

   ```sh
   horizon-browser cloud list -o -
   ```

   Result: The entry of the cloud shows `"hourly_rate"` with the rate of the
   card and `"currency": "EUR"`.

### 6.6 RUNPOD: Rates in US dollars

> **CAUTION:** THIS TASK RENTS A RUNPOD WORKER. Do the cleanup at the end.

1. Deploy a RunPod CPU cloud in a second workspace. Wait for **Ready**.

   Result: The card shows the rate and the run in US dollars, for example
   `$0.060/h · $0.01 run`.

2. Put both clouds in the same group of the sidebar, for example **Cloud**.

   Result: The header of the group shows one sum for each currency, for example
   `€0.0240/h + $0.060/h`. Horizon does not add euros to dollars.

## 7. Pass criteria

- Each Hetzner amount and rate on the card, in the header, in the Machine tab
  and in the sidebar shows euros.
- The run of a Hetzner worker starts at the creation time of its server.
  A reconnect and a restart do not change that start.
- A stopped cloud shows no hourly rate. A resumed cloud starts a new run.
- `cloud_list` gives the currency of each hourly rate.
- RunPod amounts show US dollars. No sum adds two currencies.

## 8. Cleanup

1. Close the fixture Horizon normally and stop the fixture.

   Result: The fixture removes its private state.

2. Delete each cloud of tasks 6.2 to 6.6 with its storage.

   Result: The provider consoles show no server, pod or volume of this run.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
