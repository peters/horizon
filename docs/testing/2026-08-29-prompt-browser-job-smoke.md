# Prompt browser job temporary smoke plan

Run against the exact prompt-job candidate after the standalone-host lane is
green. Browser and website access must occur only through the candidate's MCP
server; do not substitute curl, a separate browser harness, or direct HTTP.

## Linux

1. With an isolated invocation directory, run a hidden Chromium prompt against
   `https://example.com`. Require concise stdout, at least one completed
   Horizon MCP tool, a verified summary, zero exit, and no surviving browser.
2. Repeat with hidden Firefox and require the BiDi backend.
3. Run a prompt ending in `save to result.csv`. Require the requested content,
   no other task artifact in the invocation directory, mode `0600` on Unix,
   and no local path selected by page content.
4. Repeat one successful job with `--json`; parse every stdout line as JSON and
   require a final `job_completed` record. Open its report, trace, and executed
   plan paths; require mode `0600`, a validated plan, a typed panel-id reference,
   no copied page result, and a correct replayable flag.
5. Run a deliberately impossible goal and require `ok: false` plus non-zero
   exit. Run with a missing adapter and require a clear bounded error.
6. On isolated Xvfb, run Chromium and Firefox with `--visible`; require an exact
   task-owned native window, successful MCP navigation, and clean exit.

## macOS handoff

After Linux and current-head CI pass, repeat hidden and visible Chromium and
Firefox prompt jobs, the artifact/JSON lanes, and one visible Safari prompt.
Record exact head, commands, exits, artifact/report permissions, executed-plan
validation, and backend evidence in the stacked PR report.
