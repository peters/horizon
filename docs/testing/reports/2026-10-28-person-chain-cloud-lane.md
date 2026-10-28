# Test report: the person chain of an agent's cloud lasts

Report of
[the durability procedure](../procedures/cloud-person-chain-durability.md) on 2026-10-27 and
2026-10-28. It answers one claim: an agent's cloud keeps its GitHub access, and pushes as the
person, while the person's computer is off.

## Environment

| Item | Value |
|---|---|
| Candidate | `main` `244f0764a` (`#1895` and `#1896` merged) |
| Binary | `/var/tmp/horizon-run/bin/horizon-lane4` |
| Binary SHA-256 | `28e1d0ca9c140129b4927679ea22fbb376a4ad280844dd8b369b63ca6eaa03e6` |
| Instance | Horizon under Xvfb, private state, watched in a native VNC Device panel |
| Agent | One scripted agent panel that calls `cloud_create`, `cloud_exec` and `cloud_delete` through `--browser-mcp` |
| Provider | Hetzner `cx23`, chosen by the policy from its offers, EUR 0.0088 per hour |
| Repository | `peters/horizon-hello-world`, one commit `20cbd71d7c3d` |
| GitHub App | `horizon-for-peters`, app ID 5247767, mode `automatic` |

The instance started with no App private key and no stored chain, so the create had to make
the one sign-in.

## Run 1: durability over eight hours

| Step | Result |
|---|---|
| Sign-in | The create reported `GitHub sign-in needed: open https://github.com/login/oauth/authorize?...` and waited. An already signed-in browser opened that address, and the create continued by itself. |
| Cloud | `6d4d9e41-1138-48d3-ab21-93e330db8244`, Hetzner server `172865782`, `cx23`, created 2026-10-27T17:44:57Z |
| Chain in the worker | `"kind": "person"`, `"renewal": "worker"`, `"persistent": true`, `"serving": true`, login `peters`, grant `peters/horizon-hello-world` for push |
| Chain stored | 2026-10-27T17:44:53Z, with an access token that ends 2026-10-28T01:44:53Z and a refresh token that ends 2027-04-27T17:44:53Z |

All times in this report are UTC.

| Step | Result |
|---|---|
| Commit and pull request | Worker commit `8d320a2` on `app-token-9104621a`, pull request `#2`. Author `peters`, and the commit author and committer are `peters` with `869+peters@users.noreply.github.com`. The worker's Git identity was empty before the commit: the product gave it. |
| No action overnight | No person, no Horizon command and no `cloud_exec` touched the worker between 17:51Z and 01:14Z. |
| Renewal | Reading at 2026-10-28T01:18Z: `last_refresh_at` `2026-10-28T01:14:53Z`, `access_expires_at` moved to `2026-10-28T09:14:53Z`, `refresh_expires_at` to `2027-04-28T01:14:53Z`, `last_error` `null`. The worker renewed exactly 30 minutes before the end, which is its own margin. |
| Push on the renewed token | Pull request `#3` from branch `person-renewed-10656852`, author `peters`, merged with a squash as `2d2b901e4` at 2026-10-28T01:20:25Z. |
| Cost | About EUR 0.07 for 7.5 hours. |

### Deviation: the cloud was deleted at the provider, not through the card

The run stopped the instance before it deleted the cloud. The fixture removes its private
state directory when it stops, so the deployment key and the cloud record went with it, and
`cloud_delete` could not be issued.

The server was then deleted at Hetzner with the API: the delete answered `200`, the next read
answered `404`, and the account listed no server of that name. The `cloud_delete` path is
covered by [the delete procedure](../procedures/cloud-delete-tool.md) and was not exercised
here.

The procedure now says to delete the cloud before the instance stops.

## Run 2: a push with no Horizon running

| Step | Result |
|---|---|
| Cloud | `8108146a-0261-451f-9395-e5cca6952013`, Hetzner server `172934820`, created 2026-10-28T02:22Z, about ten minutes of work |
| Sign-in | The same `authorize` line, approved in the same browser, with no other action |
| Commit and pull request | Pull request `#4`, author `peters` |
| State kept before the stop | The deployment key `ed25519` and the cloud record, copied out of the state directory |
| Horizon stopped | The instance stopped, and no process of the candidate ran: `ps` counted 0 |
| Push | Commit `440b14b` on `no-horizon` at 2026-10-28T01:31:22Z, author `peters`, `869+peters@users.noreply.github.com`, over SSH as the agent user with `HOME=/workspace/home` |
| Pull request | `#5`, created 2026-10-28T01:31:48Z by the worker's `gh`, author `peters` |
| Deletion | Hetzner server `172934820`: read `200`, delete `200`, next read `404`, and the account listed no server |
| Cost | About EUR 0.002 |

## What this shows

- The worker renews its chain from its own refresh token, on its own timer, with no action of
  this computer. The renewed token then pushes and merges.
- Git and `gh` in the worker do not need Horizon: they work while no Horizon process runs on
  this computer, and the commits and the pull request are the person's own.

## What this does not show

- A renewal that fails, and what the worker reports then. The unit tests of the worker cover
  the refusal and the retry; this run had no failure to observe.
- A refresh after the refresh token ends, which is about six months away.
- A GitHub outage during the second read of the chain, which
  [#1896](https://github.com/peters/horizon/pull/1896) makes a cloud without GitHub access
  instead of a failed cloud. The type of that answer is total, and no run has forced it.
- The device code form of the sign-in. The app of this machine is in `automatic` mode, so the
  create reports the `authorize` address. [The create procedure](../procedures/cloud-create-tool.md)
  exercises the device code.
