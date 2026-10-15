---
applyTo: "examples/cloud-worker/horizon-worker-github-upkeep,examples/cloud-worker/horizon-worker-github-api-upkeep,examples/cloud-worker/horizon-worker-github-api,examples/cloud-worker/horizon-worker-github-api-rest,examples/cloud-worker/horizon-worker-github-git-upkeep,examples/cloud-worker/horizon-worker-github-git,examples/cloud-worker/test_github_upkeep_push.py,examples/cloud-worker/horizon-worker-github-graphql-policy,examples/cloud-worker/test_github_upkeep_policy.py,crates/horizon-core/src/upkeep/worker/policy.rs"
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
- Auto-merge (`enablePullRequestAutoMerge`) never reaches GitHub, nor does an
  approval of a pull request: the broker reads the whole body of a new or submitted
  REST review (`upkeep.reviews`), refuses a body that is not JSON with each field
  once, and refuses the `APPROVE` event in any case and spacing, also through the
  GraphQL `addPullRequestReview`. A dismissal of a review is refused.
- A workflow dispatch passes only for a listed (repository, file) pair, resolved
  through GitHub from the ID or file name in the path, and only with the `ref` of the
  default branch that GitHub reports: on another ref, an agent could have pushed the
  workflow file.
- A review change in any other spelling than the plain one (`REVIEW_CHANGES`) is
  refused, so none goes around the approval and dismissal checks.
- The default branch changes only through a merged pull request. The Git proxy reads
  the commands at the start of a push (`horizon-worker-github-git-upkeep`, `refs`) and
  refuses any ref that is not a branch and the default branch that GitHub reports;
  commands it cannot read are refused, never passed. The broker refuses `merges`,
  `merge-upstream`, the creation of a reference, a tag reference, a change of the
  default branch's reference, a contents write whose body names the default branch
  or no branch, and `update-branch` of a pull request whose head GitHub reports as the
  default branch of the same repository (or does not report).
- No token reaches a log, a refusal text or a subprocess argument.

## Not covered

Code that workflows run outside `.github/workflows`, pushes to other branches than
the default one (also of workflow files, which GitHub runs for a push with the
repository's secrets), merges (a later part adds a gate), and outbound network connections: the
worker has no outbound limit. The docs say so; do not claim otherwise.
