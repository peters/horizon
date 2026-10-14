# Checkout search boundary

**Where is your code?** in **New cloud** offers the Git checkouts on this
computer by name (`horizon_core::dir_search::checkouts`), and the folders of a
typed path. The user rules are in "Your repositories in New cloud" in
`docs/cloud-workspaces.md`.

## Untrusted input

- Every entry below the home folder: names, links, file types and sizes. A
  folder can be a link, a loop, a mount that hangs, or hold very many entries.
- A checkout's Git metadata: `.git` (folder, file or link), `HEAD`, `commondir`,
  `config`, ref files, `packed-refs` and the clone marker. Any program or a
  checked-out repository can write them, and they can change while they are read.
- The origin address in a checkout's config. It is text to show and to match
  against the connected account's repositories, never a trusted identity.

The home folder is `HOME`, else `USERPROFILE`, the first that is set and not
empty, on every platform. Without one, nothing is searched.

## What the search guarantees

- It runs on a background thread and starts no process: Git metadata is read
  from files only, so no configured Git program runs.
- It reads at most 50,000 folder entries (`VISITS`), an entry that fails to read
  counted too, keeps at most 2,000
  checkouts (`LIMIT`) and goes at most four folders below the home folder
  (`DEPTH`). Each folder is read one time, also when links lead to it again.
- Hidden folders are not searched, and build folders (`SKIP_NAMES`) are not
  searched below.
- Only regular files are opened, never a pipe or a device. A metadata file is
  read to at most 256 KiB, and a longer one is not used; `packed-refs` to at
  most 64 MiB. The config and clone marker reads in
  `cloud_runtime::repository::source` stop at their limits in the same way.
- On macOS nothing inside the home folders that macOS guards with a privacy
  prompt (`GUARDED`) is opened, also through links: each link is followed one
  step at a time and refused when a step enters a guarded folder. A checkout
  whose repository lies there is listed with nothing read.
- At most two searches run at the same time across all dialogs. A closed dialog
  stops its search at the next report of what was found, about once a second.
- A checkout counts as finished only when no clone into it stopped and its
  `HEAD` is an object name (40 or 64 hexadecimal digits) or names a branch
  whose ref file holds one or that `packed-refs` lists. A clone marker that is
  there but cannot be read, or is too long, counts as a stopped clone. Only a
  finished checkout stands in for a repository row or a clone; an unreadable
  marker is never a reason to remove a folder.
- A branch named in `HEAD` is read only when it is plain names joined by `/`:
  no `.` or `..` part, no `\`, `:`, drive or root, so its text cannot name a
  place outside the Git directory, also on Windows. A Windows device name such
  as `CON` is looked at but never opened, as it is no regular file.

## The folders of a typed path

- They are listed on background threads, at most four running, also when a
  listing hangs; the field and its keys work on meanwhile.
- The typed folder is listed whole, with no cap on its entries, and each listed
  folder is read as a checkout (`checkout_in`) to mark repositories.
- On macOS a guarded folder itself, or a link into one, is no checkout, so
  marking repositories while `~/` is listed reads nothing inside them. A path the person types into a
  guarded folder is looked at there (its `.git` entry only), and macOS can then
  ask; a link in it that leads into another guarded folder is not followed. A
  typed path with a `..` part is not read as a checkout on macOS, since `..`
  after a link leads where the link goes.

## What is not guaranteed

- A single `read_dir`, `metadata` or open on a mount that hangs can block that
  background thread. The cap on running searches keeps such threads from
  adding up; the dialog does not wait for them.
- A file swapped for a pipe between its check and its open can still be opened.
- The listing of a typed path (`dir_search::lookup`, shared with the folder
  picker) checks each entry with a call that follows links, and a typed path
  through a link is listed where the link leads. On macOS a link from the home
  folder into a guarded folder can therefore be looked at, though no checkout
  is read there.
- The object that `HEAD` or a ref names is not looked up, so a checkout with a
  made-up object name counts as finished.
- The origin, branch and folder names are shown as written. A checkout can claim
  any origin, so a checkout of another repository can stand in for an account
  repository of the same name.
- Repositories that keep refs in reftable are never counted as finished.
- Planning and running a clone use Git as before: `git rev-parse` in a folder
  that already exists at the clone's place, then `git clone`. Those steps are
  outside this boundary.
