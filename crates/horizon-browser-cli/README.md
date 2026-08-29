# Horizon Browser CLI

The `horizon-browser` executable makes the existing Horizon browser MCP
contract usable from scripts without creating a second action API. It has two
modes:

- `run` executes a bounded, fail-fast JSON plan and writes a structured report
  to stdout or a private file;
- `mcp` runs the same local stdio MCP server that Horizon registers for agents.

The crate is intentionally thin and is not published. Browser actions, schemas,
ownership, redacted audit, and backend behavior remain in
`horizon-browser-mcp`, `horizon-core`, and the publishable `horizon-browser`
engine.

## Plan runner

Build the binary, then pass it a plan path or `-` for stdin:

```bash
cargo build -p horizon-browser-cli
target/debug/horizon-browser run plan.json
target/debug/horizon-browser run plan.json --output report.json
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

The report is JSON with top-level `ok`, `completed_steps`, and ordered step
results. Stdout contains only the report; diagnostics use stderr. A file report
is created with owner-only permissions on Unix. An unsuccessful report produces
a non-zero exit status.

Outside Horizon, `run` can discover and control existing live panels. When it
runs in a Horizon agent terminal, it inherits `HORIZON_BROWSER_ACTOR`, so the
same plan can also call the actor-scoped `browser_create` and
`browser_visibility` tools. The CLI does not accept an actor override flag.
An outside-Horizon invocation releases its process-local panel ownership on
clean exit, so another plan can control the same panel immediately. A crashed
process still falls back to the bounded ownership TTL.

## Standalone stdio MCP server

Configure any MCP client to launch:

```text
/path/to/horizon-browser mcp
```

This is a standalone local process using MCP over stdin/stdout, not a TCP or
public network listener. It shares the same live-panel discovery and private
Horizon coordination state as the bundled server. Actor-scoped lifecycle tools
work when Horizon supplies `HORIZON_BROWSER_ACTOR`; other local launches can
control a discovered panel but cannot create or toggle panel visibility.
