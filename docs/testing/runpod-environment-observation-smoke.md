# Retained RunPod environment observation smoke

Temporary validation plan for issue #473. Linux-first. Use only a task-owned
isolated Horizon home, exact candidate binary and native display/window identified
by PID. Do not restart or alter any existing Horizon process or cloud resource.

## Preconditions and scope

- Record candidate commit/tree, binary hash, display and process IDs.
- Seed an owned synthetic RunPod workspace with its reserved public request,
  retained worker and full saved host pin. No private SSH key is required.
- Configure exactly the saved profile. Never include real credentials in screenshots
  or logs. Missing-credential cases must use a clean environment without API keys.
- This action only observes provider metadata. It does not prove SSH, Git, task,
  checkpoint, volume durability or client-off acceptance.

## Baseline and main flow

1. Launch the candidate with isolated config and ephemeral session. Capture its
   initial screenshot and verify existing local inventory remains usable.
2. Open Remote Environments, select the synthetic retained RunPod row, and click
   its existing provider-check action. Verify the fixed missing-key error, no
   worker creation/adoption and no saved-state changes.
3. Repeat explicitly. The single-flight slot must release after failure; idle
   frames, pointer movement and repaint must not initiate provider checks.
4. Resize and fit the window. Capture the selected row, status/error and controls;
   verify no overlap, clipping or accidental Stop/Delete activation.

## Edge and persistence checks

- With a missing database, run the observation path and verify neither database nor
  home directory is created. Check unsupported provider and missing profile too.
- Deterministic core tests cover ready, stopped, failed, unknown and absent exact
  worker, full pin required, wrong profile/storage binding, no private identity,
  expired setup, pending management, bounded ordinary worker lifetime, and stale
  results after credential/provider callbacks. Run these on the candidate head.
- Verify retained summaries and creation claims are unchanged. A provider absence
  must not erase the saved identity; an endpoint mismatch must not replace the pin.
- Existing successful cached observation must retain its timestamp after a failed
  repeat, and selection changes must discard late results.
- Close only the task-owned candidate normally; verify its exit and clean up only
  the display/window manager created by this test. Preserve proof artifacts until
  review completes. Never claim real RunPod success from synthetic/native refusal.

## Optional actual provider lane

Only with a separately verified retained task-owned worker, exact full pin and
approved provider credential: observe Ready/Stopped metadata and verify no store
or resource mutation. Record actual evidence separately; do not allocate or stop
resources merely to satisfy this optional lane. Cloud durability remains separate.
