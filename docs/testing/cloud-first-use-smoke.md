# Cloud first-use acceptance

This retained plan covers the requested setup UI in the Cloud Workspaces MVP.
Automatic native-view verification now works through a separate ephemeral viewer.
Interactive acceptance is in progress on the isolated candidate.

## Design contract

Use Horizon's existing theme tokens, not a second visual system. The dark palette
is canvas #070a10, elevated surface #0c1018, field surface #161b27, text #e4e9f4,
muted text #aeb9cd and accent #6a90ff. Light mode uses the corresponding theme
functions. Keep the application's existing proportional font: 26 px modal title,
14–15 px field text and 12–13 px supporting text. Left-align the form, with clear
groups and one primary action. Do not add an issue or device picker.

The global Cloud menu owns New cloud, Cloud settings and Fit all. Remove the
floating Cloud/RunPod control strip from the canvas. Settings uses a compact
modal sharing the New cloud surface, with RunPod access followed by individual
agent choices and API-key/subscription options. Advanced paths and registry
bindings remain available without making them the first-use experience.

```text
Cloud menu                  Cloud settings
  New cloud                 RunPod access       [masked key]
  Cloud settings            Coding agents       [choices]
  Fit all                   Authentication       [API key | Login]
                            Advanced             [collapsed]
                            [Cancel]             [Save settings]
```

A missing repository configuration offers an agent-assisted setup path and
Reload configuration. It must never silently deploy an arbitrary application
image or include uncommitted source. Selecting a profile shows its enabled
capabilities and missing authentication plainly.

## Preconditions

- Freeze the exact candidate and verify the actual application child PID/hash.
- Use a new ephemeral configuration and task-owned isolated home/state. Start
  with no cloud settings, saved keys, repository defaults or inherited agent
  authentication. Do not replace the developer's active configuration.
- View the isolated application through the current workspace's native Device
  panel. Follow the automatic health procedure; do not request human visibility
  confirmation. Record blocked lanes without claiming a pass.
- Record the native desktop at 4K, then inspect decoded video frames. Keep all
  credentials masked and application-specific captures private.

## Acceptance scenarios

1. Open Cloud from a fresh instance. New cloud leads to the missing-account
   setup route without a hidden-file prerequisite. Cancel and Escape allocate
   nothing; keyboard focus stays contained and returns to the previous view.
2. Enter a compute key and choose one or both supported agents. Configure API
   authentication for one and subscription login for the other. Save once;
   verify private machine-local files, no secret in YAML, logs, launch arguments,
   persisted panel data or clipboard. Blank replacement fields preserve saved
   credentials. Failed saves report errors and preserve the previous settings.
3. Verify selected agents alone require credentials. Subscription authentication
   opens the real worker CLI login flow and never claims an API key enables a
   subscription. Explain when a browser-assisted login needs another device.
4. Exercise an existing cloud.yml and then a repository without one. Start its
   setup in a real local agent panel, prepare the worker contract and YAML, reload
   it, and verify profile selection. No source changes are silently committed or
   sent remotely. A malformed/replaced config invalidates stale profile choices.
5. Create a cloud and explicitly deploy. Verify measured stage activity, elapsed
   time, transfer speed and ETA only when measurable. Ready retains the measured
   time to worker readiness; it is not called application-visible startup time.
6. Verify narrow and 4K viewports, scaling, resize/fit, nested fullscreen and
   Escape. The Cloud menu remains accessible through toolbar overflow, settings
   scroll without hiding their actions, and no controls overlap.
7. Restart only the task-owned instance. Settings bindings, authentication modes,
   cloud membership and worker/session identities survive. Repeat failure,
   cancellation and reconnect lanes; do not allocate a duplicate worker.
8. Recheck legacy settings/profile loading, disabled agents/browser capabilities,
   multiple instances, remote browser settings and no-default-features builds.
9. Complete the full repository matrix, independent review and current-head
   hosted checks; repeat affected native smoke after behavior changes. Release
   task devices before removing compute, verify provider absence, then revoke
   task registry bindings. No automatic merge or release.

## Current status

Design reviewed against the existing New cloud treatment: the interface reuses
Horizon typography and surfaces, keeps the canvas for panels, and avoids adding
an unrelated dashboard or provider picker. Implementation and focused headless
checks are complete; exact-candidate native acceptance is pending. The setup
agent uses existing local authentication, disclosed in the form; remote API
bindings are not exported automatically into a local process. Playground
preparation is removed at the user's request.


Checkpoint 21 September 09:15 UTC: native missing-account routing, cancel/Escape,
masked synthetic credentials, mixed authentication preferences, blank-save
preservation and missing-YAML guidance passed. No compute was allocated. The
800x600 resize exposed a footer-sizing defect; corrected full-app regressions
pass and independent review is clear. Repeat the native resize on the corrected
binary before marking that lane passed. Actual local setup-agent execution,
remote authentication, profile reload and cloud/session persistence remain open.

Checkpoint 21 September 10:11 UTC: corrected dialogs passed native 4K/900x700/800x600
resize, scrolling and toolbar-overflow checks. Settings and synthetic bindings
survived a normal isolated restart. The real local setup agent authenticated,
created a minimal fixture profile, and passed its unit/static checks. New cloud
loaded the resulting YAML; malformed YAML cleared the profile and disabled Create,
and restoring the file recovered loading. The image was a labelled placeholder;
no build or deployment is implied. Private local agent auth was removed. Final
remote authentication, cloud/session persistence and post-review smoke remain open.


### Setup from an existing cloud workspace

With a cloud workspace active, open Cloud > New cloud, select a local repository
without YAML and choose Open setup agent. Verify the real setup panel opens in an
ordinary local workspace, has the selected repository as its working directory,
and is not a child of the existing cloud. Repeat with a local workspace already
available: it should be reused. A legacy remote workspace must never be selected.
The setup action must not allocate compute or inherit remote credentials.

Attempt to move an ordinary panel into a cloud workspace through both the sidebar
and minimap. Membership and layout must remain unchanged. Removing an ordinary
workspace must select another compatible ordinary workspace or leave it intact if
only cloud destinations remain; no panel may become orphaned.


Creation responsiveness and cancellation regression:

- Select a committed local repository/profile, create a cloud, and verify the
  resulting undeployed card retains the selected profile and committed revision.
  Creating the card must not allocate compute.
- During a slow repository check, verify the modal keeps painting, inputs and
  duplicate Create are disabled, and Cancel/Escape remain usable. A result queued
  in the cancellation frame must not create a card afterward.
- Retry after cancellation. Only one filesystem/Git inspection may remain active;
  a still-stopping check gives a retryable explanation. Git inspection is bounded
  to 30 seconds and cancels its task-owned process group.
- Switch sessions or remove/detach the captured workspace before completion: no
  late result may create a cloud in a different workspace/session.
- Close before the first cloud preparation frame, and immediately after switching
  sessions: existing saved groups must survive. Removing the last initialized
  cloud must still persist an empty group list.

Compatibility and standalone build regressions:

- Run `cargo test -p horizon-core --no-default-features` as well as the normal
  feature-enabled matrix. Restore a cloud session with cloud support disabled,
  autosave and restore again: opaque metadata, stable panel identities and empty
  cloud workspaces survive, and cloud commands are never launched locally,
  including attempts to restart the inert panel.
- Run the worker Python suite. Dockerfile COPY inputs and .dockerignore exceptions
  must exactly match the context-generator allowlist; similarly named backup or
  credential files are excluded.
- Preparation with an already pinned commit must not inspect Git or the repository
  mount on the UI thread. Invalid/unresolved revisions are rejected; background
  deployment still validates the committed tree before allocating compute.
