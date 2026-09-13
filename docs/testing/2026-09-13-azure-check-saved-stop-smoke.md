# Azure Check saved Stop: temporary smoke plan (PR slice 1 of the Azure lifecycle integration)

Temporary validation artifact for the Remote Environments overview change that offers
**Check saved Stop** for retained persistent Azure workers. Delete after the pass.

## Scope

Only the overview's Stop controls changed: the check is offered for an Azure row whose
saved identity is an Azure worker with saved Stop intent (`Stopping` or `Stopped`), the
help and unverified-check texts name Azure, and the first **Stop environment…** button
stays disabled for Azure rows (a later slice). Local Docker and RunPod behaviour is
unchanged. No provider I/O is part of this smoke; the Azure CLI must not be signed in,
so a check ends as an unverified observation.

## Fixture

1. Fresh synthetic Horizon home (never the operator's). Write a config whose
   `remote.azure` list holds one profile named `cpu` (subscription `11111111-1111-4111-8111-111111111111`,
   `northeurope`, `Standard_D4s_v3`, registry `synthetic.azurecr.io`, a pull identity
   under the same subscription, declared cost `100000`, disk SKU `StandardSSD_LRS`).
2. Seed the cloud workflow store with one Azure allocation exactly as
   `crates/horizon-core/src/remote_workspace/stop/tests/configured_azure.rs` builds it
   (persistent target on profile `cpu`, immutable CPU binding, retained worker with a
   complete public pin, phase `Stopping`). Seed a second row without Stop intent
   (phase `Ready`) and a Local Docker row.
3. Start `target/debug/horizon` against that home on an isolated display.

## Checks

| # | Step | Expected |
| --- | --- | --- |
| 1 | Open the Remote Environments overview, select the Azure `Stopping` row | **Check saved Stop** enabled; **Stop environment…** disabled; help text names the existing Azure Stop intent, the immutable saved binding, the Azure CLI login and "no private SSH key" |
| 2 | Select the Azure `Ready` row | both buttons disabled; the dim text says Azure Stop intent can only be checked and a first Azure Stop is not offered here yet |
| 3 | Select the Local Docker row | unchanged: **Stop environment…** enabled, **Check saved Stop** disabled |
| 4 | Back on the Azure `Stopping` row, press **Check saved Stop** once | pending label "Checking saved Stop. No Stop request is sent…"; the button is disabled while pending; a second press does nothing |
| 5 | Wait for the result (CLI not signed in) | "Last saved Stop check:" with the unverified message, plus the Azure hint about signing in to the CLI and "nothing was sent to the worker"; the saved row is unchanged (revision and phase) |
| 6 | Press Enter, close and reopen the overview, refresh the page | no check is repeated; the notice stays bound to the same row; no new files under the home other than the store |
| 7 | Resize to 760×560 and back | controls and texts remain readable; no overlap |

## Evidence to record

Screenshots for steps 1, 2, 5 and 7; the store's revision and phase before and after;
the list of files under the home before and after.
