---
procedure: tailnet-tagged-enrollment
feature: tagged worker tailnet enrollment
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Tagged tailnet enrollment test procedure

## 1. Purpose

This procedure tests the marker 4 gate and one-time worker tag checks with
synthetic local inputs. A separate optional lane tests a real tailnet.

## 2. Applicability

- Candidate: the host and helper for issue #1335, milestone M0a.
- Platform: Linux for the helper; supported Unix cloud hosts for the host contract.
- The required lane uses synthetic keys and status output only.
- This procedure does not test continuous isolation, node deletion, workspace
  tags, policy changes, Mosh, UDP, Funnel or public service exposure.
- There is no visible UI layout change. Contract errors use the existing error path.

## 3. Safety

The required lane creates no provider resources and sends no real credentials.
The optional real-tailnet lane requires separate authorization.
Do not change production keys, policy, nodes or the computer's Tailscale daemon.

## 4. Equipment and preconditions

- A checkout of the candidate commit.
- Python 3 and the Rust toolchain of the repository.
- A private evidence directory outside the repository.
- For the optional lane: an explicitly authorized, task-owned worker and a
  dedicated test tailnet. The image contains marker 4.
- For the optional lane: a preauthorized, non-ephemeral auth key with only
  `tag:horizon-worker`. The policy defines the tag in `tagOwners`.
- Any provider cost cap, cleanup permission and credential transfer need separate
  authorization before the optional lane starts.

## 5. Setup

1. Record the candidate commit and tool versions in the private evidence directory.

   Result: The record identifies the exact inputs of the run.

2. Open a terminal at the root of the candidate checkout.

   Result: The commands below use the candidate source.

## 6. Tasks

### 6.1 T01 — Check the host gates

1. Run the host contract tests.

   ```sh
   cargo test -p horizon-core worker_contract
   ```

   Result: Marker 4 qualifies. Markers 1, 2, 3 and malformed markers do not qualify enrollment.

2. Run the tailnet integration tests.

   ```sh
   cargo test -p horizon-core tailnet
   ```

   Result: Allocation, reconnect and session tests reject a selected tailnet without marker 4.
   **None** remains compatible with older images.

3. Run the quick-start tests.

   ```sh
   cargo test -p horizon-core quick_start
   ```

   Result: The current pin remains unchanged. It refuses selected-tailnet use until the marker 4 image rollout.

### 6.2 T02 — Check synthetic worker transitions

1. Run the worker tests.

   ```sh
   python3 -B -m unittest discover -s examples/cloud-worker -p test_tailnet.py
   python3 -B -m unittest discover -s examples/cloud-worker -p test_check_markers.py
   ```

   Result: Synthetic enrollment requests only `tag:horizon-worker`.
   The helper accepts exactly that tag after `up` and once at each resume.
   Missing, malformed, untagged and extra-tag status fails.
   Refused nodes attempt logout. The error names the tag and `tagOwners`.
   The helper and checker report only `horizon-tailnet-contract=4`.

2. Examine the synthetic key and persistent identity test results.

   Result: The key never reaches command arguments or retained files.
   A tagged resume retains the node and device name without another key.
   An older untagged node logs out and fails resume.

3. Examine the supervisor test results.

   Result: The tag check occurs once per resume. No periodic tag watcher or
   persistent revocation marker is required.

### 6.3 T03 — Check bundled guidance

1. Run the skill coverage check.

   ```sh
   python3 scripts/check-horizon-mcp-skills.py
   ```

   Result: Both bundles agree. They describe marker 4, `tagOwners`, one-time
   checks and the quick-start rollout limit.

### 6.4 R01 — Optional real-tailnet smoke

This lane is not part of the local test result. Record **not run** without
explicit authorization for its resources and access changes.

1. Record the authorized worker and test tailnet identifiers privately.

   Result: Cleanup can identify only the resources of this run.

> **CAUTION:** ENROLL ONLY THE AUTHORIZED TEST WORKER. This step sends the auth
> key and changes tailnet access. A running provider worker can cost money.

2. Enroll the worker through Horizon with the tagged test key.

   Result: The cloud reaches Ready. Its own `Self.Tags` contains exactly
   `tag:horizon-worker`.

> **CAUTION:** RESUME ONLY THE AUTHORIZED TEST WORKER. The provider can charge
> for the resumed worker. The helper checks and can remove its tailnet access.

3. Stop and resume the test cloud through Horizon.

   Result: The tagged node retains its identity and device name without another key.

4. Record the permitted reachability from the test policy.

   Result: The report separates policy reachability from the one-time tag check.
   It makes no claim about continuous monitoring or workspace isolation.

## 7. Pass criteria

- T01, T02 and T03 pass on the candidate inputs.
- The current quick-start pin does not claim marker 4.
- A local result does not claim real provider or tailnet qualification.
- If R01 runs, its report records the authorization, image digest and observed tags.

## 8. Cleanup

1. Keep the local logs in the private evidence directory.

   Result: The repository contains no keys or private network details.

> **CAUTION:** DELETE ONLY RESOURCES THAT R01 RECORDED AND THE OPERATOR AUTHORIZED.
> Other workers and devices belong to other people.

2. If R01 ran, delete its authorized worker and remove its test node manually.

   Result: The provider no longer charges for the worker. The test node is absent.
   Cloud deletion alone does not remove the tailnet node in M0a.

## 9. Record of results

Put local command results and R01's separate status in the pull request.
Use [the report template](../reports/TEMPLATE.md) when the run must be kept.
Keep private evidence outside the repository.
