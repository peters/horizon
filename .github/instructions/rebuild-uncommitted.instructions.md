---
applyTo: "crates/horizon-core/src/cloud_runtime/repository/uncommitted.rs,crates/horizon-core/src/cloud_runtime/repository/uncommitted/**,crates/horizon-core/src/cloud_runtime/deployment/replacement.rs,crates/horizon-core/src/cloud_runtime/deployment/replacement/**,crates/horizon-core/examples/cloud_deploy.rs,crates/horizon-ui/src/app/cloud_panel/production/cards/rebuild.rs,crates/horizon-ui/src/app/cloud_panel/production/rebuild.rs"
---

# Trust boundary of the uncommitted build context

`repository::uncommitted` lists, hashes and copies the uncommitted files of an
image recipe's build context, so that **Rebuild image & restart** builds the
image from the working copy. The user rules are in the section "Uncommitted
files in the image" of `docs/cloud-workspaces.md`.

## What goes out

The image can be pushed to a public registry, for example
`ghcr.io/peters/horizon-development`. Every uncommitted file in the build
context can go into the image, so a person must see each one first.

- Names in build paths compare without case, so a build path that a
  case-insensitive checkout accepts still matches. A build path with `..`, an
  absolute one, or one that passes a link in the working copy (the build
  follows links before it applies `..`) counts all of the checkout.
- `collect` lists every uncommitted file of the build context with
  `modified`, `new` or `deleted`: tracked files with their working-tree
  content, untracked files that Git does not ignore, and deletions, inside
  `build.context`, the `build.dockerfile` file and its own
  `<Dockerfile>.dockerignore`, which BuildKit reads.
- `Uncommitted::hash` covers each path, change, kind and content digest.
- `overlay` collects the files again, refuses another hash than the
  confirmed one, and checks each copied file against its digest.
- The confirmation (UI and `cloud_deploy plan-rebuild`) lists every file
  before a person confirms. A rebuild starts only with a confirmed plan
  (`replacement::rebuild` with `confirmed`); without one, any uncommitted
  file refuses it. The rebuild journals the hash, so **Continue rebuild**
  checks the same files. `cloud_deploy rebuild` takes the printed plan token, which covers the
  settings and the files (`Plan::token`).

## What never goes out

- Untracked files that Git ignores (`.gitignore`, `.git/info/exclude`, the
  global exclude file), also a file removed from the index and then ignored. A
  tracked file stays tracked when it also matches `.gitignore`, so its changes
  are listed like any other.
- The target of a symbolic link: a link is copied as a link. A file reached
  through a folder that is a link is refused. On Unix each folder on the way
  is opened without following a link, so this also holds when the folder is
  swapped while Horizon reads. A hard link is refused on Unix.
- A path part that is not one plain name (`..`, or on Windows a `\` or a
  drive) is refused, so nothing outside the checkout or the snapshot is read
  or removed.
- A working folder that the repository's configuration names elsewhere
  (`core.worktree`) is refused, so Git's view and the files read agree.
- Uncommitted files with the name of a common secret file (`secret_name`),
  and names with a control or text-direction character.

## What is not guaranteed

- File contents are not examined. A secret in a file with an ordinary name
  goes into the image if the person confirms it.
- `.dockerignore` is applied by Docker, not here, so the list can name a
  file that the image does not get.
- On Windows, a hard link is not refused, and a folder swapped for a junction
  between the check and the read can be followed. A folder that is already a
  symbolic link or a junction is refused; other reparse points, such as cloud
  placeholders, count as plain folders and files. A symbolic link is refused
  on Windows while the list is read, because it cannot be copied there.

## Untrusted: the repository's own configuration

A setup agent can write `.git/config`. The working copy is read with git2,
never with the `git` command line, so no configured program (content filter,
file system monitor) runs in Horizon's process.
