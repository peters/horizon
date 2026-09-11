# RunPod retained Shell-view reconnect — temporary Linux smoke plan

This is an existing-view prerequisite, not workspace creation, Git preparation,
task start, cross-session Open, Stop or Delete UI. Run against the final candidate
after local review and gates. Record HEAD/tree, binary hash and runtime inputs.
No cloud allocation or credential access is authorized by this document.

## Fixture and isolation

- Use a task-owned Linux display/WM, exact candidate debug binary, isolated config
  and Horizon home, unset HORIZON, and no user session/process changes. Record PID.
- Seed one owned persistent RunPod allocation and its completed retained pin/key,
  immutable HPS selection where applicable, an existing saved SSH view, and a
  running saved Shell command with an execution counter. Keep a second unrelated
  task alive. The saved view refers to the same owning session/workspace/panel.
- Supply `remote.runpod` with the exact saved profile name and existing non-secret
  RunPod placement fields. Empty/default config creates no profile. The fixed
  controller variable RUNPOD_API_KEY is supplied only through an approved isolated
  child environment; never config YAML, command arguments, screenshots or logs.
- A synthetic provider/SSH fixture proves only its injected paths. Real RunPod
  attachment and an actual API credential require separate explicit authorization.
  Record which lane ran; do not label synthetic HTTP/SSH proof as cloud acceptance.

## Visible primary path

1. Launch, capture screenshot and verify existing local workspaces remain intact.
2. Open Remote Environments, select the saved RunPod row, list existing views,
   and explicitly reconnect the disconnected saved Shell view.
3. Verify the same panel is replaced only with its pinned local transport; no new
   worker, task, key, repository or saved Ready promotion occurs. Check actual
   shell input/output and resize propagation, not only a success notice.
4. Verify original task PID/session, counter, worker identity, repository dirty
   bytes, selection and allocation are unchanged; unrelated task continues.
5. Disconnect only the task-owned local view, reconnect and repeat. For an exited
   saved task, reconnect may observe its exit but must never relaunch it.
6. Capture after resize/fit and inspect layout, focus, panel identity and terminal.

## Refusals and asynchronous fences

- Missing/invalid credential: fixed missing-key notice, no private value or raw
  provider error. Absent/exact-case-mismatched/duplicate profile fails before I/O.
- Foreign owner, changed summary, missing panel, unsupported saved intent,
  management in flight, timed allocation, missing key/pin, corrupt or conflicting
  storage selection: no credential/provider/SSH access where locally detectable.
- Wrong host key, changed worker/volume/datacenter, unavailable SSH/API, missing
  worker/task: fail closed; no fallback, initial pin, ensure, replay or mutation.
- While pending, change row, close page, refresh, change exact RunPod profile,
  remove target view or switch owning session. Late result must be discarded and
  only its local transport closed; unrelated tasks and views remain untouched.
- Repeated clicks remain single-flight. No I/O from idle/pointer-only frames;
  measure idle and scripted hover across empty canvas and panel chrome.

## Compatibility and cleanup

- Old/empty config round-trips without migration or implicit selection. Existing
  Local Docker reconnect still works; Azure and other configured operations stay
  unsupported as before. Global macOS/Windows CI is unchanged; this path is Linux.
- Reload/relaunch the isolated client with saved views and repeat owner admission.
- Record all pass/fail/pending lanes and sanitized screenshot/log hashes. Restore
  no global config. Close the exact owned window normally and verify PID exit.
  Remove only resources explicitly owned by this smoke under separate authority.
- Remove this temporary plan only after completed UI validation, or retain if
  requested. Do not claim complete cloud workflow acceptance from reconnect alone.
