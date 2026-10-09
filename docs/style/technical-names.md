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
| persistent launcher | A task-owned copy of the fixture that keeps its saved session. It runs the candidate without `--ephemeral`. | persistent fixture |
| restart marker | The file `restart-request` in the state directory of the persistent launcher. When the candidate stops, the launcher finds the file and starts the candidate again. | restart flag |
| fixture terminal | A local terminal panel of the candidate that is not in a cloud. Its commands run inside the fixture. | — |
| worker shell | A Shell panel of a cloud. Its commands run on the worker. | remote terminal |
| synthetic repository | A Git repository that a run makes. It contains only test content. | test repo |
| resource ledger | A private file that records each provider resource that a run makes, with its ID. | inventory (for this file) |
| operator | The person who runs the procedure and owns the provider accounts. | tester, user |
| Secret Service | The D-Bus service that keeps the keys of applications on Linux, for example `gnome-keyring-daemon`. | keyring (as a service name) |
| test tailnet | A tailnet that only tests use. | — |
| contract marker | A line, for example `horizon-tailnet-contract=1`, that a worker image reports to show a function. | — |
| nonce | A random value that a run makes one time. A reply that contains it is current. | token |

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
| device name | The name of a worker in a tailnet. Other devices use it to reach the worker. | hostname, machine name |
| companion | A second repository that a cloud can use. | sibling repo, linked repo |
| sibling | A companion on the same worker as the cloud. | same-worker companion |
| companion cloud | A companion on its own worker. | — |
| SSH alias | The name `companion-<alias>` that a source worker uses to open SSH to a companion cloud. | host alias |
| agent user | The account `horizon-agent` (UID 10001) that runs agent and shell panels on a worker. | agent account |
| Local Network Bridge | The function that lets a worker reach the local network of the PC. | LNB, network share |
| Remote Hosts overlay | The SSH host chooser. | remote chooser |
| idle period | The value of `idle_stop_minutes` in a profile. | idle timeout, idle limit |
| idle stop | The stop of a worker after an idle period without agent activity. On RunPod, the worker stops itself. On Hetzner, Horizon stops the cloud. | auto stop, auto-stop |
| idle record | The file `/run/horizon-worker/idle.json` on a worker. `horizon-worker-idle --report` prints it. | idle report |
| idle log | The file `/workspace/idle.log` on a worker. The idle watcher writes its lines there. | — |
| host instance | The identity of the Horizon host process that owns the browsers of an agent. Browser tools use it to find the workspace of the agent. It is not a secret. | host ID |
| panel picker | The menu of a cloud that opens a new panel, with the title **Add panel**. | panel menu |
| browser runtime root | The directory in `HORIZON_BROWSER_ROOT` with the private browser state of a host. | browser root |
| control service | The worker service `horizon-cloud-worker serve`. It hosts the browsers of a cloud. With agent isolation, it runs as UID 10001. | browser service, worker service |
| agent isolation | The worker mode in which agent panels and workspace services run as UID 10001. The stock worker image starts it. | sandbox |
| browser tools | The `browser_*` MCP tools of an agent. | browser MCP |
| image-only profile | A profile without a `build` section. Horizon uses its image and builds nothing. | — |
| base image | The public CPU worker image `ghcr.io/peters/horizon-worker-base`. Horizon pins it by digest. | default image, stock image |
| quick start | The **New cloud** choice that runs a repository without `.horizon/cloud.yml` on the base image, with the built-in profile `quick-start`. | easy start, default cloud |
| token chain | A GitHub App user access token, its refresh token and their expiry times. Each refresh gives a new token chain and cancels the old one. It is a secret. | token pair |
| chain service | The worker service `horizon-worker-github serve`. It runs as root, refreshes the token chain and gives access tokens to agents. | GitHub daemon |
| GitHub socket | The file `/run/horizon-worker/github.sock` on a worker. Agents ask the chain service through it. | agent socket |
| fake GitHub | A small HTTP server on `127.0.0.1` that answers refresh requests with synthetic tokens. | mock GitHub |

## Install and build

| Name | Meaning | Do not use |
|---|---|---|
| release build | A Horizon executable from a GitHub release, Homebrew or WinGet. It has the default features only. | prebuilt, official build |
| source build | A Horizon executable that you build with `cargo` from the repository. | local build, dev build |
| feature | A Cargo feature that adds a function at build time, for example `speech` or `cast-nvenc`. | flag, option |
| platform | One operating system that Horizon supports: Linux, macOS or Windows. | OS (in text), target |
| platform support | The list of functions that work on each platform, in `docs/platform-support.md`. | compatibility matrix |
| welcome board | A planned sample workspace that opens on the first start. | tour, onboarding board |
| doctor | A planned command that examines this computer and reports what Horizon needs. | health check, diagnostics |

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

## Browser frame tests

| Name | Meaning | Do not use |
|---|---|---|
| Chromium | The local Chromium browser backend. | — |
| Firefox | The local Firefox browser backend. | — |
| Safari | The local Safari browser backend. | — |
| geckodriver | The driver that starts and controls the local Firefox session. | — |
| child frame | A document embedded in another browser document. | — |
| cross-origin frame | A child frame whose origin differs from its parent document. | — |
| HTTP fixture | A task-owned loopback server and synthetic pages for a browser test. | — |
| document reference | A short-lived browser node reference tied to one document. | — |

## Technical verbs

| Verb | Meaning |
|---|---|
| deploy | Put a candidate image and source on a worker and start it. |
| provision | Make a new provider resource, for example a server or a volume. |
| enroll | Join a worker to a tailnet. |
| reconcile | Compare the local record with the provider and correct the local record. |
| squash-merge | Merge a pull request as one commit. |
| freeze | Copy a candidate to a task-owned directory and record its hash. |
| bind | Make a host path available at a path inside the fixture. |
| pin | Record a host key, a commit or an image digest as the only accepted value. A client then refuses a different value. |
| revoke | Remove the access that a credential gives at the provider or at the worker. |
| refresh | Exchange a refresh token for a new token chain at GitHub. |
| forward | Connect a port on the worker to a device through the Local Network Bridge. |

## Video capture

| Name | Meaning | Do not use |
|---|---|---|
| WebM | The video file format used for browser and VNC capture. | — |
| AV1 | The video codec used in a WebM recording. | — |
| recording | A temporary video file from a panel's decoded image source. | — |
