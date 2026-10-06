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
| idle period | The value of `idle_stop_minutes` in a profile. | idle timeout, idle limit |
| idle stop | The stop of a worker after an idle period without agent activity. On RunPod, the worker stops itself. On Hetzner, Horizon stops the cloud. | auto stop, auto-stop |
| idle record | The file `/run/horizon-worker/idle.json` on a worker. `horizon-worker-idle --report` prints it. | idle report |
| idle log | The file `/workspace/idle.log` on a worker. The idle watcher writes its lines there. | — |
| host instance | The identity of the Horizon host process that owns the browsers of an agent. Browser tools use it to find the workspace of the agent. It is not a secret. | host ID |
| panel picker | The menu of a cloud that opens a new panel, with the title **Add panel**. | panel menu |
| browser runtime root | The directory in `HORIZON_BROWSER_ROOT` with the private browser state of a host. | browser root |

## Casting

| Name | Meaning | Do not use |
|---|---|---|
| receiver | A Google Cast device, for example a TV with Chromecast built-in. | Chromecast (for the device), cast target |
| sender | An application that starts a cast on a receiver, for example a phone app or the live example. | client |
| live cast | One `LiveCast` session that streams H.264 from the host to a receiver. | mirror, stream session |
| transport | How a live cast sends media: progressive (one fragmented MP4 response) or HLS. | protocol, mode |
| lag | The time between the newest frame on the host and the frame that the receiver shows. | latency, delay |
| live example | The `horizon-chromecast` example program `live`. | probe, demo |

## Technical verbs

| Verb | Meaning |
|---|---|
| deploy | Put a candidate image and source on a worker and start it. |
| provision | Make a new provider resource, for example a server or a volume. |
| enroll | Join a worker to a tailnet. |
| reconcile | Compare the local record with the provider and correct the local record. |
| squash-merge | Merge a pull request as one commit. |
| freeze | Copy a candidate to a task-owned directory and record its hash. |

## Video capture

| Name | Meaning | Do not use |
|---|---|---|
| WebM | The video file format used for browser and VNC capture. | — |
| AV1 | The video codec used in a WebM recording. | — |
| recording | A temporary video file from a panel's decoded image source. | — |
