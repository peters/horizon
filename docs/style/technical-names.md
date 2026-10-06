# Technical names and verbs

Use these names in Horizon procedures, reports and guides. Use one name for one
thing. Write a UI label exactly as the UI shows it, in bold, for example
**New cloud…**. Add a new name here before you use it in a procedure.

## Test equipment

| Name | Meaning | Do not use |
|---|---|---|
| candidate | The exact Horizon executable under test. A candidate has a commit and a SHA-256. | build under test, binary |
| frozen candidate | A copy of the candidate in a task-owned directory. Nothing changes it during the test. | — |
| isolated desktop | A task-owned desktop with its own display, input and private application state. On Linux, it is an Xvfb display with its own window manager and D-Bus. On macOS and Windows, it is a dedicated machine, VM or desktop session. | test desktop, sandbox desktop |
| fixture | The script that starts the isolated desktop and the candidate. | harness, lab |
| Device panel | A Horizon panel that shows a VNC desktop. Agents can only read it. | native viewer, Device viewer, VNC viewer |
| live view | A Device panel that shows the isolated desktop with frames that advance. | — |
| lane | One platform or provider path through a procedure, for example "Hetzner lane". | track, leg |
| run | One execution of a procedure on one candidate. | pass (as a noun), session |
| evidence | Screenshots, recordings, logs and hashes from a run. Keep private evidence out of the repository. | proof |

## Horizon objects

| Name | Meaning | Do not use |
|---|---|---|
| board | The Horizon canvas that holds workspaces and panels. | canvas (in procedures) |
| workspace | A named group of panels on the board. | — |
| panel | One terminal, agent, browser, device or cloud area on the board. | window, pane, tile |
| session | The saved state of a board. An ephemeral session is not saved. | profile |
| cloud | A Horizon cloud panel and its remote worker, storage and sessions. | cloud workspace, cloud panel instance |
| worker | The remote machine or container that runs a cloud. | pod, server, VM (use these only for provider objects) |
| profile | A named entry under `profiles:` in `.horizon/cloud.yml`. | flavor, preset |
| provider | A compute vendor that Horizon supports, for example RunPod or Hetzner. | vendor, backend |
| offer | One worker type with a price from a provider catalog. | quote, SKU |
| tailnet | A Tailscale network that Horizon joins with an auth key. | tailscale network, overlay |
| auth key | A Tailscale key that starts with `tskey-auth-`. It is a secret. | token, join key |
| companion | A second repository that a cloud can use. | sibling repo, linked repo |
| sibling | A companion on the same worker as the cloud. | same-worker companion |
| companion cloud | A companion on its own worker. | — |
| Local Network Bridge | The function that lets a worker reach the local network of the PC. | LNB, network share |
| Remote Hosts overlay | The SSH host chooser. | remote chooser |

## Casting

| Name | Meaning | Do not use |
|---|---|---|
| receiver | A Google Cast device, for example a TV with Chromecast built-in. | Chromecast (for the device), cast target |
| sender | An application that starts a cast on a receiver, for example a phone app or the live example. | client |
| live cast | One `LiveCast` session that streams H.264 from the host to a receiver. | mirror, stream session |
| transport | How a live cast sends media: progressive (one fragmented MP4 response) or HLS. | protocol, mode |
| lag | The time between the newest frame on the host and the frame that the receiver shows. | latency, delay |
| live example | The `horizon-chromecast` example program `live`. | probe, demo |

## Native app tests

| Name | Meaning |
|---|---|
| App Automate | BrowserStack's service for native apps on physical devices. |
| native host | The packaged Horizon process that owns native app operations. |
| client file | Private host configuration with the project, owner and state directory. |
| matrix | The complete set of declared physical devices and operating systems. |
| backend | A project's local service with a separate synthetic namespace for each lane. |
| guardian | A private child process that stops an owned command when its parent exits or its lifetime ends. |
| upload | An app artifact that the native host owns at the provider. |
| cleanup receipt | A private record of the result after an owned resource stops. |
| artifact | An immutable IPA or APK file declared by the project. |
| lane | One device with its own synthetic backend, port and namespace. |
| quota | The current provider account capacity for native device sessions. |
| nonce | The host's private value that binds one cleanup request to its acknowledgement. |
| ref | A short-lived element reference from one session's native snapshot. |
| snapshot | The normalized native accessibility tree returned by the host. |
| archive | A private directory that retains one run's report and evidence. |
| MCP | Model Context Protocol, the typed tool interface to the native host. |
| CLI | The command-line interface to the same native host. |
| RFB | The read-only viewer transport between the native host and Device panel. |
| NDJSON | One JSON object per line in the progress stream. |

## Technical verbs

| Verb | Meaning |
|---|---|
| deploy | Put a candidate image and source on a worker and start it. |
| provision | Make a new provider resource, for example a server or a volume. |
| enroll | Join a worker to a tailnet. |
| reconcile | Compare the local record with the provider and correct the local record. |
| squash-merge | Merge a pull request as one commit. |
| freeze | Copy a candidate to a task-owned directory and record its hash. |
