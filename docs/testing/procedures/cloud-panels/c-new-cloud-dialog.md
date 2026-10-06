---
procedure: cloud-panels-c-new-cloud-dialog
feature: Cloud panels smoke test, area C (New cloud dialog and worker picker)
platforms: [linux]
cost: rents compute   # C08 if the watch starts, C31 through D01 and D02
destructive: no
secrets: [RunPod API key in Cloud settings, Hetzner Cloud API token in Cloud settings]
owner: peters
---

# Cloud panels test procedure, area C: New cloud dialog

## 1. Purpose

This area makes sure that the **New cloud…** dialog opens from each entry point
and refuses a cloud where it must. It also audits the worker picker. The audit
examines the catalog size, the rows, the filters, the prices, the order and the
picks. It also examines the data centers and the storage choice. The last task makes sure that a worker
runs in the place that the dialog showed.

## 2. Applicability

- Candidate: a debug build of `origin/main`.
- Platforms: Linux with Xvfb. Providers: RunPod and Hetzner.
- This area does not test: the deploy of a cloud. Area D tests it. C31 uses the
  clouds of D01 and D02.
- The expected counts depend on the account, the settings and the provider
  catalog of the day. Each task tells you how to find the expected value. Record
  the expected value and the value that the dialog shows.

## 3. Safety

> **CAUTION:** DO NOT CLICK **Start cloud** IN THIS AREA. This button rents compute
> from the provider. Only C31 uses clouds, and areas D01 and D02 start them.

> **CAUTION:** DO NOT PUT THE PROVIDER KEYS IN SCREENSHOTS, RECORDINGS OR LOGS. A
> person who gets a key can rent compute on that account.

## 4. Equipment and preconditions

- The fixture of [area S](s-test-fixture.md), with a live view.
- The cloud settings of [area A](a-machine-setup.md): a RunPod key, and Hetzner
  turned on with server types and locations.
- The synthetic repository `<repo>` and its profiles from
  [area B](b-repository-configuration.md). The picker audit uses the profile
  `runpod-small` (2 vCPU, 4 GB) unless a task names another profile.
- For C09: a saved test tailnet from T01.
- For C13, C15 and C24: read access to the provider catalogs. Use the RunPod
  console and the Hetzner Cloud API.
- `xdotool` on the host, for C01.
- The notes about [the worker choice](../../../cloud-workspaces.md#choosing-a-worker),
  [Hetzner New cloud](../../../cloud-hetzner.md#new-cloud) and
  [Hetzner offers](../../../cloud-hetzner.md#offers).

## 5. Setup

1. Write the offer requirements of the picker audit to `<data-home>/smoke/offers-small.json`.

   ```json
   {"min_vcpu": 2, "min_memory_gb": 4, "storage_gb": 10, "hours": 1, "limit": 50}
   ```

   Result: The file contains no secret.

2. In the fixture terminal, save the offers of the CLI to a file.

   ```sh
   <run>/bin/cloud_deploy offers <home>/.horizon/cloud/settings.json "$(cat ~/smoke/offers-small.json)" > ~/smoke/offers-small.out.json
   ```

   Result: The file contains `offers`, `other_providers` and `comparison`.

   > **CAUTION:** KEEP THE HETZNER HEADER FILE PRIVATE. It contains the Hetzner
   > token. A person who gets the token can rent servers in the project.

3. Ask the operator to write a header file for the Hetzner API with mode `600`.

   ```text
   <run>/hetzner.header: Authorization: Bearer <Hetzner token>
   ```

   Result: The header file exists. Nobody shows its content.

4. Save the Hetzner server types and locations to the evidence.

   ```sh
   curl -fsS -H @<run>/hetzner.header https://api.hetzner.cloud/v1/server_types > <evidence>/hetzner-server-types.json
   ```

   Result: The file lists the server types, their cores, memory and prices per location.

## 6. Tasks

### 6.1 C01 — Open New cloud from the panel picker, the toolbar and the overflow menu

1. In a workspace, open the panel picker and click **Cloud**.

   Result: The New cloud dialog opens with the heading **New cloud**.

2. Click **Cancel**.

   Result: The dialog closes. No cloud starts.

3. Click **Cloud** in the menu bar, then click **New cloud…**.

   Result: The New cloud dialog opens.

4. Click **Cancel**.

   Result: The dialog closes.

5. Set the candidate window to 800 × 900 pixels.

   ```sh
   DISPLAY=<display> xdotool search --pid <child-pid> --name '^Horizon$' windowsize %1 800 900
   ```

   Result: The toolbar shows **More** instead of **Cloud**.

6. Open **More › Cloud › New cloud…**.

   Result: The New cloud dialog opens in one column.

7. Click **Cancel** and set the window back to its first size.

   Result: The toolbar shows **Cloud** again.

### 6.2 C02 — Refuse a cloud in a detached workspace

1. Make an ordinary workspace with one terminal panel.

   Result: The workspace is in the main window.

2. In the sidebar, open the context menu of this workspace and click **Open in New Window**.

   Result: The workspace opens in its own window.

3. Select the detached workspace and open **Cloud › New cloud…**.

   Result: The dialog shows `Move this workspace to the main window before creating
   a cloud`. No worker, cloud or session starts.

4. Close the dialog and move the workspace back to the main window.

   Result: The workspace is in the main window again.

5. After D01, open the context menu of the workspace that holds `smoke-a`.

   Result: **Open in New Window** is not available. Its hint says `Cloud workspaces
   stay in the main window. Use the cloud's Full screen action.`

### 6.3 C03 — Keep the title in an unsaved session and make no cloud

1. Start a second fixture with the checked-in `serve.py`, the frozen candidate and a new state directory.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <run>/bin/horizon --native-view --state <run>/fixture-unsaved
   ```

   Result: The candidate starts with an ephemeral session. The session is not saved.

2. Open a Device panel for the `vnc_address` of the second fixture.

   Result: The Device panel shows a live view.

3. In the second fixture, open **Cloud › New cloud…** and type `smoke-unsaved` in **Cloud title**.

   Result: The title shows in the field.

4. Press Enter.

   Result: The dialog shows `Open a saved session from Sessions before starting a
   cloud`. The title `smoke-unsaved` stays in the field.

5. Examine the board of the second fixture.

   Result: The board has no cloud `smoke-unsaved`.

6. Stop the second fixture with Ctrl-C and close its Device panel.

   Result: Only the first fixture continues.

### 6.4 C04 — Combine RunPod and Hetzner and show the three picks

1. Open **Cloud › New cloud…**, type `<home>/smoke/app` and click **Read .horizon/cloud.yml**.

   Result: The dialog shows `Fetching prices and stock…` and then the worker catalog.

2. Select `runpod-small` in **Profile** and click **All providers**.

   Result: The full list shows RunPod rows and Hetzner rows.

3. Examine the picks above the list.

   Result: The dialog shows the **CHEAPEST**, **BALANCED** and **MOST POWERFUL** cards.

4. Examine **BEFORE YOU START**.

   Result: The list shows `RunPod account accepted, prices are current`.

### 6.5 C05 — Show USD prices with a dated ECB rate and a run-length estimate

1. Examine the line above the picks.

   Result: The line shows `Estimated totals in USD · ECB rates dated <date> ·
   provider billing currency retained`. The date is not more than seven days old.

2. Set **Compare for** to `10 hours`.

   Result: The totals on the cards and in the summary change. The hourly prices do not change.

3. Examine **ESTIMATED COST** in the summary.

   Result: The summary shows compute per hour, the storage, the estimated run,
   **Running all month** and **Stopped**.

4. Set **Compare for** back to `1 hours`.

   Result: The totals show the value for one hour again.

### 6.6 C06 — Use the filters

1. Examine **In stock only** and **Show workers below requirements**.

   Result: **In stock only** has a check mark. **Show workers below requirements**
   has no check mark. These are the defaults.

2. Click **RunPod**, then **Hetzner**, then **All providers**.

   Result: The cards and the list show only the rows of the chosen provider.

3. Clear **In stock only**.

   Result: The list shows more rows. Some rows show `Out of stock` or `Unlisted · advisory`.

4. Type `8 vCPU` in the search field.

   Result: The list shows only rows with 8 vCPU.

5. Clear the search field and select a region in **Data center**.

   Result: The RunPod stock shows the stock for that region.

6. Close the dialog and open it again.

   Result: **In stock only** is selected again. The search field is empty.

### 6.7 C07 — Refresh the prices and block Start for a stale catalog

1. Click **Refresh** beside `Updated N s ago`.

   Result: The dialog shows `Updated N s ago · checking again…` and then new prices.

2. Do not touch the dialog for 60 seconds. Take a screenshot each 5 seconds.

   Result: Some screenshots show `checking again…`. The automatic refresh runs each 15 seconds.

3. Examine the screenshots.

   Result: No screenshot shows `Comparison incomplete`. The cards and the list do not move.

4. Record that unit tests cover the block for a catalog that is more than one hour old.

   Result: The report says that the stale block was not tested live. The text
   is `Prices are over an hour old. Refresh them before starting.`

For a detailed check of the refresh, use the
[catalog refresh procedure](../new-cloud-catalog-refresh.md).

### 6.8 C08 — Watch a sold-out worker with a price cap and stop the watch

1. Clear **In stock only** and click a row that shows `Out of stock`.

   Result: The action bar shows **Start new cloud once available**.

2. In **Data center**, select one exact data center where the worker is out of stock.

   Result: The summary names the data center and the price of the worker.

3. Select **Start new cloud once available**.

   Result: The start button shows **Start when available**.

4. Type `smoke-watch` in **Cloud title**.

   Result: **Start when available** is available.

   > **CAUTION:** THE WATCH RENTS COMPUTE WHEN STOCK RETURNS. Start the watch only
   > with the permission of the operator. Click **Stop watching** in the next step.

5. Click **Start when available**.

   Result: The summary shows `Waiting for stock in <place>` and the price limit.
   The fields are locked.

6. Click **Stop watching** at once.

   Result: The watch stops. The fields are not locked. No cloud starts.

7. If a cloud starts before step 6, write its resources in the resource ledger.

   Result: The resource ledger records the cloud. Area X deletes it.

8. Clear **Start new cloud once available**.

   Result: The start button shows **Start cloud** again.

### 6.9 C09 — List None and the saved networks in the tailnet chooser

1. Examine **Tailnet** in the New cloud dialog.

   Result: The chooser shows **None** and the name of each saved tailnet.

2. Click the name of the test tailnet.

   Result: The test tailnet is selected.

3. Click **None**.

   Result: **None** is selected. The dialog shows no auth key.

### 6.10 C10 — List the declared siblings

1. Examine the dialog for `<repo>` with the profile `runpod-build`.

   Result: The dialog shows **Siblings on this worker** with one row for `sib`.

2. Examine the row of `sib`.

   Result: The checkbox is clear. The row shows the repository, the alias, the
   profile and **Local checkout path** with `<home>/smoke/sib`.

3. Select the checkbox of `sib`.

   Result: The row shows `Checking…` and then `Ready at commit <commit>`.

4. Clear the checkbox of `sib`.

   Result: The row does not authorize the sibling. G02 selects it for `smoke-sib`.

### 6.11 C11 — Use the title, profile, revision and container disk fields

1. Leave **Cloud title** empty.

   Result: **Start cloud** is not available.

2. Type `smoke-title` in **Cloud title**.

   Result: **Start cloud** is available when every check in **BEFORE YOU START** passes.

3. Expand **More options**.

   Result: The dialog shows **Container disk**, the repository and **Committed base revision**.

4. Examine **Committed base revision**.

   Result: The field shows the newest commit of `<repo>`.

5. Change **Container disk** to a larger value.

   Result: The summary shows the new container disk. The CPU sizes that cannot hold it go away.

6. Delete the title and click **Cancel**.

   Result: The dialog closes. No cloud starts.

### 6.12 C12 — Show the full catalog size with In stock only clear

1. Open the dialog with `runpod-small`, **All providers** and **In stock only** clear.

   Result: The list line shows `Showing N of M workers`.

2. Calculate the expected number of RunPod rows.

   Result: The value is the number of CPU sizes that C13 gives for this profile.

3. Calculate the expected number of Hetzner rows.

   Result: The value is the number of rows that C15 gives for this profile.

4. Compare `M` with the sum of the two values.

   Result: `M` is the same as the sum. With `runpod-small`, no row is below
   requirements, so `N` is the same as `M`.

### 6.13 C13 — Show the RunPod CPU grid

1. Click **RunPod** with **In stock only** clear.

   Result: The list shows only RunPod CPU rows.

2. Calculate the expected grid.

   Result: RunPod CPU pods take 2, 4, 8, 16 or 32 vCPU, with 2, 4 or 8 GB for each
   vCPU. With `runpod-small`, the grid has 15 sizes.

3. Remove from the grid each size that no flavor can hold with the container disk of the profile.

   Result: You have the expected rows. See the flavor limits in
   [machine setup](../../../cloud-workspaces.md#one-time-machine-setup).

4. Compare the rows of the dialog with the expected rows.

   Result: Each expected size shows one row, for example `32 vCPU · 256 GB`. No other row shows.

### 6.14 C14 — Name only the flavor families that can hold each size

1. Examine the family names on the RunPod rows.

   Result: Each row names one or more families from the RunPod catalog.

2. Compare the family of each row with its memory for each vCPU.

   Result: 2 GB rows name Compute-Optimized. 4 GB rows name General Purpose.
   8 GB rows name Memory-Optimized.

3. Examine the `cpu_flavors` value in the settings file.

   ```sh
   jq '.cpu_flavors' <data-home>/.horizon/cloud/settings.json
   ```

   Result: You have the preferred flavors.

4. Find a row that no preferred flavor can hold.

   Result: The row names the family with the least memory for each vCPU that
   holds the size. It does not name another family.

### 6.15 C15 — Show only the configured Hetzner types and locations

1. Click **Hetzner** with **In stock only** clear.

   Result: The list shows only Hetzner rows. Each row names a server type and a location.

2. Read `server_types` and `locations` from the `hetzner` section of the settings file.

   Result: You have the allowed types and locations.

3. Calculate the expected rows from the Hetzner catalog of the setup.

   Result: For each allowed type and location, the catalog has a price there and
   the type meets the profile minimums. Each such pair is one expected row.

4. Compare the rows of the dialog with the expected rows.

   Result: Each expected pair shows one row. No other type or location shows.

5. Record that the dialog does not say which catalog types the settings exclude.

   Result: The report links [issue #1305](https://github.com/peters/horizon/issues/1305).

### 6.16 C16 — Keep the unlisted Hetzner rows under In stock only

1. With **Hetzner** and **In stock only** clear, count the rows with `Unlisted · advisory`.

   Result: You have the number of unlisted rows.

2. Select **In stock only**.

   Result: The Hetzner availability flag is advisory. All Hetzner rows must stay,
   with their `Unlisted · advisory` label.

3. Count the Hetzner rows again.

   Result: The count is the same as the number of Hetzner rows in C15. If the unlisted rows go
   away, record the known defect [issue #1302](https://github.com/peters/horizon/issues/1302).

### 6.17 C17 — Count the rows that the stock filter hides

1. Click **All providers** and clear **In stock only**.

   Result: The list shows all rows.

2. Count the rows that show `Out of stock`.

   Result: You have the expected hidden count. Add the unlisted Hetzner rows
   while issue #1302 is open.

3. Select **In stock only**.

   Result: The list line shows `Showing N of M workers`.

4. Calculate `M` minus `N`.

   Result: The value is the same as the expected hidden count.

### 6.18 C18 — Hide and then disable the workers below requirements

1. Select the profile `runpod-cpu` (4 vCPU, 8 GB) with **In stock only** clear.

   Result: The list line shows `Showing N of M workers · K below requirements hidden`.

2. Count the rows of C12 that have fewer than 4 vCPU or less than 8 GB.

   Result: The count is the same as `K`.

3. Select **Show workers below requirements**.

   Result: The list shows `K` more rows. Each new row shows **Below requirements**
   and a reason.

4. Click a row that shows **Below requirements**.

   Result: The summary does not change. You cannot select the row.

5. Clear **Show workers below requirements** and select `runpod-small` again.

   Result: The rows below requirements go away.

### 6.19 C19 — Find the expected rows with the search field

1. Type `16 vCPU` in the search field, with **All providers** and **In stock only** clear.

   Result: The list shows only rows with 16 vCPU.

2. Count the rows of C12 with 16 vCPU.

   Result: The count is the same as the number of rows in step 1.

3. Type the first location from the `hetzner` settings in the search field.

   Result: The list shows only the Hetzner rows in that location.

4. Compare the rows with the rows of C15 for that location.

   Result: The rows are the same.

5. Clear the search field.

   Result: The list shows all rows again. The search finds only text in the row
   title ([issue #1305](https://github.com/peters/horizon/issues/1305)).

### 6.20 C20 — Show EUR totals for Hetzner and USD totals for RunPod

1. Click **Hetzner**.

   Result: The line above the picks shows `Estimated totals in EUR · provider
   billing currency retained`. Hetzner prices show in euros.

2. Click **RunPod**.

   Result: The line shows `Estimated totals in USD · provider billing currency
   retained`. RunPod prices show in US dollars.

3. Click **All providers**.

   Result: The line shows `Estimated totals in USD · ECB rates dated <date>`.

### 6.21 C21 — Sort all providers by the estimated total

1. With **All providers** and **In stock only** clear, record the order of the rows.

   Result: You have the order of the dialog.

2. Read the order of the `comparison` offers in `offers-small.out.json`.

   ```sh
   jq -r '.comparison.offers[] | "\(.estimated_total_usd) \(.name)"' <data-home>/smoke/offers-small.out.json
   ```

   Result: You have the order by estimated total in USD. The cheapest offer is first.

3. Compare the two orders.

   Result: The rows of the dialog have the same order as `comparison`. If the
   dialog shows all RunPod rows before the Hetzner rows, record the known defect
   [issue #1303](https://github.com/peters/horizon/issues/1303).

### 6.22 C22 — Pick the cheapest and the most powerful worker

1. Examine `comparison.complete` in `offers-small.out.json`.

   Result: The value is `true`. If it is `false`, the dialog does not name a cheapest worker.

2. Compare the **CHEAPEST** card with the first offer in `comparison`.

   Result: The card names the same worker.

3. Find the in-stock row with the most vCPU, and then the most memory.

   Result: You have the expected most powerful worker. For RunPod CPU, this is
   often `32 vCPU · 256 GB`.

4. Compare the **MOST POWERFUL** card with the expected worker.

   Result: The card names the same worker.

### 6.23 C23 — Calculate the totals for 730 hours

1. Set **Compare for** to `730 hours`.

   Result: The totals change to the totals for one month.

2. Select a RunPod row and record its hourly price and its total.

   Result: You have the values for the next step.

3. Calculate the RunPod total.

   Result: The value is the hourly price × 730, plus the network volume for one
   month. The standard volume costs $0.07 for each GB each month.

4. Select a Hetzner row and record its hourly price, its monthly price and its total.

   Result: You have the values for the next step.

5. Calculate the Hetzner total.

   Result: The value is the smaller of the hourly price × 730 and the monthly
   cap, plus the volume and the IPv4 address.

6. Compare the calculated totals with the totals of the dialog.

   Result: The difference is less than one cent for each provider, after the ECB rate.

7. Set **Compare for** back to `1 hours`.

   Result: The totals show the value for one hour again.

### 6.24 C24 — List the data centers and disable those without storage

1. Select a RunPod CPU row and open **Data center**.

   Result: The dialog shows **Any data center** and the data centers, grouped by region.

2. Count the data centers and the data centers with **Storage unavailable**.

   Result: You have the two counts.

3. Get the RunPod data centers and their network volume support from the RunPod console.

   Result: You have the expected list. The `data_centers` setting can limit it.

4. Compare the counts with the expected list.

   Result: The dialog lists each allowed data center. A data center with no
   standard network volume shows **Storage unavailable**.

5. Click a data center with **Storage unavailable**.

   Result: You cannot select it.

### 6.25 C25 — Update the exact stock on the region chips with the size

1. Select the RunPod row `2 vCPU · 4 GB` and record the stock on each region chip.

   Result: Each chip shows how many of its data centers have this size in stock.

2. Select the RunPod row `32 vCPU · 256 GB`.

   Result: The dialog runs an exact stock check for the new size.

3. Record the stock on each region chip again.

   Result: The chips show the stock for the new size. At least one count changes,
   or the counts match the RunPod stock for both sizes.

### 6.26 C26 — Show Storage unavailable for a region without storage

1. Find a region in which no data center holds a standard network volume.

   Result: You have the region from the list of C24.

2. Examine the chip of this region.

   Result: The chip says **Storage unavailable**. If it says `none in stock`,
   record the known defect [issue #1304](https://github.com/peters/horizon/issues/1304).

### 6.27 C27 — Limit the RunPod stock to the region scope

1. Record the stock of the RunPod rows and the Hetzner rows with **Any data center**.

   Result: You have the reference values.

2. Click the chip of the region Europe.

   Result: The RunPod stock shows the stock in Europe. The Hetzner rows do not change.

3. Examine the summary.

   Result: The summary says `The workspace stays in Europe, and a stopped cloud resumes there.`

4. Click **Any data center**.

   Result: The summary says `Horizon picks a data center with stock.`

### 6.28 C28 — Change the data centers with High-performance storage

1. Select a RunPod CPU row and click **High-performance** in **STORAGE**.

   Result: The data center list changes. It shows only data centers that hold
   high-performance volumes.

2. Examine the picks and **ESTIMATED COST**.

   Result: The picks do not show. The storage price shows `Price not published`.

3. Click **Standard** in **STORAGE**.

   Result: The data center list and the picks show again.

### 6.29 C29 — Show a known stock for 32 vCPU · 256 GB

1. Select the RunPod row `32 vCPU · 256 GB`.

   Result: The summary shows `In stock` or `Out of stock`.

2. Examine the data center choices.

   Result: No chip and no summary line shows `stock unknown`. If one does, link
   [issue #1305](https://github.com/peters/horizon/issues/1305) in the report.

### 6.30 C30 — Show only RunPod GPU types for a GPU profile

1. Select the profile `runpod-gpu`.

   Result: The dialog shows `GPU workers for the runpod-gpu profile`. No Hetzner row shows.

2. Clear **In stock only** and count the GPU types.

   Result: You have the count of the dialog.

3. In the fixture terminal, get the GPU offers of the CLI with unavailable types.

   ```sh
   <run>/bin/cloud_deploy offers <home>/.horizon/cloud/settings.json '{"gpu":true,"include_unavailable":true,"limit":50}' > ~/smoke/offers-gpu.out.json
   ```

   Result: The file lists the GPU offers. `other_providers` has no Hetzner offer.

4. Compare the count of the dialog with the count of the offers.

   Result: The counts are the same. Record the value.

5. Get the GPU offers for the region Europe.

   ```sh
   <run>/bin/cloud_deploy offers <home>/.horizon/cloud/settings.json '{"gpu":true,"region":"EUROPE","limit":50}' > ~/smoke/offers-gpu-eu.out.json
   ```

   Result: Each offer names regions with that GPU in stock.

6. Click the chip of the region Europe in the dialog.

   Result: The GPU types in stock are the same as in `offers-gpu-eu.out.json`.

7. Record that the stock in the dialog does not use `min_cuda_version`.

   Result: The report says that D03 makes sure of the CUDA minimum.

### 6.31 C31 — Run the worker in the selected place

This task uses the clouds of D01 and D02. Do not start other clouds.

1. When you start `smoke-a` in D01, click one exact Hetzner location row.

   Result: The summary names the server type and the location.

2. When you start `smoke-r` in D02, select one exact data center in **Data center**.

   Result: The summary names the data center.

3. Record the place of each cloud from the summary before you start it.

   Result: The evidence has the expected place of each cloud.

4. After D01, read the location of the server of `smoke-a` from the Hetzner API.

   ```sh
   curl -fsS -H @<run>/hetzner.header https://api.hetzner.cloud/v1/servers/<server-id> | jq -r '.server.datacenter.location.name'
   ```

   Result: The location is the same as the location of the summary.

5. After D02, open the pod of `smoke-r` in the RunPod console.

   Result: The pod runs in the data center of the summary.

6. Examine the cards of `smoke-a` and `smoke-r`.

   Result: Each card names the data center or location and its region.

## 7. Pass criteria

- C01 opens the dialog from the panel picker, the toolbar and **More**.
- C02 refuses a cloud in a detached workspace. C03 refuses a cloud in an unsaved session.
- C04 to C11 show the picks, the prices, the filters, the refresh, the watch,
  the tailnet chooser, the siblings and the fields as written.
- C12 to C30 show the expected counts and values that you calculated from the
  settings and the provider catalogs. A known defect has its issue link.
- C31 shows that each worker runs in the place that the summary named.
- No cloud starts in this area except through D01 and D02, or a watch that the
  operator permitted.

## 8. Cleanup

1. Close the New cloud dialog with **Cancel**.

   Result: The dialog closes. No cloud starts.

2. Delete the offer files in `<data-home>/smoke`.

   ```sh
   rm <data-home>/smoke/offers-*.json
   ```

   Result: No offer file stays in the private home.

   > **CAUTION:** DELETE THE HETZNER HEADER FILE ONLY AFTER C31 AND AREA X. Area X
   > uses it to make sure that no server remains.

3. After area X, delete `<run>/hetzner.header`.

   Result: No file with the Hetzner token stays in `<run>`.

## 9. Record of results

Write the results in the report of the run. Record the expected value and the
observed value of each count. Use the
[report template](../../reports/TEMPLATE.md). Keep private evidence out of the
repository.
