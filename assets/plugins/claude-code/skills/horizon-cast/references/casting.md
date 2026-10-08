# Casting reference

`cast` operations are `discover`, `sources`, `status`, `paired`, `forget`,
`start`, `pair`, and `stop`.

| Operation | Use |
|---|---|
| `discover` | Start asynchronous receiver discovery. Read `status` for results. |
| `sources` | List typed sources, availability, and `requires_user_approval`. |
| `status` | Read discovery and session state, frame counters, encoder, and errors. |
| `paired` | List remembered receivers, including offline TVs. Keys are absent. |
| `start` | Start one receiver with a returned source. |
| `pair` | Submit the person's onscreen PIN for that receiver. |
| `stop` | Stop that receiver's session. Repeated stops are safe. |
| `forget` | Delete that receiver's saved pairing. An active session prevents it. |

Use returned receiver IDs and source objects. Sources have `kind: panel`,
`kind: workspace` or `kind: cloud` with an `id`, or `kind: application` without an ID.
Panel, workspace and cloud sources belong to the calling agent's workspace.
A workspace source includes its cloud cards. A cloud source's `id` is the cloud ID
that `cloud_companions` reports; it shows that cloud card with its panels and open drawer.
Application capture contains the main Horizon window and its dialogs. It excludes
the desktop and detached windows. The person's grant applies to one workspace,
is not saved, and can be revoked. Revocation stops agent-controlled application casts.

`start` accepts `orientation: landscape|portrait` and `resolution: 720p|1080p|4k`.
Defaults are landscape and 1080p. Higher resolution cannot restore missing source detail.
One receiver has at most one session. Different receivers can run concurrently.
Do not replace another task's session to solve a conflict.

After `start`, read `status` at a bounded interval. When state is `pin_required`,
tell the person immediately and request the onscreen PIN. Never guess a PIN.
Saved pairing is reused. If rejected, explain the error before an authorized
`forget` and new pairing. Stop on an unexplained error instead of retrying mutations.
Transmission and frame counters prove output from Horizon, not display on the TV.
Report receiver display evidence separately.
