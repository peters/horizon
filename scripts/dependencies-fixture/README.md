# Dependencies test worker

A synthetic dependency worker for the [Dependencies panel](../../docs/dependencies.md).
It serves real SSH on `127.0.0.1` with generated Ed25519 keys and strict host keys.
It never reaches a cloud provider or GitHub.

| Real | Simulated |
|---|---|
| SSH transport, host key checks, the `maintenance` commands | GitHub pull requests, CI, review and merges |
| Parsing `.github/dependabot.yml` in 21 synthetic repositories | Completed pull request history |
| Reading `AGENTS.md` and running each repository's `checks.py` fixture | Cloud allocation |

The worker reads `AGENTS.md`. It runs only the explicit `checks.py` fixture recipe.
It does not run commands that `AGENTS.md` or a status document contains.

## Requirements

- Python 3 with `paramiko` and `PyYAML`.
- `ssh` and `ssh-keygen` from OpenSSH.

## Run

```sh
root="$(mktemp -d)"
python3 scripts/dependencies-fixture/serve.py --root "$root" --delay 3
```

The first line of output is JSON with `ready`, the folder and the port. Then start
Horizon with `HORIZON_MAINTENANCE_FIXTURE="$root"`. Keep the folder outside the
repository: it holds the generated private keys.

`--delay` sets the seconds between the worker's steps. Stop the server with Ctrl+C.

## Commands

The server accepts only these commands, with the fixture key:

| Command | Effect |
|---|---|
| `maintenance status` | The status document as JSON. Horizon reads at most 1 MiB |
| `maintenance run` | Starts the worker if it is not running and follows its log |
| `maintenance start` | Starts the worker and returns |
| `maintenance watch` | Follows the log of a running worker |
| `maintenance configure` | Reads instructions as JSON on stdin, at most 64 KiB, and saves a new revision |
| `maintenance diagnose` | Health and revisions, without prompts, logs or keys |
| `maintenance stop` | Stops the worker |

Any other command is refused.

## Tests

```sh
python3 scripts/dependencies-fixture/test_ssh.py
python3 scripts/dependencies-fixture/test_safety.py
```

`test_ssh.py` starts a server in a temporary folder and checks SSH, instruction
saves, oversized and invalid input, heartbeats, worker death and refused commands.
`test_safety.py` checks that a repair keeps the Dependabot configuration and the
fixture checks unchanged.
