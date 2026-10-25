# Horizon Browser CLI

The `horizon-browser` executable makes the existing Horizon browser MCP
contract usable from scripts without creating a second action API. It has three
modes:

- a quoted goal (or explicit `do`) lets an optional local agent observe and
  adapt through the MCP tools;
- `run` executes a fail-fast JSON plan with bounded MCP execution and writes a
  structured report to stdout or a private file;
- `mcp` runs the same local stdio MCP server that Horizon registers for agents.
  Outside Horizon it starts and owns an isolated browser automatically.

The crate is intentionally thin and is not published. Browser actions, schemas,
ownership, redacted audit, and backend behavior remain in
`horizon-browser-mcp`, `horizon-browser-control`, and the publishable `horizon-browser`
engine. Binary size, CLI cold start, and semver expectations are recorded in
[`docs/architecture/browser-packaging.md`](../../docs/architecture/browser-packaging.md).

## Choose the interface

- **Describe an adaptive goal:** pass one quoted prompt directly to
  `horizon-browser`. The optional local agent translates it into MCP calls,
  observes results, and adapts; the user does not write JSON.
- **Run a repeatable job:** use `horizon-browser run` with a checked JSON plan.
  This path is deterministic, model-free, fail-fast, and suitable for scripts
  and CI.

Both paths use the same MCP action schemas, ownership rules, steering, and
redacted audit trail. The agent adapter is outside the engine and deterministic
`run` remains model-free.

Through that contract an agent or plan can navigate; snapshot or query the DOM;
click, fill, scroll, wait, and evaluate; supply HTTP Basic or Digest
credentials with `browser_http_auth` when a page challenges; show or hide an
actor-owned panel; read the audit trail; and capture or cursor-watch bounded
HTTP/WebSocket traffic on supported backends. Prompt jobs and checked `run`
plans use that same tool: pass the username and password the user provided
(plan variables are fine). Reports, traces, and the durable `plan.json` copy
redact those fields. Remaining `browser_http_auth` set steps cannot be resumed
from job state; re-run the plan with credentials. `clear` drops live-session
credentials for later challenges; it does not revoke Authorization values the
browser already cached, so open a new panel for a clean unauthenticated
session.

## Shared remote-provider usage

From a Horizon-launched agent or its child process, the CLI plan runner can query
shared provider capacity without opening settings or allocating a browser session.
It uses the same public `browser_provider_usage` MCP tool and the host's configured
credential bindings. A local standalone browser host has no remote-provider
configuration and returns an explicit host-identity error for this operation.

Save this as `provider-usage.json`, then run
`horizon-browser run provider-usage.json`:

```json
{
  "version": 1,
  "steps": [
    {"id": "all", "tool": "browser_provider_usage"},
    {"id": "one", "tool": "browser_provider_usage", "arguments": {"provider": "team_cloud"}}
  ]
}
```

Omitting `provider` queries all configured profiles. Each result includes the
profile name, support status, optional local limit, running/allowed/queued counts,
actual sample time (`sampled_at_millis`, Unix milliseconds), and any provider error.
Failed or unsupported reads return null counts, not zero capacity. Counts are
informational snapshots and do not reserve slots or gate session creation.

For multiple accounts on the same provider, configure separate profiles with
separate credential references, OS-store slots, or environment-variable bindings.
Reusing the same bindings intentionally shares an account. Secrets and provider
endpoints are never tool arguments or usage results.

## Prompt-first jobs

The default command is a quoted goal. It starts an isolated hidden browser,
selects Chromium or Firefox automatically, and prints only concise completed
tool names followed by the final summary and private report path:

```bash
horizon-browser "Go to amazon.com, extract 100 products with price and reviews, save to products.csv"
```

Use `do` as an explicit alias, choose a backend, show the native window, or
emit stable JSONL progress:

```bash
horizon-browser do "Summarize example.com" --backend firefox --visible
horizon-browser "Summarize example.com" --json
```

When the prompt ends with `save to`, `write to`, or `output to` followed by a
relative path, that exact user-supplied path becomes a constrained text
artifact sink. Absolute paths and lexical parent traversal are rejected before
the agent starts; the page and agent cannot choose or expand the authorized
path. Without an authorized path, returned artifact content is rejected.
Diagnostics and structured agent results stay in
an owner-only job directory under `browser-jobs/` in the browser runtime
root (see [Custom browser runtime directory](#custom-browser-runtime-directory)); MCP runtime
profiles use a separate temporary home and are removed after each job.

Every completed prompt job also writes a private redacted `trace.jsonl`, a
validated `executed-plan.json`, and `report.json`. The report includes the
same `observability` summary as deterministic `run` when the agent event
stream carries MCP results. The plan replaces the
standalone panel id with a typed reference to the recorded `browser_list`
result. The report marks the plan non-replayable when execution depended on
ephemeral semantic references or when selectors, scripts, text, values, URL
credentials, query values, or fragments had to be redacted. Tool results and
page contents are not copied into the trace.

The normal workspace build contains no model SDK. Job mode invokes an optional
local agent executable. When `HORIZON_BROWSER_AGENT_COMMAND` is unset, the CLI
uses `grok` if it is on `PATH` and otherwise `codex`. A basename of `grok`
selects Grok's headless flags, isolated `GROK_HOME`, and streaming-json MCP
events; any other basename keeps the Codex `exec` event interface. A missing
adapter is a clear runtime error and does not affect `run` or `mcp`.

## Plan runner

Build the binary, then pass it a plan path or `-` for stdin:

```bash
cargo build -p horizon-browser-cli
target/debug/horizon-browser run plan.json
target/debug/horizon-browser run plan.json --output report.json
target/debug/horizon-browser run plan.json --timeout 300
```

A plan is a sequence of literal MCP tool names and arguments:

```json
{
  "version": 1,
  "steps": [
    {
      "id": "panels",
      "tool": "browser_list"
    },
    {
      "id": "navigate",
      "tool": "browser_navigate",
      "arguments": {
        "panel_id": { "$ref": "panels#/panels/0/panel_id" },
        "url": "example.com"
      }
    },
    {
      "id": "ready",
      "tool": "browser_wait",
      "arguments": {
        "panel_id": { "$ref": "panels#/panels/0/panel_id" },
        "selector": "body",
        "state": "present"
      }
    },
    {
      "id": "title",
      "tool": "browser_evaluate",
      "arguments": {
        "panel_id": { "$ref": "panels#/panels/0/panel_id" },
        "expression": "document.title"
      }
    }
  ]
}
```

An exact `{"$ref":"step-id#/json/pointer"}` value reads typed structured
content from an earlier successful step using RFC 6901 JSON Pointer syntax.
References cannot point forward. An exact `{"$var":"name"}` value reads a
plan-level JSON literal from `variables` (at most 32 names, 4 KiB each, 128 KiB
combined). Optional `project` writes `projection.json` or `projection.csv` into
the job directory from a `$ref` after the referenced step succeeds. CSV uses
CRLF records, requires an array of objects, and prefixes formula-leading text
cells with `'` after skipping leading whitespace or control characters;
`columns` selects fields. A requested projection that exceeds 1 MiB, 10,000
CSV rows, or 32 columns fails the run after the browser steps complete.
Timeout and cancellation skip projection; only a completed successful or
fail-fast run writes it. Resolved tool arguments after `$ref`/`$var`
substitution are limited to 1 MiB per step. The runner checks every tool
against `tools/list` before making the first call, stops after the first failed
step, and never copies tool arguments into its report. Plans are limited to
1 MiB and 256 steps.

Checked examples for navigation, DOM extraction, interaction, and network
monitoring live next to this crate. They reuse `panels#/panels/0/panel_id` and
require a live keep-alive or Horizon UI panel; `run` does not create one, and
an empty `browser_list` fails at the second step.

```bash
horizon-browser mcp --standalone --keep-alive
horizon-browser run crates/horizon-browser-cli/examples/navigate.json
horizon-browser run crates/horizon-browser-cli/examples/extract.json
horizon-browser run crates/horizon-browser-cli/examples/interact.json
horizon-browser run crates/horizon-browser-cli/examples/network-watch.json
horizon-browser run crates/horizon-browser-cli/examples/http-auth.json
```

The report is JSON with top-level `job_id`, `job_dir`, `state_path`, `ok`,
`completed_steps`, ordered step results, and an `observability` summary of
audit completeness and network-capture health taken from executed MCP results
(`records_retained`, `malformed_records`, `older_records_dropped`,
`cursor_lost`, sequence gaps, records written/dropped, truncation, file
limits, and writer failure). Tools that were not called are reported as
`observed: false`. Every invocation stages its
validated plan and initial `prepared` state in an owner-only directory, flushes
both artifacts plus an empty checkpoint-artifact directory, and atomically
publishes the complete job under `browser-jobs/` in the browser runtime root; later lifecycle
updates remain atomic. The prepared
state records an absolute deadline and acts as a lease: it may represent a live
runner before the deadline, and must be treated as `timed_out` at or after the
deadline without relying on another write. Runs that reach plan execution also
save a final report. Legacy lifecycle schemas remain recognizable but are not
resumable because they lack the current checkpoint contract. Automatic
continuation remains disabled; resume is always explicit.

Before each MCP call the runner persists intent in `state.json`. Only a
verified structured outcome becomes a checkpointed completion. `state.json`
stores compact completion metadata, while each structured result is written
once to an owner-only immutable file under the job's `checkpoints/` directory;
later checkpoint updates do not rewrite earlier results.
An interrupt after dispatch records that step as `uncertain`. Resume is
explicit:

```bash
horizon-browser resume job-<id>
horizon-browser resume job-<id> --on-uncertain skip
```

`resume` refuses a live prepared lease, a successful job, and any uncertain
in-flight mutation unless `--on-uncertain skip` is set after inspecting the
browser audit. Skip never replays the uncertain call; it continues later
steps. A skipped step is not a successful completion, so the job cannot
report `ok` until every plan step has a verified result. Remaining steps
open a new MCP client. If the original run recorded a keep-alive standalone
host, resume reconnects to that same browser; a dead host is an error and is
never silently replaced. Start a keep-alive host with
`horizon-browser mcp --standalone --keep-alive` so later `run`, `--connect`,
or `resume` can reuse it. `horizon-browser mcp --stop` asks that host to exit
and remove its profile. Crashed or PID-recycled hosts are pruned on the next
MCP or resume command, including resume of jobs that never recorded a host.

Every deterministic run gets one action deadline. The default is 1800 seconds;
`--timeout` accepts 1 through 86400 whole seconds. The budget is selected after
plan input and validation, before durable preparation, and covers whether MCP
initialization, tool discovery, calls, and client/server shutdown may continue.
If preparation returns after the deadline, no MCP action starts and its
prepared lease resolves to `timed_out`. Terminal state, report, and
requested-output writes happen after browser work stops so timeout evidence can
still be preserved; this post-processing I/O can add wall-clock time after the
deadline.

Ctrl-C is separate from that action budget and remains active while stdin is
open, during durable preparation and MCP work, and while the final report is
being saved or delivered. Cancellation before a validated plan exits without
creating a job. During preparation the runner gives its task-owned filesystem
worker one shared second to finish and marks any published job `cancelled`.
Final persistence gets the same bounded grace; the process then exits 130
instead of hanging on blocked output. A cancellation during a tool call saves
the completed prefix when it can flush within that grace and records
`stop_reason: "cancelled"`.

A deadline during a tool call saves the completed prefix as a partial report
and records `timed_out` in `state.json`; the state also records the configured
`execution_timeout_seconds` and absolute `deadline_at_millis`. If durable
preparation consumes the deadline, no MCP action starts. An already-dispatched
browser mutation may still complete, so inspect the browser audit before
retrying it; `resume` will not automatically replay an uncertain mutation.

Stdout contains only the report; diagnostics use stderr. A file report is
created with owner-only permissions on Unix. Stable exit codes are 0 for
success, 1 for execution failure, 2 for invalid CLI or plan input, 124 for a
job deadline, and 130 for Ctrl-C cancellation.

Outside Horizon, `run` can discover and control existing live panels. When it
runs in a Horizon agent terminal, it inherits `HORIZON_BROWSER_ACTOR`, so the
same plan can also call the actor-scoped `browser_create`,
`browser_visibility`, and `browser_close` tools. The CLI does not accept an actor override flag.
An outside-Horizon invocation releases its process-local panel ownership on
clean exit, so another plan can control the same panel immediately. A crashed
process still falls back to the bounded ownership TTL.

## Standalone stdio MCP server

Configure any MCP client to launch a hidden browser with automatic backend
selection:

```text
/path/to/horizon-browser mcp
```

Choose a backend or show the native window when needed:

```text
/path/to/horizon-browser mcp --backend firefox
/path/to/horizon-browser mcp --backend chromium --visible
```

The default is headless Chromium, falling back to headless Firefox when
Chromium is unavailable. Safari requires macOS and `--visible`. Without
`--keep-alive`, the process owns one isolated browser session for the lifetime
of MCP stdin and removes its temporary profile during bounded shutdown.
`--keep-alive` keeps that session after stdin closes until idle, `--stop`, or
the host exits, so another MCP client can reconnect without restarting the
browser. `--connect` discovers existing Horizon or keep-alive standalone
panels without starting a browser. When Horizon supplies
`HORIZON_BROWSER_ACTOR`, connect mode remains the default so actor-scoped panel
creation, visibility changes, steering, and audit continue to use Horizon's
host lifecycle.

This is a local process using MCP over stdin/stdout, not a TCP or public network
listener. Both modes share the same private Horizon coordination state and MCP
tool schemas; the standalone host does not introduce another browser action
API.

## Profile host

`horizon-browser host --config <host file> [--visible]` keeps named persistent
profiles for `--connect` clients, for example one signed-in profile for each
site on a shared browser machine. The host file names each profile:

```yaml
browser:
  profiles:
    mail:
      backend: chromium
      args: ["--proxy-server=socks5://proxy.internal:1080"]
    chat:
      backend: firefox
      firefox_prefs:
        network.proxy.type: 1
        network.proxy.socks: proxy.internal
        network.proxy.socks_port: 1080
```

- A profile name is 1 to 40 of `a-z`, `0-9` and `-`, with a letter or a
  digit at each end. Its folder is
  `<root>/browser-profiles/profile-<name>` (mode 700). The host never deletes
  a profile; remove its folder while no host runs to forget a sign-in.
- `args` go to the browser process. Arguments that the engine manages (the
  profile folder, the automation endpoint, headless mode) are refused.
- `firefox_prefs` (booleans, integers or strings) are written to the
  `user.js` of a Firefox profile at each start of the host. A preference
  that the host file no longer names gets its default again.
- The host file must be a regular file of at most 256 KiB that group and
  other users cannot write. It holds no credential.

One host runs on each runtime root (`HORIZON_BROWSER_ROOT`). It keeps one
stable host instance in `<root>/profile-host.json` (mode 600, with
`host_instance`, `pid` and `started_at`), written when the host is ready, so
the instance stays the same over each restart. A client sets
`HORIZON_BROWSER_ROOT`, a `horizon:` identity in `HORIZON_BROWSER_ACTOR` and
that instance in `HORIZON_BROWSER_HOST_INSTANCE`, runs
`horizon-browser mcp --connect`, and calls `browser_create` with a profile
name as `target`:

- The host opens each page in the shared browser of its profile, so the
  pages of one profile share its cookies and storage. `browser_duplicate`
  opens the current URL of one of your pages in another page of the same
  profile. As for any create with a target, a `browser_create` of a client
  that already holds a page needs `allow_additional: true`.
- Each page ID is `profile-<name>-` and 12 hex digits. The host stamps each
  page for the actor that opened it only: `browser_list` of a client shows
  only its own pages, and a client of another host instance sees none.
- A create with no target, with a remote target, with a name that the host
  file does not have, or with a backend is refused (`target_required`,
  `target_invalid`, `target_unknown`, `unsupported_backend`). At most 16
  pages run, and at most 4 of one actor; the 16th page goes only to an actor
  with no page, so busy clients never lock out a new one
  (`page_limit_reached`). A page with no action for 60 minutes closes.
- `browser_screenshot` writes the newest frame of a page as a private PNG
  under `<root>/profile-host-captures/` (at most 8 for each page, removed
  when the page closes) and returns its path. A client whose agent runs on
  another computer starts `horizon-browser mcp --connect --inline-screenshots`:
  the result then also holds the PNG (at most 8 MiB) as an MCP image block.
  The flag reads only a PNG under the runtime root, so the screenshots of a
  Horizon host stay a path. A screenshot counts as an action of the page.
  `copy_to_clipboard` has no effect, and `clipboard_requested` is `false`.
  Other device viewer requests are refused.
- With `--visible`, each page is a window on the display of the host
  process (`$WAYLAND_DISPLAY` when it is set, else `$DISPLAY`); without it,
  the browsers are headless. A visible Chromium starts with no window of its
  own and opens each page in its own window in the background, so a new
  page never takes the keyboard from a person who types on that display
  (a window manager must also give new windows no focus).
- An interrupt or a termination signal closes every page, lets each browser
  write its profile, and keeps every profile.

The profile host runs no MCP server of its own and opens no network
listener. Its clients reach it only through the private coordination files of
the runtime root, so each client must run on the same computer, as the same
user, with the same root.

## Remote host bridge

An agent that runs on the computer of the person can use the profile host of
another machine. Horizon writes one setting, `browser_host`, with the SSH target
of that machine. It then gives each agent this MCP entry, which runs the bridge
of this computer:

```text
<horizon> --browser-mcp --remote <ssh-target>
```

The bridge starts the server of the other machine through one `ssh` call and
forwards the JSON-RPC lines of both directions. The same bridge is available
from the CLI, which is how the tests and a script reach it:

```bash
horizon-browser mcp --connect --inline-screenshots --remote browser-vm
```

The process on the other machine is:

```text
env HORIZON_BROWSER_ACTOR=<actor of the agent> \
    HORIZON_BROWSER_HOST_INSTANCE=<instance of the remote host> \
    horizon-browser mcp --connect --inline-screenshots
```

- The actor is the identity of the agent panel on this computer. The remote host
  stamps each page for it, so one agent sees the pages that it opened.
- The host instance is the one that the remote host wrote in its
  `profile-host.json`. The bridge reads it over the same SSH transport before it
  forwards a line, and it refuses with a typed reason when no host runs there.
- The bridge takes no profile and names none. A caller names a profile of the
  host as the `browser_create` target, and the host refuses a profile that its
  host file does not name with its own reason (`target_unknown`). A folder under
  `browser-profiles` of that machine is not proof that the host serves the
  profile, because the host never deletes a profile folder.
- The runtime root of the remote host is its default, `$HOME/.horizon`. To use
  another root, set `HORIZON_BROWSER_REMOTE_ROOT` to that path on the remote
  machine.
- The remote server runs the `horizon-browser` program of that machine. To name
  another program, or a path that the login shell of the target does not find on
  its `PATH`, set `HORIZON_BROWSER_REMOTE_EXECUTABLE`.
- The SSH target must answer without a prompt, because the bridge runs inside an
  agent turn. Run the first connection of a target from a terminal.

Two results name a file, and a file of one computer is not a file of the other,
so the bridge changes exactly two things:

1. `browser_act` with `set_files` or `drop_files` names files of this computer.
   The bridge stages each file under `<remote root>/browser-staging/call-<id>/`
   on the remote machine with `ssh`, then forwards the remote paths. A page
   reads the staged copy, which keeps the name of the file. The bridge removes
   the folder of the call when its answer arrives, and it also removes a folder
   when a staging step fails, so a failed call leaves no copy.
   - Only files under the work root of the agent (`HORIZON_WORK_ROOT`, else the
     working directory) or a root in `HORIZON_BROWSER_ATTACHMENT_ROOTS` are
     staged. A path outside those roots is refused with the message that a local
     call gives.
   - One file keeps the local attachment limit of 512 MiB, and one call stages
     at most 512 MiB in total.
   - A staged file exists on the remote machine for the duration of the call.
     The person trusts that machine with the file, as for any other file of that
     machine.
2. `browser_video` and `browser_network` write a private file on the remote
   machine. When such a result names a file of at most 8 MiB, the bridge adds
   its bytes to the result as an MCP resource block, with `video/webm` or
   `application/x-ndjson`. A larger capture stays a path on the remote machine
   and the result says so.

Every other tool and every other result crosses the bridge unchanged, and
`tools/list` of the bridge is the `tools/list` of the remote host. A screenshot
crosses as an image block, because the remote server already runs with
`--inline-screenshots`.

## Dependency boundary

The CLI consumes browser coordination and runtime paths through
`horizon-browser-control`. Its normal/build dependency graph contains no Horizon
core, terminal or UI packages. The existing default runtime root and agent
authentication lookup remain unchanged. CI checks this boundary for both the CLI
and MCP server.

## Custom browser runtime directory

Set `HORIZON_BROWSER_ROOT=/absolute/path/to/browser-state` to choose the
coordination directory for discovery, ownership, queues, results, audit and
durable browser job state. Use the same value for a host and a connecting MCP
client. Relative paths resolve once at startup; an empty value is an error.
Task-owned browser/agent subprocesses still use their dedicated private roots.
Without it, the root is the Horizon home: the absolute path in `HORIZON_HOME`
when that is set, else `HOME/.horizon`. `HORIZON_BROWSER_ROOT` does not move Horizon's
application configuration or terminal sessions.

## Provider device discovery

Use `browser_provider_devices` in a plan to discover any device/OS/browser
combination offered by the configured account. For example:

```json
{"version":1,"steps":[{"id":"devices","tool":"browser_provider_devices","arguments":{"provider":"team_cloud","search":"iPhone 18"}}]}
```

Results contain up to 50 combinations, `total`, and `next_offset`. Pass a returned
`target` to `browser_create` without `backend`. Existing named targets also work.
Discovery does not allocate a device or reserve capacity. Account access and
availability are verified when creating the session. Cloud workers use only their
repository-declared, locally granted account and keep operating after disconnect.

Concurrent requests for the same account share one lookup. Reading the
credential (for example waiting on an OS keychain prompt), the provider request
and decoding the catalog each have their own time budget. A credential store
that does not answer within 10 seconds returns
`provider_catalog_credentials_timed_out`; a stalled request returns
`provider_catalog_unavailable`. Rows that arrive late for the same account
binding are still used. For an unchanged binding, a retry starts no sooner than
15 seconds after the previous lookup. Changing the account's profile or
credentials discards earlier results immediately and may start a new lookup at
once. Across retries and changes, each account runs at most two lookups at once,
including superseded ones, and starts at most four per minute. When a stuck
credential read eventually returns, discovery resumes without restarting
Horizon. While two reads for an account never return, that account's
discovery keeps reporting the stall.

## Direct remote orientation

`horizon-browser orientation <PANEL-ID> portrait|landscape` rotates an existing
remote device through the same `browser_orientation` MCP contract and durable
plan runner. Use `--timeout-millis <1..60000>` (default 15000) to bound measured
acknowledgement and `--output <REPORT.json|->` to choose the JSON report location.
A timeout may follow a device mutation; inspect current state before retrying.
Create-time orientation is available in the MCP plans below.

## Cloud list

From a Horizon agent panel, `horizon-browser cloud list` reads the clouds of
your workspace through the `cloud_list` MCP tool and the durable plan runner.
`horizon-browser cloud attach|park <CLOUD-ID>` acts on one cloud from that list,
as the sidebar does: attach moves the person's view to the cloud, and park parks a
cloud that is out of view. Horizon refuses each other case. Use `--output <REPORT.json|->` to choose the JSON report
location. Outside a Horizon panel, the command fails.

## Remote orientation plans

The plan runner calls the same public MCP contract as interactive agents. A
configured or catalog tablet can start in landscape and return to portrait:

```json
{
  "version": 1,
  "steps": [
    {"id": "tablet", "tool": "browser_create", "arguments": {"target": "tablet", "orientation": "landscape"}},
    {"id": "portrait", "tool": "browser_orientation", "arguments": {"panel_id": {"$ref": "tablet#/panel/panel_id"}, "orientation": "portrait"}},
    {"id": "release", "tool": "browser_close", "arguments": {"panel_id": {"$ref": "tablet#/panel/panel_id"}}}
  ]
}
```

Use an existing configured target or an exact catalog reference returned by
`browser_provider_devices`. Inspect `orientation_support` before rotating an
existing session. Timeouts can follow an applied rotation; inspect rather than
blindly replaying a failed mutation. Unsupported endpoints return
`orientation_unsupported`, and physical viewports remain non-resizable. Both
the MCP server and hosting application must include the orientation capability.
