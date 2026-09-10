# Azure workspace readiness preflight

Read-only preflight for the Azure CPU persistent-workspace candidates tracked in
[#474](https://github.com/peters/horizon/issues/474) (parent
[#383](https://github.com/peters/horizon/issues/383)). It identifies
authentication, provider-registration, region, SKU and regional-quota gaps before
anyone spends money on the three-creation spike. It never selects a platform and
never reports "ready": the useful outputs are `no_blockers_observed`, `unknown`
and `blocked`, each with the list of live-qualification gates that remain open.

Requirements: Python 3.9+ standard library only. Live reads additionally need the
official Azure CLI (`az`) already installed and already logged in by the operator.
The tool never runs `az login`, never changes the default subscription or any
CLI configuration, never installs extensions, and never registers a provider or
creates, starts, stops, restarts or deletes a resource.

## Commands

Run everything from the repository root.

Focused tests (no Azure CLI, no credentials, no network):

```bash
python3 -B -m unittest discover -s scripts/azure-workspace-preflight/tests -v
```

Offline plan (default mode; zero subprocesses; shows the exact redacted commands
that a live run would execute):

```bash
python3 scripts/azure-workspace-preflight/preflight.py \
  --candidate aci --subscription <subscription-uuid> --region northeurope
```

Offline evaluation of synthetic responses (zero subprocesses; the fixture is a
JSON object keyed by check id, see the test module for the shape):

```bash
python3 scripts/azure-workspace-preflight/preflight.py \
  --candidate vm --vm-size Standard_D4s_v3 \
  --subscription <subscription-uuid> --region northeurope \
  --fixture /path/to/fixture.json --json
```

Live read-only preflight (requires `--live`; executes only the allowlisted
operations below against the explicit subscription and region):

```bash
python3 scripts/azure-workspace-preflight/preflight.py \
  --candidate aci --subscription <subscription-uuid> --region northeurope \
  --live --report /private/path/aci-northeurope.json
```

Options:

| Flag | Meaning |
| --- | --- |
| `--candidate {aci,vm,container-apps}` | Candidate to preflight. None is the selected architecture. |
| `--subscription` | Exact subscription UUID. Names and the CLI default are rejected. |
| `--region` | Lowercase region name, for example `northeurope`. |
| `--vm-size` | Required for `vm` only, for example `Standard_D4s_v3`. |
| `--cpu-cores`, `--memory-gb` | Requested worker size used for headroom checks (defaults 2 and 4). |
| `--live` / `--fixture PATH` | Live reads or synthetic responses. Mutually exclusive; default is plan mode. |
| `--report PATH` | Write the JSON report to a new private file (mode 0600). Existing files are refused. |
| `--json` | Print the JSON report instead of the text summary. |
| `--timeout-seconds`, `--az-path` | Per-command bound (default 60, max 300) and CLI executable. |

Exit codes: `0` planned or no blockers observed, `1` blocked, `2` unknown,
`3` rejected input including argument-parser errors (nothing executed), `4` report
file could not be written.

## What a live run executes

Only these documented read-only operations can be emitted. The planner asserts
the allowlist for every command, arguments are passed as an array without a
shell in their own process group, each command runs under the timeout (the whole
group is killed on expiry, because the packaged `az` is a shell wrapper), output
above 4 MiB per stream is discarded and reported as `oversized_output`, and
there are no retries. Checks declare prerequisites: when the account context is
blocked nothing else runs, when the region is unavailable the regional checks
are skipped, and the VM quota check waits for a resolved SKU. Skipped checks
report `unknown` with reason `prerequisite_failed`.

| Check | Operation | Source |
| --- | --- | --- |
| `account_context` | `az account show --subscription <id>` | [az account](https://learn.microsoft.com/en-us/cli/azure/account?view=azure-cli-latest#az-account-show) |
| `region_available` | `az rest --method get` on `/subscriptions/<id>/locations?api-version=2022-12-01`; passes only a `Region` whose `regionType` is `Physical` | [Subscriptions - List Locations](https://learn.microsoft.com/en-us/rest/api/resources/subscriptions/list-locations) |
| `provider_*` | `az provider show --namespace <ns> --subscription <id>` | [az provider show](https://learn.microsoft.com/en-us/cli/azure/provider?view=azure-cli-latest#az-provider-show) |
| `aci_regional_quota` | GET `.../providers/Microsoft.ContainerInstance/locations/<region>/usages?api-version=2026-07-01` | [Location - List Usage](https://learn.microsoft.com/en-us/rest/api/container-instances/location/list-usage) |
| `aci_regional_capabilities` | GET `.../providers/Microsoft.ContainerInstance/locations/<region>/capabilities?api-version=2026-07-01` | [Location - List Capabilities](https://learn.microsoft.com/en-us/rest/api/container-instances/location/list-capabilities) |
| `vm_sku_availability` | `az vm list-skus --location <region> --size <size> --resource-type virtualMachines --all` | [az vm list-skus](https://learn.microsoft.com/en-us/cli/azure/vm?view=azure-cli-latest#az-vm-list-skus) |
| `vm_regional_quota` | `az vm list-usage --location <region> --subscription <id>` | [az vm list-usage](https://learn.microsoft.com/en-us/cli/azure/vm?view=azure-cli-latest#az-vm-list-usage) |
| `container_apps_regional_quota` | GET `.../providers/Microsoft.App/locations/<region>/usages?api-version=2025-07-01` | [Usages - List](https://learn.microsoft.com/en-us/rest/api/resource-manager/containerapps/usages/list?view=rest-resource-manager-containerapps-2025-07-01) |

`az rest` semantics are documented in the
[az reference](https://learn.microsoft.com/en-us/cli/azure/reference-index?view=azure-cli-latest#az-rest);
the tool always passes a full `https://management.azure.com/subscriptions/<id>/...`
URL, so no `{subscriptionId}` placeholder and no CLI default is ever substituted.

Provider namespaces per candidate: `aci` requires `Microsoft.ContainerInstance`;
`vm` requires `Microsoft.Compute` and `Microsoft.Network`; `container-apps`
requires `Microsoft.App` and `Microsoft.Network`. `Microsoft.ContainerRegistry`
and `Microsoft.ManagedIdentity` are checked for every candidate because image
access must go through a managed identity, and `Microsoft.Storage` or
`Microsoft.OperationalInsights` where the candidate's documented setup needs them.
An unregistered provider is reported as a blocker; registering it is a separate
approval, not something this tool does.

## Report

The JSON report is versioned (`schema_version` 1) and contains `observed_at`,
`candidate`, `region`, the redacted `subscription`, one entry per check with
`role` (`required` or `supporting`), `outcome`, `reason` and safe `details`, the
`blockers` list, `unverified_gates` and an overall `status`. The overall status
folds every check the same way (`blocked` beats `unknown` beats
`no_blockers_observed`); a consumer that wants to weigh supporting providers
differently can fold by `role` itself.

Check outcomes:

| Outcome | Meaning |
| --- | --- |
| `planned` | Plan mode; nothing executed. |
| `observed_ok` | The read succeeded and no blocker was observed in that response. |
| `blocked` | A concrete gap that must be fixed before any creation attempt. |
| `unknown` | Missing, malformed, contradictory, unsupported or timed-out evidence. Never treated as available. |

Reason codes: `authentication_required`, `subscription_not_visible`,
`subscription_mismatch`, `subscription_not_enabled`, `insufficient_permission`,
`unregistered_provider`, `registration_in_progress`, `region_unavailable`,
`quota_exhausted`, `quota_entry_missing`, `sku_unavailable_in_region`,
`sku_restricted`, `no_linux_public_capability`,
`request_exceeds_regional_maximum`, `prerequisite_failed`, `throttled`,
`unsupported_query`, `malformed_response`, `contradictory_response`, `timeout`,
`oversized_output`, `command_failed`, `not_executed`.

Failure classification prefers the structured error code from the CLI or ARM
response (for example `AuthorizationFailed`, `MissingSubscriptionRegistration`,
`TooManyRequests`); free-text patterns are consulted only when no code parses.
The report records the code, never the message.

Quota checks compare `limit - currentValue` against the requested worker size:
`ContainerGroups` and `StandardCores` for `aci`, `ManagedEnvironmentCount` and
`ManagedEnvironmentCores` for `container-apps`, and Compute `cores` plus the SKU
family for `vm`. A negative limit is treated as unlimited. Every quota and SKU
detail carries `"capacity": "unverified"` because quota headroom is not regional
capacity. For `vm`, the SKU response supplies the family name and vCPU count used
for the family quota lookup; when the SKU read does not succeed, the quota check
is skipped as `prerequisite_failed` rather than guessing a family.

## Redaction and privacy

Normal output never includes raw account payloads, raw CLI errors, tokens or
resource identifiers. Interpretation extracts only the fields it evaluates,
error output is reduced to a validated error code, and every string in the
report is passed through redaction that masks the subscription UUID, other
UUIDs, e-mail addresses, JWT-like tokens and resource-group names. The report
file is created with `O_EXCL` and mode 0600; an existing path is refused before
any command runs. Fixture files are treated as untrusted input and redacted the
same way.

## Limits

- A successful preflight is not readiness. It does not prove create permission,
  actual capacity, SSH reachability, image pull through a managed identity,
  storage qualification, the 180-second boundary or PC-off acceptance. The
  `unverified_gates` list in every report enumerates those obligations.
- The on-worker repository storage qualifier in
  `crates/horizon-core/src/repository_overlay/storage.rs` requires healthy
  journaled ext4 with exact kernel options. No Azure disk SKU, SMB or NFS share or
  advertised durable volume satisfies that by description; only an on-worker
  check can. This tool cannot and does not evaluate it.
- Regional quota entry names come from the documented responses (`ContainerGroups`,
  `ManagedEnvironmentCount`, `ManagedEnvironmentCores`, Compute `cores` and the
  SKU family). The Container Instances cores entry is expected as `StandardCores`,
  which the official sample response does not show; if a live response names it
  differently the check reports `quota_entry_missing` and the requirement list
  needs updating. A missing entry never passes.
- The tests are not yet wired into the CI workflow; adding a step next to the
  existing Python test in the maintainability job is a separate change.
- `--live` reads are bounded but still count against ARM read throttling; the tool
  issues at most about ten requests per run and never retries.
- The operator comparison, spike checklist and evidence rules live in
  [`docs/testing/azure-workspace-readiness.md`](../../docs/testing/azure-workspace-readiness.md).
