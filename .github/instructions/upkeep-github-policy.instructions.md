---
applyTo: "examples/cloud-worker/horizon-worker-github-upkeep,examples/cloud-worker/horizon-worker-github-api-upkeep,examples/cloud-worker/horizon-worker-github-api,examples/cloud-worker/horizon-worker-github-api-rest,examples/cloud-worker/horizon-worker-github-graphql-policy,examples/cloud-worker/test_github_upkeep_policy.py,crates/horizon-core/src/upkeep/worker/policy.rs"
---

# Trust boundary of the Upkeep policy of the worker's GitHub access

The agent of an Upkeep reads untrusted pull request text and runs without approvals.
It never holds a GitHub token: the root API broker and the root Git proxy add the
token. Horizon sets the Upkeep policy as root (`horizon-worker-github upkeep`). The
user-facing description is in `docs/upkeep.md`, section "What the agent can do on
GitHub".

## Untrusted: every request of an agent, and everything an agent can write

Requests from uid 10001 (and root's own `gh`) reach the broker with any method, path,
body and GraphQL document. The loop and its files in `/workspace/home/upkeep` are
agent-writable, so nothing there is a control. Check that:

- `upkeep.load` reads only root-private files and treats a policy file that exists
  but cannot be read, or that the marker does not name, as the strictest policy, never
  as no policy. `install` names the new policy in the marker before it writes a copy.
- `rest_refusal` sees the decoded path segments that GitHub reads (`Route.parts`),
  for `/repos/OWNER/NAME/...`, `/repositories/ID/...` and release uploads.
- Auto-merge (`enablePullRequestAutoMerge`) never reaches GitHub.
- A workflow dispatch passes only for a listed (repository, file) pair, resolved
  through GitHub from the ID or file name in the path.
- No token reaches a log, a refusal text or a subprocess argument.

## Not covered

Code that workflows run outside `.github/workflows`, Git pushes (a later part checks
them), merges (a later part adds a gate), and outbound network connections: the
worker has no outbound limit. The docs say so; do not claim otherwise.
