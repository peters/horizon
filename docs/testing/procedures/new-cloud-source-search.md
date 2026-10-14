---
procedure: new-cloud-source-search
feature: New cloud source field, search over checkouts and GitHub repositories, folder paths and links
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# New cloud source search test procedure

## 1. Purpose

This procedure proves that **Where is your code?** in the **New cloud** dialog is
one search field. It finds Git checkouts on this computer and the repositories of
the connected GitHub App by fuzzy search, lists the folders of a typed path, and
takes a pasted link. It also proves the keyboard: the arrow keys, **Enter** and
**Tab**.

## 2. Applicability

- Candidate: each candidate that changes the source field of **New cloud**, the
  checkout search in `horizon-core` or the fuzzy ranking.
- Platforms: Linux, in an isolated desktop with a live view.
- This procedure does not test: a clone, the sign-in of this computer to GitHub
  (the [Connect GitHub procedure](connect-github.md) tests it) or a cloud start.

## 3. Safety

> **CAUTION:** DO NOT CLICK **Start cloud**. This button rents compute from the
> provider. The provider charges money until somebody deletes the worker.

> **CAUTION:** USE SYNTHETIC REPOSITORIES ONLY. A recording shows every name in
> the result list.

## 4. Equipment and preconditions

- The [local device smoke fixture](../../../scripts/device-smoke/README.md)
  with `--native-view`, and a Device panel with a live view of it.
- A frozen candidate and its SHA-256.
- In the private home of the fixture, `<home>`:
  - Git checkouts with an `origin` on github.com: `<home>/github/acme/web`
    (`https://github.com/acme/web.git`, changed most recently) and
    `<home>/github/acme/mobile-app` (`git@github.com:acme/mobile-app.git`, on the
    branch `feature/offline-sync`).
  - A Git checkout without `origin`: `<home>/code/dotfiles`.
  - A Git checkout with a GitLab `origin`: `<home>/projects/gitlab-pipeline`
    (`https://gitlab.com/acme-group/platform/gitlab-pipeline.git`).
  - A plain folder: `<home>/code/scratch`.
- A connected GitHub App in the cloud settings of the fixture whose list, signed
  in or kept, has `acme/web`, `acme/web-board`, `acme/website-legacy` and
  `northwind/billing-api`, and no checkout of the last three.

## 5. Setup

1. Start the fixture with the frozen candidate and a new state directory.

   Result: The Device panel shows the candidate on the isolated desktop.

2. Click **Cloud** in the toolbar, then **Cloud** in the menu.

   Result: The **New cloud** dialog opens. The field **Where is your code?** has
   the focus and no **Browse…** button.

## 6. Tasks

### 6.1 S01 — Recent checkouts first

1. Examine the results below the empty field.

   Result: The caption says **RECENT ON THIS COMPUTER, THEN GITHUB**. The first
   row is `acme/web` with the path, the branch, the time of the last change and
   the badges **GitHub** and **on this computer**. `gitlab.com/acme-group/...`
   and `dotfiles` show **on this computer** only. The rows of GitHub
   repositories without a checkout follow, with **GitHub** and **clone**.

### 6.2 S02 — Fuzzy search

1. Type `acwb`.

   Result: The rows include `acme/web-board`, `acme/web` and
   `acme/website-legacy`. The typed letters are lit in each name. `acme/web`
   shows one time, as the checkout, with both badges.

2. Type a space and `zzz`.

   Result: The dialog says **Nothing matches. Paste a link to clone any
   repository.**

### 6.3 S03 — Keyboard choice of a checkout

1. Select all the text in the field and type `nwbil`.

   Result: One row, `northwind/billing-api`, is highlighted.

2. Push **Down**, then **Up**.

   Result: The highlight stays on the row and the list does not scroll away.

3. Select all the text in the field and type `web`. Push **Down** until
   `acme/web` with **on this computer** is highlighted, then push **Enter**.

   Result: The field shows `~/github/acme/web` and the dialog reads the
   repository. The cloud title becomes `web`. No folder picker opens.

### 6.4 S04 — Choice of a GitHub repository without a checkout

1. Close the dialog and open **New cloud** again. Type `wbbd` and click
   `acme/web-board`.

   Result: The field shows `https://github.com/acme/web-board`. The results go
   away and the clone row shows **CLONE INTO** with **Choose folder…**.

### 6.5 S05 — Folders of a path

1. Select all the text in the field and type `~/`.

   Result: The caption says how many folders show. Hidden folders do not show.
   Folders with a repository come first.

2. Type `co`, then push **Tab**.

   Result: The field shows `~/code/` with the cursor at its end. The rows show
   `dotfiles` (a repository) before `scratch`.

3. Push **Down** to highlight `scratch` and push **Enter**.

   Result: The field shows `~/code/scratch/`. **Enter** on a folder without a
   repository opens it.

4. Push **Backspace** until the field shows `~/code/`, highlight `dotfiles` and
   push **Enter**.

   Result: The field shows `~/code/dotfiles` and the dialog reads the
   repository.

5. Type `~/.` in an empty field.

   Result: Hidden folders show now.

### 6.6 S06 — Tab through the dialog

1. Clear the field and push **Tab** several times.

   Result: The focus moves from the field to the next control of the dialog. No
   result row takes the focus.

### 6.7 S07 — Edge cases

1. Click the field **Where is your code?**, select all its text, type `~/code/` and
   push **Shift+Tab**.

   Result: The focus moves back to the control before the field. No folder opens.

2. Click the field, select all its text and type `..`.

   Result: The rows show the folders of the parent of `<home>`, not a search.

3. Select all the text in the field and type `.git`.

   Result: The field is a search, not a path: the caption says **MATCHES** or the
   dialog says **Nothing matches**.

4. Select all the text in the field and type `~/github/acme/`. Highlight `web` and
   push **Tab**.

   Result: The field shows `~/github/acme/web/`. The first row is **Use this
   repository**, and the rows below it show its folders. The dialog does not use
   the repository until you push **Enter** on a row or delete the last `/`.

5. Click the field and clear it. While the rows show, push **Down** two times, then wait for
   the spinner beside the counts to stop.

   Result: The highlight stays on the same row, also if rows move.

## 7. Pass criteria

- The dialog has no **Browse…** button for the repository.
- Words find checkouts and GitHub repositories in one list, ranked with the
  closest name first, with the matched letters lit.
- A repository that is on GitHub and on this computer shows one time and uses
  the checkout.
- A path lists its folders; **Tab** opens a folder and **Enter** uses a
  repository.
- The arrow keys move the highlight, and **Tab** leaves the field when no folder
  is highlighted.

## 8. Cleanup

1. Click **Cancel** and stop the fixture.

   Result: The fixture removes its state directory.

## 9. Record of results

Put the results in the pull request. Keep private evidence out of the
repository.
