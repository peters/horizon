---
procedure: tailnet-node-api
feature: Desktop exact recorded-node HTTP prerequisite
platforms: [linux, macos]
cost: none
destructive: yes
secrets: [desktop tailnet API credential]
owner: peters
---

# Desktop node API test procedure

## 1. Purpose

Test the SDK operation for an exact recorded node ID.
Make sure uncertain responses retain the ID without automatic cloud cleanup.

## 2. Applicability

Use the candidate with the NodeApi SDK addition.
This procedure tests SDK transport only, not its owning cloud-delete integration.
This procedure does not qualify automatic cleanup or native owner approval.
No visible UI change or native GIF applies to this prerequisite.

## 3. Safety

> **CAUTION:** DELETE ONLY THE EXACT TASK-OWNED NODE.
> Node deletion removes access. Preserve every baseline node and unrelated saved binding.

Live checks require an authorized operator and a dedicated test tailnet.
Keep API credentials on the desktop. Never put them in worker state, arguments, environment variables or logs.
This procedure rents no compute. Any provider setup needs its own authorized procedure and cost record.

## 4. Equipment and preconditions

- Candidate source, dependency lock, toolchain and input hashes.
- The existing synthetic HTTP tests and credential-store tests.
- A trusted desktop adapter that calls `NodeApi::delete_recorded` with the original captured receipt.
- An authorized desktop API credential for the exact saved tailnet ID.
- A task-owned node with its actual Self.ID in that receipt.
- Recorded baseline IDs and evidence of original provider compute and storage absence.

The SDK cannot authenticate caller ownership.
The trusted adapter must examine the original receipt before an API request.
If no qualified adapter exists, record the live lane as blocked.
A direct call with an unexamined ID does not qualify this boundary.

## 5. Setup

1. Record the exact candidate source and executable inputs.

   Result: The input record includes generated files and verified Git LFS content that the checks consume.

2. Compare the candidate with any result proposed for reuse.

   Result: Only checks with identical consumed inputs and runtime assumptions are reused.

## 6. Tasks

### 6.1 N01 — Synthetic SDK behavior

1. Run the focused SDK tests.

   ```bash
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-cloud --lib tailnet::api::node
   ```

   Result: Personal and OAuth requests delete only the exact encoded device ID.
   OAuth exchange requests devices:core and tag:horizon-worker.
   Denied, redirected, oversized, offline and 404 responses retain uncertainty.
   OAuth denial differs from timeout, rate limiting and unavailable service.
   Invalid OAuth replies, oversized headers and an expired shared budget prevent DELETE dispatch.

2. Run the existing tailnet regressions.

   ```bash
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-cloud --lib tailnet::
   ```

   Result: Missing managed slots and disabled bindings prevent staged fallback.
   Interrupted recovery preserves the original binding rules. Enrollment credentials remain separate.

3. Run the required repository matrix on the stable candidate.

   Result: Blocking and strict checks pass. Advisory results remain separate.
   SDK tests do not qualify automatic Core, UI, CLI or MCP cleanup.

### 6.2 N02 — First-head and final-head real inputs

1. Record the protected baseline IDs and exact task-owned captured receipt.

   Result: The receipt identifies the saved tailnet, allocation, worker and actual Self.ID.

2. Examine original provider absence evidence through the trusted adapter.

   Result: Compute and storage are absent before the SDK receives the captured ID.
   Unknown ownership or provider state prevents dispatch.

> **CAUTION:** SEND ONLY THE AUTHORIZED DESKTOP API CREDENTIAL.
> The request can remove access to the recorded node. Keep its value inside the desktop client.

3. Run the exact-node request through the trusted adapter.

   Result: HTTP 200 or 204 returns ConfirmedDeleted without a raw response body or credential value.
   Personal and OAuth lanes are recorded separately. An unavailable lane is not a pass.

4. Examine the exact device and protected baseline nodes with the authorized API reader.

   Result: Independent evidence identifies the affected node. Baseline nodes remain unchanged.
   The SDK result alone does not prove durable Core confirmation.

5. Repeat N02 on the final public head.

   Result: The evidence binds both runs to their actual source and executable inputs.

### 6.3 N03 — Missing access and retry limits

1. Run synthetic missing, removed and unavailable API-binding cases.

   Result: Missing or disabled bindings send no HTTP request.
   Unavailable storage returns a fixed retained reason, with no enrollment-key fallback.

2. Run synthetic denied, offline and wrong-tailnet 404 cases.

   Result: The SDK retains uncertainty for the same exact ID.
   Every 404 remains unconfirmed. No inventory, hostname or tag lookup occurs.

3. Examine the future controller integration boundary.

   Result: Retry needs the same original authority. The SDK grants no provider deletion or new agent lifecycle permission.
   Confirmation, exact-ID manual messages and durable deduplication remain separate Core acceptance criteria.

## 7. Pass criteria

- Current SDK and tailnet tests pass with the required matrix.
- Requests use the original saved binding and exact encoded ID.
- Missing, denied, offline and 404 results retain uncertainty.
- Credentials and response bodies remain private.
- Applicable first-head and final-head live checks have exact source evidence.
- No UI, CLI, MCP or automatic cleanup behavior is claimed for this SDK.

## 8. Cleanup

1. Reconcile uncertain task-node results with the original authority record.

   Result: Unknown deletion remains retained. No name-based or broad cleanup occurs.

2. Preserve the original resource ledger and baseline IDs.

   Result: The operator can distinguish confirmed deletion from an unresolved request.

Any remaining provider, node or credential cleanup follows its original authorized resource procedure.
This SDK grants no additional cleanup authority.

## 9. Record of results

Use the [report template](../reports/TEMPLATE.md) for evidence that must remain.
Record new, reused, blocked and non-applicable lanes separately.
Keep credentials, private IDs and raw responses outside the repository.
