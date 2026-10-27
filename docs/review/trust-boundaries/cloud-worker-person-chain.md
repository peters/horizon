---
applyTo: "crates/horizon-core/src/cloud_runtime/github/person_chain.rs,crates/horizon-core/src/cloud_runtime/github/host.rs,crates/horizon-core/src/cloud_runtime/github/worker.rs,crates/horizon-core/src/cloud_runtime/deployment/git_credentials.rs,crates/horizon-ui/src/app/cloud_panel/production/create_agent.rs"
---

# Cloud worker person-chain boundary

Read the module comment in
`crates/horizon-core/src/cloud_runtime/github/person_chain.rs`, and the token
boundary [`github-installation-token`](github-installation-token.md) for the
app's private key and the sign-in it describes.

The create of an agent's cloud has no person at the card. An agent's cloud
therefore takes the chain of this computer's own GitHub sign-in, which
`crates/horizon-core/src/cloud_runtime/github/host.rs` keeps, renews and checks
with GitHub.

## What is trusted

- The cloud settings file of this computer: the app, and the repositories of the
  agent lifecycle policy, including `allow_github_sign_in_skip`.
- The stored sign-in file of this computer, `credentials/github-host-<app>.json`
  in the cloud root, a private regular file with mode 0600. Horizon writes it,
  and a file readable by another user is refused.
- The record of a cloud that an agent created.
- The worker's root service. Its answer is parsed, never shown as deployment
  output. Every byte of a GitHub answer is untrusted input.

## What a worker holds

- One access token and one refresh token of the person's own GitHub sign-in for
  the app, with the end that GitHub states for each.
- The chain arrives as payload version 1 on SSH stdin, in memory. It never enters
  a command argument, an output, a log line or a named file on this computer.
- The worker's root service keeps the chain in a file with mode 0600 in a
  directory with mode 0700. Git reaches GitHub through a loopback proxy that adds
  the access token, and `gh` reaches it through the API broker, which holds the
  token. An unprivileged agent user does not hold either token.
- When the chain came from a web sign-in, the payload also carries the app's
  client secret, because only that secret renews such a chain. A chain from a
  device sign-in renews with the client ID alone, and then the worker holds no
  secret.

## What the code guarantees

- The chain renews itself on the worker for as long as GitHub honours the refresh
  token. Horizon records the end that GitHub states for the refresh token, now
  about six months. After that end GitHub refuses the refresh, the worker reports
  a chain it can no longer renew, and Horizon reports that the cloud has no
  GitHub access. Horizon never renews the chain on this computer for a cloud that
  already holds it.
- The chain is stored by one sign-in, and an agent's create can trigger that one
  sign-in: when no chain is stored and `allow_github_sign_in_skip` is false, the
  create runs the same sign-in that the person's own path runs, reports the
  device code or the authorize page as its status, and continues when the person
  approves. The sign-in happens once for the machine, not once for each cloud.
- A refusal that arrives after the cloud is rented never fails the cloud. The
  deployment reads the chain once before it rents, where a refusal stops the
  create and nothing is rented, and once after it rents, where a chain that
  GitHub ends during the image build, a GitHub outage or a network drop leaves
  the cloud without GitHub access instead of failing it. The cloud's card
  connects it again.
- An agent's create never waits for a sign-in that it did not start. A policy
  that allows continuing without GitHub access proceeds without one.
- `login`, `author_name` and `author_email` come from the account that the chain
  acts as, so a commit and a pull request of the worker are authored as the
  person, never as the app's bot. The author name is the name GitHub answers, and
  the login when GitHub answers none. The author address is
  `<id>+<login>@users.noreply.github.com`, built from the account, which GitHub
  attributes to that account and never shows an address that the person keeps
  private.
- The chain is installed for the repositories of the agent lifecycle policy and
  for no other repository. A grant outside that list keeps no access, and the
  cloud's output names it.
- A policy path without a GitHub remote, a policy that lists no repository, a
  machine with no GitHub App, a sign-in that does not finish, and GitHub not
  confirming the stored chain each refuse the create before a card exists or a
  worker is rented. Each refusal is a typed error that names the setting that
  resolves it: `agent_lifecycle.repositories`, the GitHub connection in Horizon's
  settings, or `agent_lifecycle.allow_github_sign_in_skip`.
- A policy whose `allow_github_sign_in_skip` is true continues without GitHub
  access when no chain is stored, and asks no person.
- A chain that GitHub ended is forgotten, so the next create asks for one sign-in
  again. A chain that is only unreachable, or that GitHub does not confirm, is
  kept for the next try.
- The chain is read and the sign-in waits on a host thread, never on the frame
  thread. The create reads it once to refuse, and drops it. The deployment reads
  it again and installs the chain the worker holds, so a chain renewed while an
  image built is the one installed.
- No value of either token, the client secret or the app's private key reaches
  output. The log lines of this path name a login, a repository and a count.

## Report these failures

- A token, the client secret or the app's private key reaches `Debug` output, an
  error message, a log line, a progress line, a command argument, an environment
  of a host process, or a test fixture other than a synthetic value.
- A commit or a pull request of the worker is authored as the app's bot, or with
  an address that is not the noreply address of the account.
- The chain is installed for a repository that the policy does not list.
- A create rents a worker before a refusal of the list above.
- A create signs in while `allow_github_sign_in_skip` is true, or asks no person
  while no chain is stored and the policy forbids continuing without access.
- A sign-in or the reading of the chain runs on the frame thread.
- A chain that GitHub ended is kept, or a chain that GitHub did not answer for is
  forgotten.
- The device code or the authorize URL of a sign-in reaches a log line or an error
  instead of the status of the operation that waits for it.

## What this does not guarantee

- The chain reaches every repository that the app is installed on, with the
  app's permissions, and not only the repositories of the policy. The install
  narrows the worker's grants to the policy, and the worker's Git proxy refuses
  the others, but the token itself is wider than the cloud.
- A worker's root user can read the chain, as root reads every file of that
  worker. Horizon does not protect it from root, from the provider's console or
  from a fault in the worker image.
- The chain is the person's own sign-in for the app. Horizon does not scope it
  more narrowly, and a person who revokes the app on GitHub ends every cloud that
  holds it.
- Horizon does not guarantee a lifetime beyond the refresh token's. GitHub decides
  that lifetime, and Horizon only records the end that GitHub states. When it
  ends, the person signs in again.
- The person's device code or authorize page reaches the agent as a status line of
  its operation. Horizon cannot confirm that the person read it, and the sign-in
  fails when nobody approves it in time.
- This boundary covers the chain of an agent's cloud. It does not cover the
  person's own sign-in on a cloud's card, which `github-installation-token` and
  the card's Connect GitHub path keep.
