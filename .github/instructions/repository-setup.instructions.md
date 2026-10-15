---
applyTo: "crates/horizon-ui/src/app/cloud_panel/production/repository_setup.rs,crates/horizon-ui/src/app/cloud_panel/production/repository_setup/**"
---

# Trust boundary of the setup agent

This code runs **Set up from CI**. A local setup agent, Claude or Codex, writes
`.horizon`. Horizon proves the setup with Docker on this computer, or with the
Docker of a Docker host that the person chose under **Build on**, and
**Use this setup** commits it. The design is in the module comments and in the
section "Repository setup and deployment" of `docs/cloud-workspaces.md`.

## Untrusted: the files of the agent

While the agent runs, it can write all of the repository. This includes
`.horizon`, the status file, the run folder, and the repository's git
configuration, refs and hooks.

- These files must never run code in Horizon's process or in Horizon's git
  commands. For a git command in the repository, `git_command` in `commit.rs`
  turns off hooks, file monitors and replace refs. Filters and signing stay off
  through the plumbing, for example `hash-object --no-filters` and
  `commit-tree --no-gpg-sign`.
- Horizon reads an agent-written file with the readers in `untrusted.rs`. They
  follow no link, do not wait on a FIFO or a device, and stop at a size limit.
- **Use this setup** commits only the setup that the proof tested. Horizon
  reads it again after the agent ended, and names every other change first.

## Bounded commands

Each git or docker command of a setup must have a time limit and an output
limit. `bounded.rs` and the proof's captures give these limits. Git reads the
repository configuration, and an `include` there can name a FIFO. No git or
docker command may hold **Use this setup**, **Stop**, a session switch or
Horizon's exit past its time limit.

## Deliberate limits: do not report

- The proof shows that the setup works, not that the image is honest. Every
  program that the CI runs comes from the image, its shell included.
- The proof runs the localci that a worker session finds first on its `PATH`,
  as worker agents do. Horizon's own verified localci checks the plan before
  the CI. It runs the CI only for a published image without a localci. Do not
  report that the image, its shell, its git or its localci can make a proof
  pass.
- The agent runs on the person's own login, in a systemd user scope. With the
  scope, Horizon can stop all processes of the agent before it commits.
- The scope is not a sandbox. Do not report that the agent, or a program that
  it starts, can leave the scope through another service of the person, such
  as `systemd-run --user` or Docker. Do not report what the agent can do with
  the person's account outside Horizon's process, Horizon's git commands and
  the committed setup.
- The opt-in steps on this computer, the private NuGet restore and the private
  Rust git clones, are off by default. The docs state their risk. Do not report
  that risk again.

## A Docker host under Build on

- A Docker host is a machine that the person saved in Cloud settings. Horizon
  reaches it with the person's own SSH configuration and keys, and keeps no
  secret for it. The host is trusted as much as this computer's Docker.
- The proof copies files into its containers and mounts nothing of this
  computer (`proof/launch.rs`). On a host it sends the same copies: the bundle,
  the committed tree, Horizon's localci, and the NuGet packages and Rust git
  clones that the person allowed. Do not report that the host gets them.
- The SSH forwards (`proof/remote.rs`) end with the daemon value that holds
  them. A forwarded socket is in a private folder of this computer.

## Report

- An agent-written file that makes Horizon run code, block, follow a link out
  of the repository or use memory without a limit.
- A git command in the repository that can run a hook, filter, file monitor or
  signing program, or that reads a replace ref.
- A git or docker command of a setup without a time limit or an output limit.
- A path where **Use this setup** commits a setup that the proof did not test.
- A proof container or an image that gets this computer's git folder, or a
  credential of the person, such as a Git or feed login.
- A bind mount of a folder of this computer into a proof container, or a copy
  that follows a link out of the folder it copies (`proof/launch.rs`).
- A docker command of a proof or its cleanup that goes to another daemon than
  the one the proof pinned, such as this computer's for a run on a Docker host.
- Code from the repository that Horizon runs on this computer outside Docker,
  for example a build file, a Cargo step or a hook. The only exception is the
  private NuGet restore when its box is checked.
