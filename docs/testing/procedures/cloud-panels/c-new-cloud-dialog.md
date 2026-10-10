---
procedure: cloud-panels-c-new-cloud-dialog
feature: Cloud panels smoke test, area C (New cloud dialog and worker picker)
platforms: [linux]
cost: rents compute   # C08 if the watch starts, C31 through D01 and D02
destructive: yes   # C32 deletes the clone and owner folder it made in the private home
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

- Candidate: the frozen candidate of [area S](s-test-fixture.md).
- Platforms: Linux with Xvfb. Providers: RunPod and Hetzner.
- This area does not test: the deploy of a cloud. Area D tests it. C31 uses the
  clouds of D01 and D02.
- The expected counts depend on the account, the settings and the provider
  catalog of the day. Each task tells you how to find the expected value. Record
  the expected value and the value that the dialog shows.

## 3. Safety

> **CAUTION:** DO NOT CLICK **Start cloud** IN THIS AREA. This button rents compute
> from the provider. C31 uses the clouds that D01 and D02 start.

> **CAUTION:** START THE WATCH OF C08 ONLY WITH THE PERMISSION OF THE OPERATOR.
> The watch starts a cloud and rents compute if stock returns before you stop it.

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
- For C12 to C30: the [New cloud picker procedure](../new-cloud-picker.md) and
  the local Claude Code panel of the main setup. The agent calls `cloud_offers`.
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

3. Make sure that `<run>/hetzner.header` from the main setup exists.

   Result: The file exists and has the mode `-rw-------`. Do not show its content.

   > **CAUTION:** SEND THE HETZNER TOKEN ONLY TO THE HETZNER API. The header file
   > contains the token. Do not show the file or the request headers.

4. Save the Hetzner server types and locations to the evidence.

   ```sh
   bash <run>/hetzner-list.sh server_types full > <evidence>/hetzner-server-types.jsonl
   ```

   Result: The file has one line for each server type of all pages, with its cores, memory and prices per location.

## 6. Tasks

The tasks from C04 use an open New cloud dialog for `<home>/smoke/app`. If the
dialog is closed at the start of a task, do steps 1 to 3 of C04 first.

### 6.1 C01 — Open New cloud from the panel picker, the toolbar and the overflow menu

1. In a workspace, open the panel picker.

   Result: The panel picker opens.

2. Click **Cloud**.

   Result: The New cloud dialog opens with the heading **New cloud**.

3. Click **Cancel**.

   Result: The dialog closes. No cloud starts.

4. Click **Cloud** in the menu bar.

   Result: The Cloud menu opens.

5. Click **New cloud…**.

   Result: The New cloud dialog opens.

6. Click **Cancel**.

   Result: The dialog closes.

7. Set the candidate window to 800 × 900 pixels.

   ```sh
   DISPLAY=<display> xdotool search --pid <child-pid> --name '^Horizon$' windowsize %1 800 900
   ```

   Result: The toolbar shows **More** instead of **Cloud**.

8. Open **More › Cloud › New cloud…**.

   Result: The New cloud dialog opens in one column.

9. Click **Cancel**.

   Result: The dialog closes. No cloud starts.

10. Set the window back to its first size.

    Result: The toolbar shows **Cloud** again.

### 6.2 C02 — Refuse a cloud in a detached workspace

1. Make an ordinary workspace with one terminal panel.

   Result: The workspace is in the main window.

2. In the sidebar, open the context menu of this workspace.

   Result: The context menu opens.

3. Click **Open in New Window**.

   Result: The workspace opens in its own window.

4. Select the detached workspace.

   Result: The detached workspace is the active workspace.

5. Open **Cloud › New cloud…**.

   Result: The dialog shows `Move this workspace to the main window before creating
   a cloud`. No worker, cloud or session starts.

6. Close the dialog.

   Result: The dialog closes. No cloud starts.

7. Move the workspace back to the main window.

   Result: The workspace is in the main window again.

8. After D01, open the context menu of the workspace that holds `smoke-a`.

   Result: **Open in New Window** is not available. Its hint says `Cloud workspaces
   stay in the main window. Use the cloud's Full screen action.`

### 6.3 C03 — Keep the title in an unsaved session and make no cloud

1. Start a second fixture with the checked-in `serve.py`, the frozen candidate and a new state directory.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <run>/bin/horizon --native-view --state <run>/fixture-unsaved [--tools <tools-root>]
   ```

   Use `--tools <tools-root>` only if S02 step 9 used it.

   Result: The candidate starts with an ephemeral session. The session is not saved.

2. Open a Device panel for the `vnc_address` of the second fixture.

   Result: The Device panel shows a live view.

3. In the second fixture, open **Cloud › New cloud…**.

   Result: The New cloud dialog opens.

4. Type `smoke-unsaved` in **Cloud title**.

   Result: The title shows in the field.

5. Press Enter.

   Result: The dialog shows `Open a saved session from Sessions before starting a
   cloud`. The title `smoke-unsaved` stays in the field.

6. Examine the board of the second fixture.

   Result: The board has no cloud `smoke-unsaved`.

7. Close the Device panel of the second fixture.

   Result: The Device panel closes. The second fixture continues.

8. Stop the second fixture with Ctrl-C.

   Result: Only the first fixture continues.

### 6.4 C04 — Combine RunPod and Hetzner and show the three picks

1. Open **Cloud › New cloud…**.

   Result: The New cloud dialog opens.

2. Type `<home>/smoke/app` as the repository.

   Result: The field shows the repository.

3. Click **Read .horizon/cloud.yml**.

   Result: The dialog shows `Fetching prices and stock…` and then the worker catalog.

4. Select `runpod-small` in **Profile**.

   Result: The dialog shows the workers of `runpod-small`.

5. Click **All providers**.

   Result: The full list shows RunPod rows and Hetzner rows.

6. Examine the picks above the list.

   Result: The dialog shows the **CHEAPEST**, **BALANCED** and **MOST POWERFUL** cards.

7. Examine **BEFORE YOU START**.

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

2. Click **RunPod**.

   Result: The list shows only RunPod rows.

3. Click **Hetzner**.

   Result: The list shows only Hetzner rows.

4. Click **All providers**.

   Result: The cards and the list show RunPod rows and Hetzner rows again.

5. Clear **In stock only**.

   Result: The list shows more rows. Some rows show `Out of stock` or `Unlisted · advisory`.

6. Type `8 vCPU` in the search field.

   Result: The list shows only rows with 8 vCPU.

7. Clear the search field.

   Result: The list shows all rows again.

8. Select a region in **Data center**.

   Result: The RunPod stock shows the stock for that region.

9. Close the dialog with **Cancel**.

   Result: The dialog closes. No cloud starts.

10. Open **Cloud › New cloud…** again.

    Result: **In stock only** is selected again. The search field is empty.

### 6.7 C07 — Refresh the prices and block Start for a stale catalog

1. Click **Refresh** beside `Updated N s ago`.

   Result: The dialog shows `Updated N s ago · checking again…` and then new prices.

2. Do not touch the dialog for 60 seconds. Take a screenshot each 5 seconds.

   Result: Some screenshots show `checking again…`. The automatic refresh runs each 15 seconds.

3. Examine the screenshots.

   Result: No screenshot shows `Comparison incomplete`. The cards and the list do not move.

   > **CAUTION:** KEEP A COPY OF THE RUNPOD KEY FILE AND PUT IT BACK IN STEP 10.
   > Without the real key, Horizon cannot show RunPod prices or delete RunPod clouds.

4. Copy the RunPod key file of the fixture to `<run>/runpod-key.saved` with mode `0600`.

   Result: The copy exists. Do not show its content.

5. Write the synthetic text `rpa_SMOKEINVALIDKEY` to the RunPod key file of the fixture.

   Result: The next refresh of the RunPod prices fails. The dialog keeps the last prices.

6. Click a RunPod row.

   Result: The summary shows the RunPod worker.

7. Keep the dialog open for 61 minutes.

   Result: The dialog shows the age of the prices and the reason of the failed refresh.

8. Examine the action bar.

   Result: **Start cloud** is disabled. The dialog says `Prices are over an hour old. Refresh them before starting.`

9. Click **Cancel**.

   Result: The dialog closes. No cloud starts.

10. Copy `<run>/runpod-key.saved` back to the RunPod key file.

    Result: The key file contains the real key again.

11. Delete `<run>/runpod-key.saved`.

    Result: Only the key file of the fixture contains the RunPod key.

12. Open **Cloud › New cloud…**.

    Result: The New cloud dialog opens.

13. Type `<home>/smoke/app` as the repository.

    Result: The field shows the repository.

14. Click **Read .horizon/cloud.yml**.

    Result: The dialog shows current RunPod prices. C08 uses this dialog.

For a detailed check of the refresh, use the
[catalog refresh procedure](../new-cloud-catalog-refresh.md).

### 6.8 C08 — Watch a sold-out worker with a price cap and stop the watch

1. Clear **In stock only**.

   Result: The list also shows sold-out rows.

2. Click a row that shows `Out of stock`.

   Result: The action bar shows **Start new cloud once available**.

3. In **Data center**, select one exact data center where the worker is out of stock.

   Result: The summary names the data center and the price of the worker.

4. Select **Start new cloud once available**.

   Result: The start button shows **Start when available**.

5. Type `smoke-watch` in **Cloud title**.

   Result: **Start when available** is available.

   > **CAUTION:** THE WATCH RENTS COMPUTE WHEN STOCK RETURNS. Start the watch only
   > with the permission of the operator. Click **Stop watching** in the next step.

6. Click **Start when available**.

   Result: The summary shows `Waiting for stock in <place>` and the price limit.
   The fields are locked.

7. Click **Stop watching** at once.

   Result: The watch stops. The fields are not locked. No cloud starts.

8. If a cloud starts before step 7, write its resources in the resource ledger.

   Result: The resource ledger records the cloud. Area X deletes it.

9. Clear **Start new cloud once available**.

   Result: The start button shows **Start cloud** again.

### 6.9 C09 — List None and the saved networks in the tailnet chooser

1. Open **Cloud › New cloud…**.

   Result: The New cloud dialog opens.

2. Type `<home>/smoke/app` as the repository.

   Result: The field shows the repository.

3. Click **Read .horizon/cloud.yml**.

   Result: **Profile** lists the profiles that area B committed.

4. Examine **Tailnet** in the New cloud dialog.

   Result: The chooser shows **None** and the name of each saved tailnet.

5. Click the name of the test tailnet.

   Result: The test tailnet is selected.

6. Click **None**.

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

6. Delete the title.

   Result: **Start cloud** is disabled.

7. Click **Cancel**.

   Result: The dialog closes. No cloud starts.

Tasks C12 to C30 audit the worker picker. The
[New cloud picker procedure](../new-cloud-picker.md) gives the detailed steps
for most of these tasks. This area does not repeat those steps. It adds only the
checks that compare the dialog with the settings, the provider catalogs and the
CLI of this run.

When a task tells you to do a task of the New cloud picker procedure, use these
changes:

- Use the open dialog of this area. Do not do the setup or the cleanup of that
  procedure.
- Use `runpod-small` for the profile `small`, `runpod-cpu` for `large` and
  `runpod-gpu` for `gpu`.
- Use the Hetzner server types and locations of area A for `cx33`, `cx43`,
  `cpx42`, `fsn1`, `hel1` and `nbg1`. Use the first location for `fsn1`.
- Do not use the reference counts or the named workers of that procedure. They
  apply only to its test account. Use the expected values of this area.
- Use the local Claude Code panel of the main setup to call `cloud_offers`.

### 6.12 C12 — Show the full catalog size with In stock only clear

1. Select `runpod-small` in **Profile**.

   Result: The dialog shows the workers for `runpod-small`.

2. Click **All providers**.

   Result: The list shows the rows of all providers.

3. Do task [C12](../new-cloud-picker.md#61-c12--catalog-size) of the New cloud picker procedure.

   Result: The list line shows `Showing N of M workers`.

4. Compare `M` with the number of RunPod rows of C13 plus the number of Hetzner rows of C15.

   Result: `M` is the same as the sum. With `runpod-small`, no row is below
   requirements, so `N` is the same as `M`.

### 6.13 C13 — Show the RunPod CPU grid

1. Do task [C13](../new-cloud-picker.md#62-c13--runpod-size-grid) of the New cloud picker procedure.

   Result: The list shows only RunPod CPU rows. Each row has 2, 4, 8, 16 or 32
   vCPU, with 2, 4 or 8 GB for each vCPU.

2. Calculate the expected grid.

   Result: With `runpod-small`, the grid has 15 sizes.

3. Remove from the grid each size that no flavor can hold with the container disk of the profile.

   Result: You have the expected rows. See the flavor limits in
   [machine setup](../../../cloud-workspaces.md#one-time-machine-setup).

4. Compare the rows of the dialog with the expected rows.

   Result: Each expected size shows one row, for example `32 vCPU · 256 GB`. No other row shows.

### 6.14 C14 — Name only the flavor families that can hold each size

1. Do task [C14](../new-cloud-picker.md#63-c14--runpod-cpu-flavors) of the New cloud picker procedure.

   Result: Each RunPod row names one or more families from the RunPod catalog.

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

### 6.15 C15 — Show the complete Hetzner catalog in permitted locations

1. Do task [C15](../new-cloud-picker.md#64-c15--complete-hetzner-catalog) of the New cloud picker procedure.

   Result: The list shows only Hetzner rows. Each row names a server type and a location.

2. Read `server_types` and `locations` from the `hetzner` section of the settings file.

   Result: You have the fallback type preferences and the permitted locations.

3. Calculate the expected rows from the Hetzner catalog of the setup.

   Result: Each expected pair has a current x86 type, a permitted location and a price there.
   The type fits the worker image and meets the profile requirements.
   Each pair is one expected row. `server_types` does not limit the pairs.

4. Compare the rows of the dialog with the expected rows.

   Result: Each expected pair shows one row. No location outside `locations` shows.

5. If the catalog has a compatible type outside `server_types`, find its row.

   Result: The row shows the type and a permitted location. The fallback preferences do not hide the row.

6. Read the note under the search field.

   Result: When the settings exclude locations, the note gives their count.
   The list does not show those locations. When the settings permit every
   catalog location, the dialog shows no exclusion note.
   The fallback type preferences do not add to the exclusion count.

### 6.16 C16 — Keep the unlisted Hetzner rows under In stock only

1. Do task [C16](../new-cloud-picker.md#65-c16--unlisted-hetzner-offers) of the New cloud picker procedure.

   Result: **In stock only** keeps each row that shows `Unlisted · advisory`.
   The Hetzner availability flag is advisory.

2. Compare the number of Hetzner rows with the number of Hetzner rows in C15.

   Result: The numbers are the same. If the unlisted rows go away, record the
   known defect [issue #1302](https://github.com/peters/horizon/issues/1302).

### 6.17 C17 — Count the rows that the stock filter hides

1. Do task [C17](../new-cloud-picker.md#66-c17--hidden-count-of-the-stock-filter) of the New cloud picker procedure.

   Result: The stock filter hides the sold-out RunPod rows.

2. Click **All providers**.

   Result: The list shows the rows of all providers.

3. Clear **In stock only**.

   Result: The list shows all rows.

4. Count the rows that show `Out of stock`.

   Result: You have the expected hidden count. Add the unlisted Hetzner rows
   while issue #1302 is open.

5. Select **In stock only**.

   Result: The list line shows `Showing N of M workers`.

6. Calculate `M` minus `N`.

   Result: The value is the same as the expected hidden count.

### 6.18 C18 — Hide and then disable the workers below requirements

1. Do task [C18](../new-cloud-picker.md#67-c18--workers-below-the-requirements) of the New cloud picker procedure.

   Result: You cannot select a row below requirements. Its tooltip gives the reason.

2. Select the profile `runpod-cpu` (4 vCPU, 8 GB).

   Result: The list line shows `K below requirements hidden`.

3. Clear **In stock only**.

   Result: The list line shows `Showing N of M workers · K below requirements hidden`.

4. Count the rows of C12 that have fewer than 4 vCPU or less than 8 GB.

   Result: The count is the same as `K`.

5. Select `runpod-small` again.

   Result: The rows below requirements go away.

### 6.19 C19 — Find the expected rows with the search field

1. Do task [C19](../new-cloud-picker.md#68-c19--search) of the New cloud picker procedure, and count the rows for each search.

   Result: You have the number of rows for `16 vCPU` and for the location.

2. Compare the number of rows for `16 vCPU` with the number of rows of C12 with 16 vCPU.

   Result: The numbers are the same.

3. Compare the number of rows for the location with the number of rows of C15 in that location.

   Result: The numbers are the same.

4. Search for a RunPod flavor id, a data center id, a region name, `shared` and `dedicated`.

   Result: The flavor id shows the RunPod rows that use that flavor. The
   data center id shows the rows that can run there. The region name shows the
   rows in that region. `shared` and `dedicated` show the Hetzner rows of that
   CPU kind.

### 6.20 C20 — Show EUR totals for Hetzner and USD totals for RunPod

1. Do task [C20](../new-cloud-picker.md#69-c20--currencies) of the New cloud picker procedure.

   Result: Hetzner shows totals in EUR and RunPod shows totals in USD. **All
   providers** shows `Estimated totals in USD · ECB rates dated <date>`.

### 6.21 C21 — Sort all providers by the estimated total

1. In the fixture terminal, save the offers of the CLI again, as in setup step 2.

   ```sh
   <run>/bin/cloud_deploy offers <home>/.horizon/cloud/settings.json "$(cat ~/smoke/offers-small.json)" > ~/smoke/offers-small.out.json
   ```

   Result: The file has current offers. C21 and C22 compare the dialog with it.

2. Click **Refresh** in the dialog.

   Result: The dialog shows prices of the same age as the file.

3. Do task [C21](../new-cloud-picker.md#610-c21--order-by-estimated-total) of the New cloud picker procedure.

   Result: The rows have the order of their estimated totals. The MCP
   `comparison` has the same order.

4. Read the order of the `comparison` offers in `offers-small.out.json`.

   ```sh
   jq -r '.comparison.offers[] | "\(.estimated_total_usd) \(.name)"' <data-home>/smoke/offers-small.out.json
   ```

   Result: You have the order by estimated total in USD. The cheapest offer is first.

5. Clear **In stock only**.

   Result: The list shows all rows.

6. Compare the order of the rows with the order from step 4.

   Result: The rows of the dialog have the same order as `comparison`. If the
   dialog shows all RunPod rows before the Hetzner rows, record the known defect
   [issue #1303](https://github.com/peters/horizon/issues/1303).

### 6.22 C22 — Pick the cheapest and the most powerful worker

1. Do task [C22](../new-cloud-picker.md#611-c22--picks) of the New cloud picker procedure.

   Result: The **Cheapest** card names the first offer of the MCP `comparison`.

2. Examine `comparison.complete` in `offers-small.out.json`.

   Result: The value is `true`. If it is `false`, the dialog does not name a cheapest worker.

3. Compare the **CHEAPEST** card with the first offer in `comparison`.

   Result: The card names the same worker.

4. Find the in-stock row that has the most vCPU, with the most memory to decide a tie.

   Result: You have the expected most powerful worker. For RunPod CPU, this is
   often `32 vCPU · 256 GB`.

5. Compare the **MOST POWERFUL** card with the expected worker.

   Result: The card names the same worker.

### 6.23 C23 — Calculate the totals for 730 hours

1. Do steps 1 to 5 of task [C23](../new-cloud-picker.md#612-c23--totals-for-730-hours) of the New cloud picker procedure.

   Result: The Hetzner total is not more than the capped charges of each UTC
   calendar month in the run.

2. Click **RunPod**.

   Result: The cards show totals for 730 hours in US dollars.

3. Select a RunPod row.

   Result: The summary shows the RunPod worker.

4. Record its hourly price and its total.

   Result: You have the values for the next step.

5. Calculate the RunPod total.

   Result: The value is the hourly price × 730, plus the network volume and the
   container disk for one month.

   Note: The standard network volume costs $0.07 and the container disk costs
   $0.10 for each GB each month.

6. Compare the calculated total with the total of the dialog.

   Result: The difference is less than one cent.

7. Do steps 6 and 7 of task C23 of the New cloud picker procedure.

   Result: The list shows the rows of all providers. The totals show the value
   for one hour again.

### 6.24 C24 — List the data centers and disable those without storage

1. Do task [C24](../new-cloud-picker.md#613-c24--data-centers) of the New cloud picker procedure.

   Result: You have the number of data centers and the number with
   **Storage unavailable**.

2. Get the RunPod data centers and their network volume support from the RunPod console.

   Result: You have the expected list. The `data_centers` setting can limit it.

3. Compare the counts with the expected list.

   Result: The dialog lists each allowed data center. A data center with no
   standard network volume shows **Storage unavailable**.

4. Click a data center with **Storage unavailable**.

   Result: You cannot select it.

### 6.25 C25 — Update the exact stock on the region chips with the size

1. Select the RunPod row `2 vCPU · 4 GB`.

   Result: The summary shows `2 vCPU · 4 GB`.

2. Do task [C25](../new-cloud-picker.md#614-c25--exact-stock-on-the-region-chips) of the New cloud picker procedure. Use the row `32 vCPU · 256 GB`.

   Result: The chips show the stock for the new size. At least one count
   changes, or the counts match the RunPod stock for both sizes.

### 6.26 C26 — Show Storage unavailable for a region without storage

1. Find a region in which no data center holds a standard network volume.

   Result: You have the region from the list of C24.

2. Do task [C26](../new-cloud-picker.md#615-c26--region-without-storage) of the New cloud picker procedure. Use this region for **Oceania**.

   Result: The chip of the region says **Storage unavailable** and is disabled.
   It does not say `none in stock`.

### 6.27 C27 — Limit the RunPod stock to the region scope

1. Do task [C27](../new-cloud-picker.md#616-c27--region-scope) of the New cloud picker procedure.

   Result: The RunPod stock shows the stock in Europe. The Hetzner rows do not change.

2. Click the chip of the region Europe again.

   Result: The summary says `The workspace stays in Europe, and a stopped cloud resumes there.`

3. Click **Any data center**.

   Result: The summary says `Horizon picks a data center with stock.`

### 6.28 C28 — Change the data centers with High-performance storage

1. Select a RunPod CPU row.

   Result: The summary shows the RunPod worker and its storage.

2. Do task [C28](../new-cloud-picker.md#617-c28--high-performance-storage) of the New cloud picker procedure.

   Result: With **High-performance**, the picks do not show and the storage
   price shows `Price not published`. With **Standard**, the picks show again.

### 6.29 C29 — Show a known stock for 32 vCPU · 256 GB

1. Do task [C29](../new-cloud-picker.md#618-c29--stock-of-the-largest-size) of the New cloud picker procedure.

   Result: The summary shows `In stock`, `Low stock` or `Out of stock`.

2. Examine the data center choices.

   Result: No chip and no summary line shows `stock unknown`. If one does, record
   a defect.

### 6.30 C30 — Show only RunPod GPU types for a GPU profile

1. Do task [C30](../new-cloud-picker.md#619-c30--gpu-workers) of the New cloud picker procedure.

   Result: The dialog shows `GPU workers for the runpod-gpu profile`. No Hetzner row shows.

2. Clear **In stock only**.

   Result: The list also shows sold-out GPU types.

3. Count the GPU types.

   Result: You have the count of the dialog.

4. In the fixture terminal, get the GPU offers of the CLI with unavailable types.

   ```sh
   <run>/bin/cloud_deploy offers <home>/.horizon/cloud/settings.json '{"gpu":true,"include_unavailable":true,"limit":50}' > ~/smoke/offers-gpu.out.json
   ```

   Result: The file lists the GPU offers. `other_providers` has no Hetzner offer.

5. Compare the count of the dialog with the count of the offers.

   Result: The counts are the same. Record the value.

6. Record that the stock in the dialog does not use `min_cuda_version`.

   Result: The report says that D03 makes sure of the CUDA minimum.

### 6.31 C31 — Run the worker in the selected place

This task uses the clouds of D01 and D02. Do not start other clouds.

1. When you start `smoke-a` in D01, click one exact Hetzner location row.

   Result: The summary names the server type and the location.

2. When you start `smoke-r` in D02, select one exact data center in **Data center**.

   Result: The summary names the data center.

3. Record the place of each cloud from the summary before you start it.

   Result: The evidence has the expected place of each cloud.

> **CAUTION:** SEND THE HETZNER TOKEN ONLY TO THE HETZNER API. The header file
> contains the token. Do not show the file or the request headers.

4. After D01, read the location of the server of `smoke-a` from the Hetzner API.

   ```sh
   curl -fsS -H @<run>/hetzner.header https://api.hetzner.cloud/v1/servers/<server-id> | jq -r '.server.datacenter.location.name'
   ```

   Result: The location is the same as the location of the summary.

5. After D02, open the pod of `smoke-r` in the RunPod console.

   Result: The pod runs in the data center of the summary.

6. Examine the cards of `smoke-a` and `smoke-r`.

   Result: Each card names the data center or location and its region.

### 6.32 C32 — Clone a pasted link into its owner's folder and reuse an earlier checkout

This task clones a small public repository on this computer. It starts no cloud.
`<folder>` is the first of `github`, `code`, `src`, `projects` and `dev` in
`<data-home>` that exists, else `Horizon`.

1. Close the New cloud dialog if it is open. Open New cloud again and type
   `https://github.com/octocat/Hello-World` in the repository field.

   Result: The dialog shows **CLONE INTO** `<home>/<folder>/octocat/Hello-World`.

2. Click **Continue**.

   Result: The clone finishes. `git -C <data-home>/<folder>/octocat/Hello-World
   remote get-url origin` shows `https://github.com/octocat/Hello-World.git`.
   While it runs, the line under the step says what the clone received out of
   about the repository's size and how long it has run, and counts up each second.

3. Close the dialog with **Cancel**. Move the clone to the place where an earlier
   Horizon cloned it:

   ```sh
   mv <data-home>/<folder>/octocat/Hello-World <data-home>/<folder>/Hello-World
   ```

   Result: `<data-home>/<folder>/octocat` holds only the hidden
   `.horizon-clone-claims` folder, where a clone keeps the lock of its folder.

4. Open New cloud and type `https://github.com/octocat/Hello-World` again.

   Result: The dialog shows **ALREADY CLONED, CONTINUE USES IT**
   `<home>/<folder>/Hello-World`. No second clone starts.

> **CAUTION:** THE NEXT STEP DELETES FOLDERS. Delete only the two folders that this
> task made, in the private home of the fixture.

5. Close the dialog with **Cancel** and remove the clone and its owner folder:

   ```sh
   rm -rf <data-home>/<folder>/Hello-World <data-home>/<folder>/octocat
   ```

   Result: Neither folder exists.

### 6.33 C33 — Make a new workspace in the cloud, with a GPU or on This PC

This task starts no cloud. `<local>` is a new repository whose committed
`.horizon/cloud.yml` sets `placement: local`.

1. Make `<local>`:

   ```sh
   mkdir -p <data-home>/smoke/local/.horizon && cd <data-home>/smoke/local && git init -q
   printf 'version: 1\ndefault: dev\nplacement: local\nprofiles:\n  dev:\n    provider: runpod\n    image: <worker-image>\n    min_cpu: 2\n    min_memory_gb: 4\n' > .horizon/cloud.yml
   git add -A && git -c user.name=smoke -c user.email=smoke@example.invalid commit -q -m local
   ```

   Result: `git -C <data-home>/smoke/local log --oneline` shows one commit.

2. Click **New** in the sidebar.

   Result: A menu shows **Cloud**, **Cloud GPU** and **This PC**, in that order.

3. Click **This PC**.

   Result: A new empty workspace shows. No dialog opens.

4. Click **New**, then **Cloud**. Then click **Cancel** in **New cloud**.

   Result: **New cloud** opens for a new workspace. After **Cancel**, that
   workspace is gone and the sidebar shows the same workspaces as before.

5. Click **New**, then **Cloud GPU**. Type `<repo>` in the repository field.

   Result: The dialog reads the profiles and selects `runpod-gpu`, the first
   profile with `gpu: true`. Click **Cancel**.

6. Double-click an empty part of the canvas.

   Result: The **New Workspace** menu shows **Cloud** and **Cloud GPU** first,
   then **This PC** above the presets.

7. Click **Cloud** in that menu. Type `<home>/smoke/local` in the repository field.
   This is `<data-home>/smoke/local` from step 1, as the fixture shows it.

   Result: The dialog shows **This repository runs on This PC** and
   **Open on This PC**.

8. Click **Open on This PC**.

   Result: The dialog closes. The workspace has a terminal whose directory is
   `<home>/smoke/local`.

> **CAUTION:** THE NEXT STEP DELETES A FOLDER. Delete only `<data-home>/smoke/local`.

9. Close the workspaces that this task made, then remove `<local>`:

   ```sh
   rm -rf <data-home>/smoke/local
   ```

   Result: The folder does not exist.

## 7. Pass criteria

- C01 opens the dialog from the panel picker, the toolbar and **More**.
- C02 refuses a cloud in a detached workspace. C03 refuses a cloud in an unsaved session.
- C04 to C11 show the picks, the prices, the filters, the refresh, the watch,
  the tailnet chooser, the siblings and the fields as written.
- C12 to C30 show the expected counts and values that you calculated from the
  settings and the provider catalogs. A known defect has its issue link.
- C31 shows that each worker runs in the place that the summary named.
- C32 clones a pasted link into `<folder>/<owner>/<repository>` and uses an
  earlier checkout at `<folder>/<repository>` without a second clone.
- C33 offers Cloud, Cloud GPU and This PC for a new workspace, takes away the
  empty workspace of a cancelled Cloud, selects a GPU profile for Cloud GPU and
  opens a `placement: local` repository on This PC.
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

3. Keep `<run>/hetzner.header`. The cleanup of area X deletes it.

   Result: The header file stays in `<run>` for the teardown. Do not show its content.

## 9. Record of results

Write the results in the report of the run. Record the expected value and the
observed value of each count. Use the
[report template](../../reports/TEMPLATE.md). Keep private evidence out of the
repository.
