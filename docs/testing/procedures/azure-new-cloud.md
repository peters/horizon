---
procedure: azure-new-cloud
feature: Azure in the New cloud dialog
platforms: [linux]
cost: none            # task ANC-L1 rents compute; the other tasks do not
destructive: no
secrets: [ARM bearer token in a private token file, for task ANC-L1 only]
owner: peters
---

# Azure in the New cloud dialog test procedure

## 1. Purpose

This procedure proves that the **New cloud** dialog offers Azure only on a
computer whose cloud settings have a valid `azure` section. It proves that the
dialog lists only the VM sizes and regions of that section, prices them in US
dollars, and starts a cloud on the one VM size and region that the person
chooses.

## 2. Applicability

- Candidate: a build that includes the chosen Azure placement and Azure in New
  cloud (PR 6 of [epic #1774](https://github.com/peters/horizon/issues/1774)).
- Platforms: Linux with Xvfb. Provider: Azure, beside RunPod.
- This procedure does not test:
  - A form for the `azure` section in **Cloud settings**. There is no such
    form. You write the section in `settings.json`.
  - Stop, resume, delete and the deletion proof of an Azure cloud. The
    [Azure provisioning procedure](azure-provisioning.md) tests them.
  - The Azure offers of the `cloud_offers` MCP tool. The
    [Azure price catalog procedure](azure-price-catalog.md) tests them.

## 3. Safety

> **CAUTION:** DO NOT CLICK **Start cloud** FOR AN AZURE ROW IN TASKS ANC-1 TO
> ANC-6. Use task ANC-L1 only on a subscription that you can use for tests.
> A started cloud bills its VM, its disks and its static address until somebody
> deletes it.

> **CAUTION:** DO NOT PUT A REAL ARM TOKEN IN SCREENSHOTS, RECORDINGS OR LOGS.
> A person who gets it can change resources in the subscription until the token
> expires.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The fixture `scripts/device-smoke/serve.py` with `--native-view` and
  `--saved-session`.
- A Device panel that shows a live view of the isolated desktop.
- HTTPS access from the fixture to `prices.azure.com`. The Azure price list
  needs no credential.
- A RunPod API key in **Cloud settings** of the fixture, or the key form in the
  dialog. Tasks ANC-1 to ANC-6 need the settings file that **Cloud settings**
  writes.
- A Git repository with this `.horizon/cloud.yml` file:

  ```yaml
  version: 1
  default: development
  profiles:
    development:
      provider: runpod
      image: example.invalid/worker:latest
      cpu: 4
      memory_gb: 8
    azure:
      provider: azure
      image: example.invalid/worker:latest
      cpu: 4
      memory_gb: 8
  ```

In this procedure, `$CLOUD` is the cloud settings folder of the fixture:
`<state>/data/home/.horizon/cloud`.

## 5. Setup

1. Build the candidate and start the fixture.

   ```sh
   cargo build -p horizon-ui --bin horizon
   smoke_bin=$(mktemp -d /tmp/horizon-smoke-bin.XXXXXX)
   cp "$CARGO_TARGET_DIR/debug/horizon" "$smoke_bin/horizon"
   (cd "$smoke_bin" && sha256sum horizon > SHA256SUMS)
   python3 scripts/device-smoke/serve.py --horizon "$smoke_bin/horizon" \
     --native-view --saved-session --state /tmp/horizon-azure-new-cloud
   ```

   Result: The fixture prints a manifest with `vnc_address`.

2. Open a Device panel on `vnc_address`. Examine the viewer health.

   Result: `image_received` and `image_displayed` are true. `frame_sequence`
   increases while the desktop changes.

3. Find the Horizon child of the fixture. Compare
   `sha256sum /proc/<pid>/exe` with `SHA256SUMS`.

   Result: The two values are the same.

4. In the candidate, open **Cloud settings**, put a RunPod API key, and click
   **Save**.

   Result: `$CLOUD/settings.json` exists. It has no `azure` section.

## 6. Tasks

### 6.1 ANC-T — Run the headless tests

1. Run the tests of the dialog and the placement.

   ```sh
   cargo test -p horizon-ui --bin horizon -- cloud_panel
   cargo test -p horizon-core --lib -- cloud_runtime::settings cloud_panel::selection
   cargo test -p horizon-cloud --lib -- offers::workers
   ```

   Result: All tests pass. These tests examine the items that follow:
   - Without an `azure` section, the dialog shows no Azure chip, filter or row.
   - With a section whose token cannot be read, the dialog shows the reason and
     no Azure row.
   - With a valid section, the dialog lists only its VM sizes and regions,
     prices them in US dollars, and records the chosen size and region in the
     new cloud.
   - `Settings::for_cloud` narrows the section to the chosen size and region,
     and refuses a size or region that the section does not have.

### 6.2 ANC-1 — Examine the dialog without an `azure` section

1. Open the test repository in **New cloud**. Keep the `development` profile.

   Result: The heading shows RunPod and no Azure chip. **All machines and
   filters** shows no **Azure** filter. No row starts with `Azure ·`.

2. Select the `azure` profile.

   Result: The checks show "No Azure settings on this computer yet" with the
   names `token_file`, `subscription_id`, `vm_sizes` and `regions`, or "Add an
   azure section to the cloud settings". The check list shows no key form for
   Azure. **Start cloud** stays disabled.

### 6.3 ANC-2 — Add a valid `azure` section

1. Close the dialog. Make a private token file with a synthetic token.

   ```sh
   umask 077
   printf 'synthetic-arm-token\n' > "$CLOUD/azure-token"
   ```

   Result: `$CLOUD/azure-token` has mode `0600`.

2. Add this section to `$CLOUD/settings.json`. Use the absolute path of the
   token file.

   ```json
   "azure": {
     "token_file": "<absolute path of $CLOUD/azure-token>",
     "subscription_id": "00000000-0000-4000-8000-000000000001",
     "vm_sizes": ["Standard_D4s_v3", "Standard_D8s_v3"],
     "regions": ["northeurope"]
   }
   ```

   Result: The file is valid JSON.

3. Open the test repository in **New cloud**. Keep the `development` profile.
   Wait until the Azure prices load.

   Result: The heading shows an Azure chip with a blue "A" mark. **All machines
   and filters** shows an **Azure** filter.

4. Click the **Azure** filter.

   Result: The list shows `Azure · Standard_D4s_v3 · northeurope` and
   `Azure · Standard_D8s_v3 · northeurope`. Each row has the pill "Quota
   checked at start" and a price in `$`. No row shows `Standard_D2s_v3` or a
   region other than `northeurope`. The note says "Only the VM sizes and
   regions of the azure cloud settings are listed."

### 6.4 ANC-3 — Choose an Azure row

1. Click the row `Azure · Standard_D4s_v3 · northeurope`.

   Result: The summary shows "4 vCPU · 16 GB", "Azure · profile development"
   and `northeurope`. It shows the lines **VM size** `Standard_D4s_v3`,
   **Compute** in `$`/hr, **Estimated run, disks + static address** in `$`,
   and **Stopped workspace** in `$`/mo. The note starts with "Azure retail
   prices in US dollars, before tax". No amount shows `€`.

2. Examine the storage fields in the summary.

   Result: The fields are **Workspace size** and **OS disk**. The note under
   **OS disk** says that it is kept, and billed, while the cloud is stopped.

3. Click the row `Azure · Standard_D8s_v3 · northeurope`.

   Result: The summary shows "8 vCPU · 32 GB" and **VM size**
   `Standard_D8s_v3`.

### 6.5 ANC-4 — Make the token unreadable

1. Close the dialog. Make the token file readable by others.

   ```sh
   chmod 644 "$CLOUD/azure-token"
   touch "$CLOUD/settings.json"
   ```

   Result: The mode of the token file is `0644`.

2. Open the test repository in **New cloud**. Select the `azure` profile.

   Result: The selector shows "Azure workers are not offered:" and the reason.
   No row starts with `Azure ·`. The provider check fails with the same
   reason. The dialog shows no key form. **Start cloud** stays disabled.

3. Close the dialog. Set the mode back.

   ```sh
   chmod 600 "$CLOUD/azure-token"
   touch "$CLOUD/settings.json"
   ```

   Result: The mode of the token file is `0600`.

### 6.6 ANC-5 — Use a VM size that the catalog does not have

1. Close the dialog. In `$CLOUD/settings.json`, set `"vm_sizes"` to
   `["Standard_NC6"]`.

   Result: The file is valid JSON.

2. Open the test repository in **New cloud**. Select the `azure` profile.

   Result: The checks show "Azure settings need VM sizes from Horizon's
   catalog, such as Standard_D4s_v3. Open Cloud settings to repair them." The
   dialog shows no Azure row. **Start cloud** stays disabled.

3. Close the dialog. Set `"vm_sizes"` back to
   `["Standard_D4s_v3", "Standard_D8s_v3"]`.

   Result: The file is valid JSON.

### 6.7 ANC-6 — Remove a chosen size from the settings

1. Open the test repository in **New cloud**. Click the **Azure** filter and
   the row `Azure · Standard_D8s_v3 · northeurope`.

   Result: The summary shows **VM size** `Standard_D8s_v3`.

2. Keep the dialog open. In `$CLOUD/settings.json`, set `"vm_sizes"` to
   `["Standard_D4s_v3"]`. Move the pointer over the dialog.

   Result: The row `Azure · Standard_D8s_v3 · northeurope` goes away. The
   summary says "Choose an exact worker and location." The footer says "The
   selected worker is no longer offered for this profile. Choose another
   worker." **Start cloud** stays disabled.

3. Set `"regions"` to `["westeurope"]`. Move the pointer over the dialog.

   Result: No `northeurope` row shows. Only `westeurope` rows show, if the
   price list has prices for that region.

4. Set `"regions"` back to `["northeurope"]` and `"vm_sizes"` back to
   `["Standard_D4s_v3", "Standard_D8s_v3"]`.

   Result: The file is valid JSON.

### 6.8 ANC-7 — Examine the window after a resize

1. Open the dialog with an Azure row chosen. Make the Horizon window smaller,
   then larger, then fit the board.

   Result: The Azure chip, the rows, the summary and **Start cloud** stay
   inside the dialog. No text overlaps.

### 6.9 ANC-L1 — Start one Azure cloud

> **CAUTION:** THIS TASK RENTS COMPUTE IN THE AZURE SUBSCRIPTION. Delete the
> cloud at the end of the task.

1. Write a real ARM token to the token file and the real subscription to
   `subscription_id`.

   ```sh
   umask 077
   az account get-access-token --query accessToken -o tsv > "$CLOUD/azure-token"
   az account show --query id -o tsv
   ```

   Result: The token file has mode `0600`. The subscription is a lowercase UUID.

2. Open the test repository in **New cloud**. Select the `azure` profile.
   Click the row `Azure · Standard_D4s_v3 · northeurope`. Click
   **Start cloud**.

   Result: The dialog closes. A cloud card shows. Its worker becomes ready.
   The card shows the provider Azure, the region `northeurope` and the rate
   text of Azure.

3. Delete the cloud from its card.

   Result: The card goes away after the deletion proof. `az group list` shows
   no group of the cloud.

## 7. Pass criteria

- Without an `azure` section, the dialog shows no Azure chip, filter, row or
  price.
- With a valid section, the dialog lists only the VM sizes and regions of the
  section, in US dollars.
- A token that cannot be read, a size outside the catalog and a removed size
  each show a reason, and **Start cloud** stays disabled.
- A chosen row records its VM size and region in the new cloud.

## 8. Cleanup

1. Stop the fixture with `Ctrl+C`.

   Result: The fixture stops its display, its window manager and Horizon.

2. Remove the state directory and the frozen binary.

   ```sh
   rm -rf /tmp/horizon-azure-new-cloud "$smoke_bin"
   ```

   Result: The two directories do not exist.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
