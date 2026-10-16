---
procedure: protected-ingress-reader
feature: Default-off protected host receipt reader
platforms: [linux]
cost: rents compute
destructive: no
secrets: none
owner: peters
---

# Protected host receipt reader test procedure

## 1. Purpose

Test the fixed receipt reader and its original file and namespace checks.
Keep exported facts separate from runtime authority.

## 2. Applicability

- Candidate: the Linux Worker with `protected-ingress` enabled.
- Lane: the original host-root Docker worker on a task-owned VM.
- Rootless workers and user-namespace remapping are unsupported.
- This procedure does not qualify a Gate, source preservation, owner approval or public activation.

## 3. Safety

> **CAUTION:** USE ONLY THE APPROVED WORKER AND COST CAP. The original host procedure owns paid capacity and cleanup.

> **CAUTION:** DO NOT RUN H03 ON THE DESKTOP. H03 requires the genuine original receipt and isolated worker namespaces.

## 4. Equipment and preconditions

- Use a separate task checkout, private HOME, temporary directory and build cache.
- Keep the feature off in default builds and public dispatch.
- Complete source review before qualification.
- Prepare the H03 command plan for the original host procedure.
- Keep the actual VM, Docker CID, image, allocation and resource ledger from that procedure.
- Use a frozen candidate with its compiler inputs and SHA-256.
- Keep raw evidence private.

## 5. Setup

1. Read `docs/architecture/protected-ingress-reader.md`.

   Result: The file, namespace and authority limits are clear.

2. Record the candidate source inventory and build environment.

   Result: The record identifies the exact tested inputs.

## 6. Tasks

### 6.1 H01 — Test pure refusal paths

1. Run the focused offline tests.

   ```bash
   cargo test --locked --offline -p horizon-cloud-worker --features protected-ingress protected_ingress::tests
   ```

   Result: Five pure tests pass. The actual-root test remains ignored.
   Duplicate/unknown receipt fields, wrong allocation, unsafe metadata and writable flags are refused.
   Thread capability, identity-map and init-state failures are refused.

### 6.2 H02 — Complete local qualification

1. Complete the applicable repository matrix on the stable candidate.

   Result: Required blocking and strict tiers pass. Record advisory findings separately.

2. Examine both normal UI candidates for the current paired guidance.

   Result: The complete current guidance is present in both candidates.

3. Examine the new reader's callers.

   Result: No public command, Core operation or Gate starts the reader.
   Every exported fact remains passive.

### 6.3 H03 — Read the original actual-root receipt

1. Approve the exact H03 command plan through the original host procedure.

   Result: The plan identifies the original owned VM, CID, image, allocation and frozen candidate.
   No copied receipt or replacement worker supplies the result.

2. Run the frozen test candidate as the original worker's root process.

   Use this exact test selector with `--ignored --exact --nocapture`:

   ```text
   protected_ingress::tests::actual_original_root_receipt_readback
   ```

   Result: The test exits zero and reports `H03_PASSIVE_RECEIPT`.
   The genuine fixed host receipt supplies the retained allocation after live kernel checks.
   Compare it with the original owned provider journal; no startup capture is created.
   The original file stays UID 0, mode 0400, single-link and read-only.
   The original directory stays UID 0, mode 0700 and read-only.
   Current thread/init capabilities, PID-1 identity, namespaces and sysctl match.

3. Compare the reported facts with the original host observations.

   Result: Allocation, generation, CID, image, init start, namespace and publications match the original receipts.
   This comparison supplies no peer-source or public-readiness result.

4. Repeat H03 on the final head after a consumed-input or behavior change.

   Result: First-reviewable and final evidence cover the actual current candidate.
   Reuse requires the repository input comparison.

## 7. Pass criteria

- H01 through H03 cover the current candidate.
- The original descriptor and namespace checks pass twice within H03.
- No ignored or unexecuted case is recorded as an actual-root pass.
- The result states the source-preservation, runtime and public-interface limits.

## 8. Cleanup

1. Make sure the original test process exits.

   Result: The reader releases its held descriptors.

2. Return resource cleanup to the original host procedure.

   Result: The original resource ledger remains authoritative. This reader creates no resource or credential.

## 9. Record of results

Write retained H03 results with `docs/testing/reports/TEMPLATE.md`.
Record the exact source, frozen candidate, compiler inputs, argv, raw output and original host receipts.
Keep new, reused and non-applicable results separate.
Do not record this component as complete M2 delivery.
