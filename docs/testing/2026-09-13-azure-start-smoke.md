# Azure explicit Start: temporary smoke plan (slice 3c of the Azure lifecycle integration)

Temporary validation artifact for the Remote Environments overview change that offers
**Start environment…** for saved-Stopped Azure workers. Delete after the pass.

## Scope

Only the overview's Stop section changed: Start is offered for an Azure row whose saved
identity is an Azure worker and whose saved phase is a verified Stop or existing Start
intent; the confirmation discloses the declared hourly cost, the same identity, that
in-memory work did not survive the stop and nothing resumes a task; the pending label
names Azure Start; an unverified Start points to a retry; a row with Start intent offers
neither Stop nor the saved-Stop check. Local Docker and RunPod behaviour is unchanged. No
real provider mutation is part of this smoke: the Azure CLI is not signed in to the
synthetic subscription, so the one Start request fails before any control-plane call and
leaves saved Start intent behind, exactly as an interrupted real Start would.

## Fixture

The synthetic home from the earlier plans plus one Azure environment driven to a
verified saved Stop (`azure-stopped`) through the public Stop coordinator with a fake
provider; `remote.azure` profile `cpu` on a synthetic subscription; `target/debug/horizon`
from the exact candidate on an isolated display.

## Checks

| # | Step | Expected |
| --- | --- | --- |
| 1 | Select `azure-stopped` | **Start environment…** and **Check saved Stop** enabled, **Stop environment…** disabled; help text names the declared hourly cost and that nothing resumes |
| 2 | Select `azure-stopping`, `azure-ready`, `docker-ready` | Start disabled on each; the other controls unchanged from slices 1 and 2 |
| 3 | Press **Start environment…** on `azure-stopped` | confirmation shows the identity rows, the cost line "0.10 currency units per hour" for the fixture profile, "already runs is not re-posted", "did not survive the stop", "nothing resumes a task", "no private SSH key", "Azure CLI login", "Reconnect session panels", "exiting Horizon"; Enter does not submit; Cancel closes it |
| 4 | Press **Start environment…** again, then **Start environment** (confirm) | pending label "Azure Start is pending…"; every control disabled while pending; a second press does nothing |
| 5 | Wait for the result (CLI not signed in) | "Last explicit Start result:" with the unverified Azure message, the retry hint and the CLI hint; after **Refresh saved page** the row shows "Start requested (saved)", Start enabled (retry), Stop and Check disabled; the saved revision advanced by exactly one (intent) |
| 6 | Press **Start environment…** and confirm again | the retry runs against the saved intent and ends unverified; revision unchanged |
| 7 | Close and reopen the overview; resize to 760×560 | nothing repeats; layout readable |

## Evidence to record

Screenshots for steps 1, 3, 4, 5; the store's revisions before and after; the list of
files under the home before and after.
