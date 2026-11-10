# Technical names and verbs

Use these names in Horizon procedures, reports and guides. Use one name for one
thing. Write a UI label exactly as the UI shows it, in bold, for example
**Sessions**. Add a new name here before you use it in a procedure.

## Test equipment

| Name | Meaning | Do not use |
|---|---|---|
| candidate | The exact Horizon executable under test. A candidate has a commit and a SHA-256. | build under test, binary |
| frozen candidate | A copy of the candidate in a task-owned directory. Nothing changes it during the test. | — |
| isolated desktop | A task-owned desktop with its own display, input and private application state. On Linux, it is an Xvfb display with its own window manager and D-Bus. On macOS, it is a dedicated machine, VM or desktop session. | test desktop, sandbox desktop |
| fixture | The script that starts the isolated desktop and the candidate. | harness, lab |
| candidate window | The operating-system window of the candidate on the isolated desktop. | Horizon window, app window |
| state path | The directory of one run, given to the fixture with `--state`. It holds the private home. | state dir, evidence dir |
| private home | The home directory that the fixture gives the candidate inside the state path. | sandbox home, fake home |
| session folder | The directory of one session under `.horizon/sessions` in the private home. | session dir |
| Device panel | A Horizon panel that shows a VNC desktop. Agents can only read it. | native viewer, Device viewer, VNC viewer |
| live view | A Device panel that shows the isolated desktop with frames that advance. | — |
| lane | One platform path through a procedure, for example "macOS lane". | track, leg |
| run | One execution of a procedure on one candidate. | pass (as a noun for a run), session |
| evidence | Screenshots, recordings, logs and hashes from a run. Keep private evidence out of the repository. | proof |
| fixture terminal | A local terminal panel of the candidate. Its commands run inside the fixture. | — |
| synthetic repository | A Git repository that a run makes. It contains only test content. | test repo |
| operator | The person who runs the procedure and owns the provider accounts. | tester, user |
| Secret Service | The D-Bus service that keeps the keys of applications on Linux, for example `gnome-keyring-daemon`. | keyring (as a service name) |
| scripted agent | An HTTP agent in a test that answers each request from a list of expected requests and answers. It sends nothing to a network. | mock server |

## Horizon objects

| Name | Meaning | Do not use |
|---|---|---|
| board | The Horizon canvas that holds workspaces and panels. | canvas (in procedures) |
| workspace | A named group of panels on the board. | — |
| menu search field | The search control at the top of a menu. The **Move to Workspace** menu uses one. | search bar, default text frame |
| panel | One terminal, agent, browser or device area on the board. | window, pane, tile |
| session | The saved state of a board. An ephemeral session is not saved. | profile |
| Sessions dialog | The dialog of **Menu** › **Sessions**. It lists the saved sessions of the candidate. | session manager, session window |
| session card | One session in the **Sessions** dialog, with its label, its counts and its **Delete** button. | session row, session entry |
| backdrop | The dim rectangle that a dialog paints over the board behind it. | overlay, scrim, dim rectangle |
| provider | A vendor that Horizon uses for a remote target, for example BrowserStack. | vendor, backend |
| endpoint | One IPv4 host or DNS name and one TCP port. | network grant |
| Remote Hosts overlay | The SSH host chooser. | remote chooser |
| toolbar Menu | The **Menu** button at the right of the toolbar and its menu. It holds Quick Nav, Sessions, Remote Hosts and Settings. | More menu, overflow menu |
| New workspace button | The **New** button in the sidebar header next to **Workspaces**. It makes an empty workspace on this computer. | New menu, + Workspace |
| host instance | The identity of the Horizon host process that owns the browsers of an agent. Browser tools use it to find the workspace of the agent. It is not a secret. | host ID |
| browser runtime root | The directory in `HORIZON_BROWSER_ROOT` with the private browser state of a host. | browser root |
| browser tools | The `browser_*` MCP tools of an agent. | browser MCP |
| browser host | The machine whose Chromium serves a browser panel. The settings entry `browser_host` names its SSH target and one profile. | profile host, remote browser |
| browser tunnel | The one `ssh -N -L` process that forwards the loopback `DevTools` port of the browser of a host to a loopback port of this computer. | port forward, relay |
| attached panel | A browser panel whose Chromium runs on a browser host. Its tools, frames and input are the ones of a local panel. | remote panel |
| browser profile | The folder `~/.horizon/browser-profiles/<profile>` on a browser host, and the sign-ins in it. The pages of one profile share its cookies. | profile (alone) |
| host file | The YAML file that `--config` gives to a standalone `horizon-browser`. It holds no credential value. | config file |
| connect client | The process `horizon-browser mcp --connect`. It gives the browser tools of a host to one agent. | MCP proxy |

## Install and build

| Name | Meaning | Do not use |
|---|---|---|
| release build | A Horizon executable from a GitHub release or Homebrew. It has the default features only. | prebuilt, official build |
| source build | A Horizon executable that you build with `cargo` from the repository. | local build, dev build |
| feature | A Cargo feature that adds a function at build time, for example `trace-profiling`. | flag, option |
| platform | One operating system that Horizon supports: Linux or macOS. | OS (in text), target |
| platform support | The list of functions that work on each platform, in `docs/platform-support.md`. | compatibility matrix |
| welcome board | A planned sample workspace that opens on the first start. | tour, onboarding board |
| doctor | A planned command that examines this computer and reports what Horizon needs. | health check, diagnostics |

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

## UI code

| Name | Meaning | Do not use |
|---|---|---|
| UI thread | The thread that runs the Horizon window and calls `App::ui`. It is the main thread of the process. | render thread, GUI thread |
| frame | One call of `Context::run_ui` that lays out and paints the window. It runs one or more passes and paints only the last one. | tick (for a frame) |
| pass | One run of `App::ui` in a frame. A frame usually has one pass. egui runs one more pass, up to `Options::max_passes`, when code calls `request_discard`. | frame (for a pass) |
| worker thread | A thread that does blocking work for the UI and sends the result back. | background task |
| generation | A number that the UI increases when a request starts or its context changes. A result with an older generation is dropped. | epoch, version |
| fence | A value that tells a worker thread to wait for earlier work, or to write nothing when its board was replaced. | barrier |

## Technical verbs

| Verb | Meaning |
|---|---|
| poll | Send the same read request at intervals until a state changes. |
| squash-merge | Merge a pull request as one commit. |
| freeze | Copy a candidate to a task-owned directory and record its hash. |
| bind | Make a host path available at a path inside the fixture. |
| pin | Record a host key or a commit as the only accepted value. A client then refuses a different value. |

## Video capture

| Name | Meaning | Do not use |
|---|---|---|
| WebM | The video file format used for browser and VNC capture. | — |
| AV1 | The video codec used in a WebM recording. | — |
| recording | A temporary video file from a panel's decoded image source. | — |
