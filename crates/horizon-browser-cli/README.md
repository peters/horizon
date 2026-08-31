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
`horizon-browser-mcp`, `horizon-core`, and the publishable `horizon-browser`
engine.

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
click, fill, scroll, wait, and evaluate; show or hide an actor-owned panel; read
the audit trail; and capture or cursor-watch bounded HTTP/WebSocket traffic on
supported backends.

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
an owner-only job directory under `~/.horizon/browser-jobs/`; MCP runtime
profiles use a separate temporary home and are removed after each job.

Every completed prompt job also writes a private redacted `trace.jsonl`, a
validated `executed-plan.json`, and `report.json`. The plan replaces the
standalone panel id with a typed reference to the recorded `browser_list`
result. The report marks the plan non-replayable when execution depended on
ephemeral semantic references or when selectors, scripts, text, values, URL
credentials, query values, or fragments had to be redacted. Tool results and
page contents are not copied into the trace.

The normal workspace build contains no model SDK. Job mode invokes an optional
local agent executable and expects its structured `exec` event interface; set
`HORIZON_BROWSER_AGENT_COMMAND` to a compatible adapter. A missing adapter is a
clear runtime error and does not affect `run` or `mcp`.

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
References cannot point forward. The runner checks every tool against
`tools/list` before making the first call, stops after the first failed step,
and never copies tool arguments into its report. Plans are limited to 1 MiB and
256 steps.

The report is JSON with top-level `job_id`, `job_dir`, `state_path`, `ok`,
`completed_steps`, and ordered step results. Every invocation stages its
validated plan and initial `prepared` state in an owner-only directory, flushes
both artifacts, and atomically publishes the complete job under
`~/.horizon/browser-jobs/`; later lifecycle updates remain atomic. The prepared
state records an absolute deadline and acts as a lease: it may represent a live
runner before the deadline, and must be treated as `timed_out` at or after the
deadline without relying on another write. Runs that reach plan execution also
save a final report. Legacy `running` states remain readable. Automatic
continuation is not enabled yet.

Every deterministic run gets one action deadline. The default is 1800 seconds;
`--timeout` accepts 1 through 86400 whole seconds. The budget is selected after
plan input and validation, before durable preparation, and covers whether MCP
initialization, tool discovery, calls, and client/server shutdown may continue.
If preparation returns after the deadline, no MCP action starts and its
prepared lease resolves to `timed_out`. Plan input and the blocking preparation
write are not yet interruptible. Terminal state, report, and requested-output
writes happen after browser work stops so timeout evidence can still be
preserved; this post-processing I/O can add wall-clock time after the deadline.

A deadline during a tool call saves the completed prefix as a partial report
and records `timed_out` in `state.json`; the state also records the configured
`execution_timeout_seconds` and absolute `deadline_at_millis`. If durable
preparation consumes the deadline, no MCP action starts. An already-dispatched
browser mutation may still complete, so inspect the browser audit before
retrying it; automatic replay remains disabled.

Stdout contains only the report; diagnostics use stderr. A file report is
created with owner-only permissions on Unix. Stable exit codes are 0 for
success, 1 for execution failure, 2 for invalid CLI or plan input, and 124 for
a job deadline.

Outside Horizon, `run` can discover and control existing live panels. When it
runs in a Horizon agent terminal, it inherits `HORIZON_BROWSER_ACTOR`, so the
same plan can also call the actor-scoped `browser_create` and
`browser_visibility` tools. The CLI does not accept an actor override flag.
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
Chromium is unavailable. Safari requires macOS and `--visible`. The process
owns one isolated browser session for the lifetime of MCP stdin and removes its
temporary profile during bounded shutdown. `--connect` instead discovers
existing Horizon panels without starting a browser. When Horizon supplies
`HORIZON_BROWSER_ACTOR`, connect mode remains the default so actor-scoped panel
creation, visibility changes, steering, and audit continue to use Horizon's
host lifecycle.

This is a local process using MCP over stdin/stdout, not a TCP or public network
listener. Both modes share the same private Horizon coordination state and MCP
tool schemas; the standalone host does not introduce another browser action
API.
