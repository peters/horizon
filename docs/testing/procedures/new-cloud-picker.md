---
procedure: new-cloud-picker
feature: New cloud dialog, worker list, filters, picks and data centers
platforms: [linux]
cost: none
destructive: yes
secrets: [RunPod API key in Cloud settings, Hetzner API token in Cloud settings]
owner: peters
---

# New cloud picker test procedure

## 1. Purpose

This procedure proves that the worker picker in the **New cloud…** dialog shows
the correct offers. It examines the worker list, the filters, the search, the
estimated totals, the **Cheapest**, **Balanced** and **Most powerful** cards,
and the data center chips. It also examines the `cloud_offers` MCP tool where
the UI and the agents must agree.

## 2. Applicability

- Candidate: a build that includes the fixes for
  [issue #1302](https://github.com/peters/horizon/issues/1302) and
  [issue #1303](https://github.com/peters/horizon/issues/1303).
- Platforms: Linux with Xvfb. Providers: RunPod and Hetzner.
- Test list: C12 to C30 of the cloud panel smoke test,
  [issue #1264](https://github.com/peters/horizon/issues/1264).
- This procedure does not test:
  - The start of a cloud.
  - A background price refresh. The
    [catalog refresh procedure](new-cloud-catalog-refresh.md) tests it.
  - The region chip for a region without storage (C26). A later fix adds this
    task.

## 3. Safety

> **CAUTION:** DO NOT CLICK **Start cloud**. This button rents compute from the
> provider. The provider charges money until somebody deletes the worker.

> **CAUTION:** DO NOT PUT THE API KEY OR THE TOKEN IN SCREENSHOTS, RECORDINGS OR
> LOGS. A person who gets them can rent compute on that account.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The fixture `scripts/device-smoke/serve.py` with `--native-view`.
- A Device panel that shows a live view of the isolated desktop.
- A RunPod API key from the secret store of the test account.
- A Hetzner API token from the secret store of the test account.
- An MCP client that can call the `cloud_offers` tool of the candidate.
- A Git repository with a `.horizon/cloud.yml` file that has these profiles:

  ```yaml
  version: 1
  default: small
  profiles:
    small:
      provider: runpod
      image: example.invalid/worker:latest
      min_cpu: 2
      min_memory_gb: 4
      gpu: false
    large:
      provider: runpod
      image: example.invalid/worker:latest
      min_cpu: 8
      min_memory_gb: 32
      gpu: false
    gpu:
      provider: runpod
      image: example.invalid/worker:latest
      min_cpu: 8
      min_memory_gb: 32
      gpu: true
  ```

The reference counts in this procedure come from the test account of
issue #1264. If the provider catalog changes, write the new counts in the
report.

## 5. Setup

1. Start the fixture with a new directory for its state.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <frozen-candidate> \
     --native-view --state <new-directory>
   ```

   Result: The fixture output shows a `vnc_address`.

2. Open a Device panel with the `vnc_address`.

   Result: The Device panel shows a live view of the candidate.

3. In the candidate, open **Cloud settings…**.

   Result: The Cloud settings dialog opens.

   > **CAUTION:** PASTE THE KEY AND THE TOKEN ONLY INTO THE CANDIDATE IN THE
   > ISOLATED DESKTOP. If you paste them in another window, other people or logs
   > can get them.

4. Paste the RunPod API key.

   Result: The RunPod field shows **Unsaved key**.

5. Enable Hetzner and paste the Hetzner API token.

   Result: The Hetzner field shows **Unsaved key**.

6. Type the server types `cx33`, `cx43` and `cpx42`.

   Result: The dialog shows the three server types.

7. Type the locations `fsn1`, `hel1` and `nbg1`.

   Result: The dialog shows the three locations.

8. Click **Save settings**.

   Result: Horizon saves the settings and closes the dialog.

   Note: If you close the dialog without **Save settings**, Horizon discards
   the key, the token and the lists.

9. Open **Cloud › New cloud…**.

   Result: The New cloud dialog opens.

10. Type the path of the repository.

    Result: The dialog reads the repository and selects the profile `small`.

11. Wait until the prices load.

    Result: The dialog shows the **Cheapest**, **Balanced** and **Most
    powerful** cards.

12. Click **All providers**.

    Result: The worker list shows RunPod rows and Hetzner rows.

## 6. Tasks

Give each task an ID. A report uses the ID to give a result. Do the tasks in
this order. Each task starts with the result of the task before it.

### 6.1 C12 — Catalog size

1. Clear **In stock only**.

   Result: The check box shows no check mark.

2. Read the text **Showing N of M workers** above the worker list.

   Result: The text shows **Showing 24 of 24 workers**.

### 6.2 C13 — RunPod size grid

1. Click **RunPod**.

   Result: The worker list shows only RunPod rows.

2. Count the rows in the worker list.

   Result: The list has 15 rows.

3. Examine the sizes in the rows.

   Result: Each row has 2, 4, 8, 16 or 32 vCPU. The memory is 2, 4 or 8 GB
   for each vCPU.

### 6.3 C14 — RunPod CPU flavors

1. Examine the flavor names in the RunPod rows.

   Result: The rows show only **Compute-Optimized**, **General Purpose** and
   **Memory-Optimized**.

2. If a row shows two names with **or**, examine its price.

   Result: The price starts with **up to**.

### 6.4 C15 — Hetzner allowlist

1. Click **Hetzner**.

   Result: The worker list shows only Hetzner rows.

2. Count the rows in the worker list.

   Result: The list has 9 rows.

3. Examine the server type and the location of each row.

   Result: Each row has `cx33`, `cx43` or `cpx42`, in `fsn1`, `hel1` or `nbg1`.

### 6.5 C16 — Unlisted Hetzner offers

1. Find the rows that show **Unlisted · advisory**.

   Result: Hetzner does not list these server types in these locations now.

2. Record the text **Showing N of M workers**.

   Result: You have the count without the stock filter.

3. Check **In stock only**.

   Result: The check box shows a check mark.

4. Read the text **Showing N of M workers** again.

   Result: The count is the same as in step 2.

5. Find the rows that show **Unlisted · advisory**.

   Result: The list shows each row from step 1.

6. Examine the three cards.

   Result: A card can show **Unlisted · advisory**. The cards do not skip an
   offer only because Hetzner does not list it.

7. Click a row that shows **Unlisted · advisory**.

   Result: The summary shows the server type and the location of that row.

### 6.6 C17 — Hidden count of the stock filter

1. Click **RunPod**.

   Result: The worker list shows only RunPod rows.

2. Clear **In stock only**.

   Result: The check box shows no check mark.

3. Record the text **Showing N of M workers**.

   Result: You have the count N without the stock filter.

4. Count the rows that show **Out of stock**.

   Result: You have the number of sold-out rows.

5. Check **In stock only**.

   Result: The check box shows a check mark.

6. Read the text **Showing N of M workers** again.

   Result: The count is N minus the number of sold-out rows from step 4.

### 6.7 C18 — Workers below the requirements

1. Click **All providers**.

   Result: The worker list shows RunPod rows and Hetzner rows.

2. Select the profile `large`.

   Result: The text above the list shows a number of rows **below requirements
   hidden**.

3. Check **Show workers below requirements**.

   Result: The list shows more rows. The added rows are disabled.

4. Put the pointer on a disabled row.

   Result: A tooltip shows **Below the profile's minimum CPU count** or **Below
   the profile's minimum memory**.

5. Click a disabled row.

   Result: The summary does not change.

6. Select the profile `small`.

   Result: The filters go back to their default values.

### 6.8 C19 — Search

1. Clear **In stock only**.

   Result: The check box shows no check mark.

2. Type `16 vCPU` in the search field.

   Result: Each row in the list has 16 vCPU.

3. Remove the text from the search field.

   Result: The list shows all rows again.

4. Type `fsn1` in the search field.

   Result: The list shows only the Hetzner rows in `fsn1`.

5. Remove the text from the search field.

   Result: The list shows all rows again.

### 6.9 C20 — Currencies

1. Click **Hetzner**.

   Result: A note shows **Estimated totals in EUR**.

2. Examine a Hetzner card.

   Result: The price and the total are in euros (**€**).

3. Click **RunPod**.

   Result: The prices and the totals are in US dollars (**$**).

4. Click **All providers**.

   Result: A note shows **Estimated totals in USD · ECB rates dated** and a date.

5. Examine a Hetzner card.

   Result: The hourly price is in euros. The total is in US dollars.

### 6.10 C21 — Order by estimated total

1. Clear **In stock only**.

   Result: The check box shows no check mark.

2. Read the estimated total under the price of each row, from the top down.

   Result: Each total is equal to or more than the total in the row above it.
   The rows of the two providers can alternate.

3. Examine the first row.

   Result: The row shows Hetzner `cx33`.

4. Call the `cloud_offers` MCP tool with this request:

   ```json
   {"min_vcpu": 2, "min_memory_gb": 4, "limit": 50}
   ```

   Result: The answer has a `comparison` with `complete` set to `true`.

5. Compare the order of `comparison.offers` with the order of the rows.

   Result: The order is the same.

   Note: The MCP totals do not include the RunPod container disk. If a RunPod
   total and a Hetzner total differ by less than this charge, the order can
   differ.

6. Check **In stock only**.

   Result: The check box shows a check mark.

### 6.11 C22 — Picks

1. Examine the **Cheapest** card.

   Result: The card shows Hetzner `cx33`.

2. Examine the **Most powerful** card.

   Result: The card shows **32 vCPU · 256 GB** from RunPod.

3. Call the `cloud_offers` MCP tool with `{"min_vcpu": 2, "min_memory_gb": 4}`.

   Result: The answer has a `comparison` with `complete` set to `true`.

4. Examine the first offer in `comparison.offers`.

   Result: The offer is Hetzner `cx33`, the same as the **Cheapest** card.

### 6.12 C23 — Totals for 730 hours

1. Set **Compare for** to `730` hours.

   Result: The totals on the cards and the summary change.

2. Click **RunPod**.

   Result: The cards show totals in US dollars.

3. Calculate the hourly price of a RunPod card × 730.

   Result: The card total is a little more than this value. The difference is
   the workspace volume and the container disk for 730 hours.

4. Click **Hetzner**.

   Result: The cards show totals in euros.

5. Examine the total of a Hetzner card.

   Result: The total is not more than the capped charges of each UTC calendar
   month in the run.

   Note: Hetzner caps the compute, the volume and the IPv4 address for each
   UTC calendar month. A run of 730 hours can touch two or three months.

6. Click **All providers**.

   Result: The worker list shows RunPod rows and Hetzner rows.

7. Set **Compare for** to `1` hour.

   Result: The totals go back to the values for one hour.

### 6.13 C24 — Data centers

1. Click a RunPod row.

   Result: The dialog shows the **Data center** section.

2. Count the data center chips.

   Result: The section shows 31 data centers.

3. Count the chips that show **Storage unavailable**.

   Result: 21 chips show **Storage unavailable**. These chips are disabled.

### 6.14 C25 — Exact stock on the region chips

1. Record the stock text of each region chip.

   Result: You have the stock counts for this size.

2. Click a RunPod row with a different size.

   Result: The summary shows the new size.

3. Wait until no region chip shows **checking stock**.

   Result: The chips show the stock counts for the new size.

### 6.15 C27 — Region scope

1. Click the region chip **Europe**.

   Result: The summary shows **any in Europe**.

2. Examine the RunPod rows.

   Result: The stock of the RunPod rows is the stock in Europe.

3. Examine the Hetzner rows.

   Result: The Hetzner rows do not change.

4. Click **Any data center**.

   Result: The summary shows **any data center**.

### 6.16 C28 — High-performance storage

1. In the summary, click **High-performance**.

   Result: Fewer data centers can hold the workspace. The other chips show
   **Storage unavailable**.

2. Examine the cards.

   Result: The dialog shows no **Cheapest**, **Balanced** or **Most powerful**
   card. A note tells you that high-performance storage prices are not published.

3. Click **Standard**.

   Result: The three cards show again.

### 6.17 C29 — Stock of the largest size

1. Click **RunPod**.

   Result: The worker list shows only RunPod rows.

2. Click the row **32 vCPU · 256 GB**.

   Result: The summary shows **32 vCPU · 256 GB**.

3. Wait 15 seconds.

   Result: The summary shows **In stock**, **Low stock** or **Out of stock**.
   It does not show **stock unknown**.

### 6.18 C30 — GPU workers

1. Select the profile `gpu`.

   Result: The worker list shows GPU types.

2. Count the rows in the worker list.

   Result: The list has about 33 GPU types.

3. Examine the providers in the list.

   Result: The list shows no Hetzner row.

4. Call the `cloud_offers` MCP tool with `{"gpu": true, "region": "EUROPE"}`.

   Result: Each offer has a GPU type that is in stock in Europe.

5. Call the `cloud_offers` MCP tool with `{"gpu": true, "region": "NOWHERE"}`.

   Result: The answer has no offers.

## 7. Pass criteria

- C12 shows 24 workers. C13 shows 15 RunPod rows. C15 shows 9 Hetzner rows.
- In C21, the rows are in the order of their estimated totals, and each row
  shows its total. The MCP comparison has the same order.
- In C16, **In stock only** keeps each row that shows **Unlisted ·
  advisory**, and the cards can show these rows.
- The hidden count in C17 is the number of sold-out rows.
- In C18, a row below the requirements cannot be selected.
- The search, the currencies, the picks and the totals agree with C19, C20, C22 and C23.
- The MCP answer in C22 has the same cheapest offer as the **Cheapest** card.
- The data center chips and the region chips agree with C24, C25 and C27.
- C28 hides the cards for high-performance storage.
- C29 never shows **stock unknown**.
- C30 shows no Hetzner row for a GPU profile.

## 8. Cleanup

1. Close the New cloud dialog with **Cancel**.

   Result: The dialog closes. No cloud starts.

2. Close the Device panel.

   Result: The Device panel closes. The fixture continues.

3. Stop the fixture with Ctrl+C.

   Result: The fixture stops the candidate, the display and the VNC server.

   > **CAUTION:** DELETE ONLY THE DIRECTORY THAT THIS RUN GAVE TO `--state`.
   > This step deletes the saved key and token. Other directories can hold data
   > of other people.

4. Delete the directory of the fixture state.

   Result: The directory, the saved key and the saved token are deleted.

## 9. Record of results

Put the results in the pull request. Keep the screenshots and the recording
private, because they show account data.
