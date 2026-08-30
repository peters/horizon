# Standalone browser host temporary smoke plan

Run this plan against the exact candidate head for the standalone
`horizon-browser mcp` host. Keep every runtime under a task-owned temporary
`HOME`; do not reuse, signal, or close an existing Horizon or browser process.

## Linux

1. Run CLI, engine, and MCP unit/integration tests.
2. Start hidden Chromium through `horizon-browser mcp --standalone --backend
   chromium`, negotiate MCP, and require one panel with `visible: false`.
3. Navigate to `https://example.com`, query its title, inspect the audit, close
   MCP stdin normally, and require the exact browser process and temporary
   profile to disappear.
4. Repeat the hidden lane with Firefox and require the BiDi backend.
5. On an isolated Xvfb display, repeat Chromium and Firefox with `--visible`.
   Require `visible: true`, a native window owned by the exact task process,
   successful navigation, and a proof screenshot after resize.
6. Verify Safari without `--visible` fails clearly before starting a driver.
7. Interrupt standalone startup once and confirm its exact child/profile are
   cleaned within the bounded shutdown path.

## macOS handoff

After Linux and CI pass on the current head, repeat hidden and visible Chromium
and Firefox lanes. Run visible Safari, navigate and query through the same MCP
contract, then close stdin and require `safaridriver` lease/session cleanup.
Record each backend, mode, command, result, and exact commit in the PR report.
