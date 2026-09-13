# Azure first explicit Stop: temporary smoke plan (slice 2 of the Azure lifecycle integration)

Temporary validation artifact for the Remote Environments overview change that offers
the first explicit **Stop environment…** for retained persistent Azure workers. Delete
after the pass.

## Scope

Only the overview's Stop controls changed: the first Stop is offered for an Azure row
whose saved identity is an Azure worker without Stop intent, the confirmation discloses
the deallocation, the retained disk and its continuing billing, the pending label names
Azure, and an unverified Stop points to Check saved Stop. Rows with existing intent keep
offering only the check. Local Docker and RunPod behaviour is unchanged. No real
provider mutation is part of this smoke: the Azure CLI is not signed in to the
synthetic subscription, so the one Stop request fails before any control-plane call and
leaves saved intent behind, exactly as an interrupted real Stop would.

## Fixture

Same synthetic home as the slice 1 plan: `remote.azure` profile `cpu` on a synthetic
subscription, one Azure environment with saved Stop intent (`azure-stopping`), one
without (`azure-ready`), one Local Docker environment; `target/debug/horizon` from the
exact candidate on an isolated display.

## Checks

| # | Step | Expected |
| --- | --- | --- |
| 1 | Select `azure-ready` | **Stop environment…** enabled, **Check saved Stop** disabled |
| 2 | Select `azure-stopping` | unchanged from slice 1: only the check is offered |
| 3 | Press **Stop environment…** on `azure-ready` | confirmation shows workspace, owning session, profile and exact resource ID, the process-memory warning, "Deallocates the worker VM", the retained disk and continuing disk billing, "no private SSH key", "Azure CLI login", "never resend", "exiting Horizon"; Enter does not submit; Cancel closes it |
| 4 | Press **Stop environment…** again, then **Stop environment** (confirm) | pending label "Azure Stop is pending…"; every control disabled while pending; a second press does nothing |
| 5 | Wait for the result (CLI not signed in) | "Last explicit Stop result:" with the unverified Azure message and the hint to refresh and use Check saved Stop; after **Refresh saved page** the row shows "Stop requested (saved)", Stop is disabled and Check is enabled; the saved revision advanced by exactly one (intent) |
| 6 | Press **Check saved Stop** on that row | the slice 1 check runs against the saved intent and ends unverified; revision unchanged |
| 7 | Close and reopen the overview; resize to 760×560 | nothing repeats; layout readable |

## Evidence to record

Screenshots for steps 1, 3, 4, 5; the store's revisions before and after; the list of
files under the home before and after.
