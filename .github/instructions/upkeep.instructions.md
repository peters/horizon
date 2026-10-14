---
applyTo: "crates/horizon-core/src/upkeep.rs,crates/horizon-core/src/upkeep/**,examples/cloud-worker/test_upkeep_worker.py,docs/upkeep.md,docs/testing/procedures/upkeep/worker.sh,crates/horizon-ui/src/upkeep_widget.rs,crates/horizon-ui/src/upkeep_widget/**,crates/horizon-ui/src/app/upkeep.rs,crates/horizon-ui/src/app/upkeep/**,crates/horizon-ui/src/app/cloud_panel/production/new_workspace/upkeep.rs,crates/horizon-ui/src/app/cloud_panel/production/new_workspace/upkeep/**"
---

# Trust boundaries of Upkeep

The Upkeep board shows a status document that an agent on a cloud worker writes.
The user-facing description is in `docs/upkeep.md`, section "Trust boundary of the
status document". The worker loop (`crates/horizon-core/src/upkeep/worker.rs` and
`upkeep/worker/*.py`, `run.sh`) is described in the section "Trust boundary of the
worker loop".

## Untrusted: the status document

Every field can hold any value: very long texts, very many entries, unknown states,
times far in the past or the future, and URLs to other sites. Check that:

- No file read, parse or other blocking work on the status runs on the UI thread.
- The size limit and the list and text bounds apply before the board draws anything.
- Arithmetic over counts from the document cannot overflow or panic.
- An unknown enum value, a field of the wrong type or one broken list entry never makes
  the whole document invalid.
- Nothing assumes the order of lists in the document.
- Only `PullRequest::github_url` decides which page opens, and it accepts only the
  exact GitHub pull request page of a plain `owner/name` repository.

## Trusted: the person and the panel configuration

The panel's `command` (the status path) comes from the person's config or session.
The board reads that path and nothing else.

## Untrusted: everything the worker agent can write

The worker loop runs as the agent account, and the agent runs with full access to
that account. Treat every file under `/workspace/home/upkeep`,
`/workspace/sessions/upkeep-<cloud>` and `/workspace/session-data/upkeep-<cloud>`, and
everything the exchange reads back, as written by the agent. Check that:

- `exchange.py` runs only as the owner of `/workspace/home`. The remote command in
  `worker::command` starts it through `horizon-worker-tailnet agent` when
  `/run/horizon-tailnet/agent-isolation` exists. Root must never write, create,
  `chmod` or `chown` a path inside a folder the agent account owns.
- Every folder is opened with `O_NOFOLLOW | O_DIRECTORY` relative to a checked parent
  and must belong to the running account. Every file is written through a new
  `O_EXCL | O_NOFOLLOW` temporary file and `os.replace`, and read only when it is a
  regular file opened with `O_NOFOLLOW | O_NONBLOCK`.
- Every read from the worker has a size limit, and the answer stays under the 4 MiB
  that `Runner::run_parsed` accepts. Only typed, range-checked fields of `runs.jsonl`
  and `next-run` go back, so no file of the agent can make the answer invalid.
- `run.py` starts `codex exec` without approvals only under the rule of
  `horizon-worker-run`: not as root, and `/run/horizon-worker/agent-approvals` is
  exactly `auto`. Otherwise it starts no agent.

## Untrusted: names on their way to a command line, a path or the prompt

Repository names and bot logins pass `Config::validate` before any exchange, and
`run.py` checks them again with the same rules before they reach `gh`, a file name
or the prompt. Check that:

- A repository is a plain `owner/name` (`horizon_cloud::github::valid_repository`)
  and a bot is a login, `login[bot]` or `app/slug`. No value can start with `-`.
- The cloud id, session name and revision match their patterns on both sides.
- Values reach subprocesses as separate arguments, never through a shell, and the
  prompt reaches `codex exec` on standard input, not as an argument.
- The remote command stays under the worker shell's 128 KiB argument limit.

## Trusted: the person's settings

The instructions, schedule and worker name come from the person. They have size
limits but no content rules: the instructions are the person's own prompt.

## Untrusted: GitHub access requests from the worker

`new_workspace/upkeep/sync.rs` allows some `github_access` requests without the person.
The worker and its agents write every request field. Check that:

- `allowed` needs all of: `task` true, `session` exactly `upkeep-<cloud>`, access `read`
  or `push`, and a repository of this Upkeep's saved list (ASCII case ignored).
- The decision is always `Decision::AllowTask`, never a cloud-wide allow.
- Every other request stays on the cloud card for the person.

## Host work off the UI thread

Discovery (`horizon_core::upkeep::discover`), the clone of the first repository, the
reads and writes under `~/.horizon/upkeep` and every exchange with a worker run on
their own threads. The UI thread only polls their channels.

## The board's actions

Run now, Pause and Repositories show only on a board whose status path is exactly
`worker::directory(<cloud>)/status.json` of a saved Upkeep (`worker::cloud_of`). Check that:

- A board from a config file or the default path never gets a cloud.
- The Repositories editor saves through `worker::update`, under `worker::settings_lock(<cloud>)`,
  with only the added and removed names, off the UI thread, and the result still passes
  `Config::validate`.
- An exchange holds `worker::settings_lock(<cloud>)` and loads the saved choice itself
  (`worker::load`), so no exchange started before a save allows a request for, or sends
  the list with, a repository that the save removed. Do not pass it a choice read
  earlier, and call `worker::update` only off the UI thread: it can wait for an exchange.
- `worker::settings_lock(<cloud>)` is a guard for the threads of this process and a file
  lock (`settings.lock` in the Upkeep's folder) for other Horizon processes on the same
  home. Both must be held for the whole read, change and save, and the whole exchange.
