# Pi RPC Follow-Up

Horizon's first Pi integration launches the standard `pi` CLI/TUI in a PTY, matching the existing agent panel model and preserving Pi's native interactive behavior.

An optional v2 integration can add a Horizon-native Pi status feed by launching `pi --mode rpc`, writing JSONL commands to stdin, and parsing event records from stdout. Candidate events to map into Horizon UI state include `agent_start`, `message_update`, `tool_execution_*`, and `queue_update`.

Keep the PTY launch path as the default. RPC mode should be additive, opt-in, and limited to structured status or inspection surfaces until it can faithfully preserve Pi's interactive session behavior.
