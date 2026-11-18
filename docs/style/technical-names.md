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
| secrets file | The one SQLite file of a Horizon home that holds every secret value of that home: `<Horizon home>/secrets.sqlite`. | keyring, Keychain, credential store |
| scripted agent | An HTTP agent in a test that answers each request from a list of expected requests and answers. It sends nothing to a network. | mock server |
| stand-in recorder | The script `scripts/device-smoke/speech_standin_recorder.py`. It copies one prepared recording to the file of a take and waits for `SIGINT`. It opens no audio device. | fake microphone |
| stand-in service | The script `scripts/device-smoke/speech_standin_service.py`. It answers transcription requests on a loopback address, until the run stops it or its count of requests is done, and it records what it received last, without the key. | mock service |

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
| speech input | The hotkey, the recorder program and the transcriber of one text that Horizon inserts. | dictation, voice typing |
| take | One recording of speech input, from the hotkey down to the transcribed text. | recording (for the take), dictation |
| recorder | The one command line with `{file}` that records a take. | microphone client |
| transcriber | The transcription service or the transcription command of one take. | speech engine, model |
| state mark | The small pill at the bottom of the panel of a take, with its state and its sentences. | badge, indicator |
| **Speech** tab | The tab of the Settings window that holds speech input. | dictation settings |
| endpoint | One IPv4 host or DNS name and one TCP port. | network grant |
| Remote Hosts overlay | The SSH host chooser. | remote chooser |
| toolbar Menu | The **Menu** button at the right of the toolbar and its menu. It holds Quick Nav, Sessions, Remote Hosts and Settings. | More menu, overflow menu |
| New workspace button | The **New** button in the sidebar header next to **Workspaces**. It makes an empty workspace on this computer. | New menu, + Workspace |
| host instance | The identity of the Horizon host process that owns the browsers of an agent. Browser tools use it to find the workspace of the agent. It is not a secret. | host ID |
| hidden window | The main window after a close with **Keep Horizon running**. The programs, the server link and the shared terminals continue. | background window, minimized window |
| browser runtime root | The directory in `HORIZON_BROWSER_ROOT` with the private browser state of a host. | browser root |
| browser tools | The `browser_*` MCP tools of an agent. | browser MCP |
| browser host | The machine whose browser serves a browser panel. The settings entry `browser_host` names its SSH target, or a machine of the machine list, and one profile. | profile host, remote browser |
| browser tunnel | The one `ssh -N -L` process that forwards the loopback `DevTools` port of the browser of a host to a loopback port of this computer. | port forward, relay |
| attached panel | A browser panel whose Chromium runs on a browser host. Its tools, frames and input are the ones of a local panel. | remote panel |
| browser profile | The folder `~/.horizon/browser-profiles/<profile>` on a browser host, and the sign-ins in it. The pages of one profile share its cookies. | profile (alone) |
| host file | The JSON file that `--config` gives to a standalone `horizon-browser`. It holds no credential value. | config file |
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

## Shared terminals

| Name | Meaning | Do not use |
|---|---|---|
| shared terminal | A terminal panel of one computer that the other computers of the same server can show and type in. | remote terminal, mirrored terminal |
| owner | The computer that runs the program of a shared terminal. | host, source |
| follower | A computer that shows a shared terminal of another computer. | viewer, watcher, client |
| follow panel | The panel of a follower that shows a shared terminal. It starts no program. | mirror panel, remote panel |
| sync link | The link of a computer to its server that carries the machine list, the secrets and the settings. | store link |
| live link | The second link of a computer to its server. It carries the shared terminals. | terminal link, stream link |
| screen | For a shared terminal: the bytes that draw the grid of the terminal and set its modes. The output after a screen continues it. | snapshot, replay |
| hub | The part of the server that relays the frames of the live links. | broker, router |
| Computers section | The section **Computers** of the sidebar. It lists the other computers of the server and the terminals that each one shares. | peers section, shared section |
| chip | The short text in a small frame at the right of the title of a panel: `seen by <name>`, `typed by <name>`, `sized by <name>`, `on <name>` or `on <name> · view only`. | label, tag |
| line of an ended stream | The line below the grid of a follow panel whose stream ended. It gives the cause and says that typed text is not sent. | error line, status line |
| sign of life | The `heartbeat` frame that each side of a live link sends while it has no other frame for 10 seconds, and the 30 seconds without a frame that end the link. | keepalive, ping |
| history of a follower | The lines above the viewport that the program of the owner wrote before the follower joined. A fresh screen carries them. | scrollback of a follower, backlog |

## Technical verbs

| Verb | Meaning |
|---|---|
| poll | Send the same read request at intervals until a state changes. |
| share | Put a terminal panel in the list that the other computers of the server get. |
| follow | Show a shared terminal of another computer in a follow panel, and get its screens and its output. |
| relay | Give the bytes or the frames of one link to another link, and store none of them. |
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
