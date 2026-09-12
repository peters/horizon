# Azure saved-repository preparation: bounded Linux UI smoke

## Candidate and safety boundaries

- Record exact committed source head/tree, clean worktree, debug GUI/fixture
  hashes and passing focused/admission tests before runtime binding.
- Use a fresh private home and unused task-owned X11 display with a lightweight
  window manager. Scope every screenshot/input to the exact new PID and window.
- Never use the user's home, Azure login, subscription, credentials or workers.
  No Docker resources, network, image pulls or real repository are required.
- Create only synthetic local public SessionStore/allocation metadata through
  existing APIs: one persistent owning session and already-created Azure worker
  shape, immutable CPU profile binding, explicit saved branch and public host pin.
  This fixture is not evidence of Azure creation, live readiness or retention.
- Do not create a private SSH identity or provider credential. Never submit Git,
  install a PAT, inspect a remote receipt, start/restart/stop/delete compute, or
  exercise provider status/remote task actions. No actual credential transfer.
- Keep provider namespace and external Azure client-off harness unchanged.
- Use isolated networking and existing no-I/O trace controls. Preserve all failed
  intents/screenshots; never replay an uncertain helper or extend its deadline.

## Baseline and primary flow

1. Launch the exact debug candidate once. Record PID/display/start time and a
   finite normal-close deadline, no more than 20 minutes per GUI phase.
2. Capture the empty local board, open Remote Environments, select the saved
   Azure row and verify its saved-not-live status. No action starts automatically.
3. Verify Review repository preparation and Check preparation receipt are
   available for the owning Linux context. Only click Review, not Check.
4. With first-token installation unchecked, Review shows the exact repository,
   commit, saved branch, Azure destination/profile and full retained resource ID.
   Verify the existing no-PAT-delivery and no-task-start disclosures.
5. Cancel. Enable first-token installation and Review again. The field must be
   empty/masked, consent unchecked and Confirm disabled. Do not type a token,
   check consent or click Confirm. Cancel again; no token/consent survives.
6. Close and reopen the overview. Review remains an explicit user action; no
   credential request, Git handoff or remote receipt check is replayed.

## Edge cases and deterministic coverage

- Core tests, not seeded GUI outcomes, prove missing/corrupt profile binding,
  all eight profile-field drift cases, missing private identity, foreign owner,
  stale full snapshot, missing branch/trust and pending Stop/cleanup refusal.
- Existing common tests preserve installation acknowledgement, no rotation,
  post-install ambiguity, consumed confirmation and no automatic replay.
- Synthetic-provider tests prove only existing-worker inspection and exact-pin
  admission into the common Git frame; no SSH/provider operation is claimed.
- Non-Linux remains unsupported through the existing platform gate and CI.
- No migration/backfill occurs; record unchanged fixture allocation/binding
  metadata and continued absence of private identities and remote artifacts.

## Visual and passive checks

- Inspect actual normal-size and 800×600 form/review screenshots. The long ARM
  resource ID must wrap or remain reachable, with Cancel and disabled Confirm
  accessible by scrolling. Do not reduce the native minimum below 800×600.
- Repeat affected disclosures in an isolated light-theme phase after normal
  close; change only this fixture's theme, not provider configuration or identity.
- Exercise keyboard focus without submitting. Separately sample idle, pointer
  movement and bounded resize; counts must show no operational dispatch.
- On an empty board, Fit may be disabled. Record that qualification rather than
  claiming populated-canvas Fit or whole-canvas layout correctness.

## Completion

- Close each exact GUI normally before its original deadline. Verify GUI,
  supervisor-owned children and display sockets are absent, retaining evidence.
- Verify zero network/provider/SSH/Git/PAT operations, unchanged saved metadata,
  no private identity and no newly created remote resources.
- Report each case as pass/fail/not run; disclose fixture and coverage limits.
  This preview-only smoke is not real Azure/GitHub or client-off acceptance.
- After independent evidence review, archive this plan privately and remove only
  this temporary repository plan. Prove production/test/Cargo byte parity before
  the documentation-only final commit and final exact-head validation matrix.
