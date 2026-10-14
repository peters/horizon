---
applyTo: "crates/horizon-core/src/cloud_runtime/repository/uncommitted.rs,crates/horizon-core/src/cloud_runtime/repository/uncommitted/**"
---

# Trust boundary of the uncommitted build context

`repository::uncommitted` lists, hashes and copies the uncommitted files of an
image recipe's build context, so that **Rebuild image & restart** can build
the image from the working copy. The rebuild wiring that calls it, and its
user documentation, come in a follow-up PR.

## What goes out

The image can be pushed to a public registry, for example
`ghcr.io/peters/horizon-development`. Every uncommitted file in the build
context can go into the image, so a person must see each one first.

- Build paths resolve `.` and `..` as the build does, and names compare
  without case, so a build path that a case-insensitive checkout accepts
  still matches; a path that leaves the checkout counts all of it.
- `collect` lists every uncommitted file of the build context with
  `modified`, `new` or `deleted`: tracked files with their working-tree
  content, untracked files that Git does not ignore, and deletions, inside
  `build.context` and the `build.dockerfile` file.
- `Uncommitted::hash` covers each path, change, kind and content digest.
- `overlay` collects the files again, refuses another hash than the
  confirmed one, and checks each copied file against its digest.

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
  junction or another reparse point is refused. A symbolic link is refused
  on Windows while the list is read, because it cannot be copied there.

## Untrusted: the repository's own configuration

A setup agent can write `.git/config`. The working copy is read with git2,
never with the `git` command line, so no configured program (content filter,
file system monitor) runs in Horizon's process.
