# Browser profiles

A browser panel opens one page in a browser profile. A person signs in one time,
and every panel of the same profile stays signed in. A private panel opens its
own throwaway profile instead, so a person can look at a site without the
sign-ins of the shared profile.

The trust boundary is
[`docs/review/trust-boundaries/browser-profiles.md`](../review/trust-boundaries/browser-profiles.md).
The repeatable check is
[`docs/testing/procedures/browser-profiles.md`](../testing/procedures/browser-profiles.md).

## Two kinds of panel

| Kind | Profile folder | Deleted at close | Signed in |
|------|----------------|------------------|-----------|
| Shared `default` | `browser-profiles/%<default>` | Never | With every panel of `default` |
| Shared `<name>` | `browser-profiles/%<name>` | Never | With every panel of `<name>` |
| Private | `browser-profiles/%<panel id>` | Yes, with its last panel | Never |

The folder name uses the existing `safe_local_id` form, which starts with `%`.
A named profile and a panel therefore never get one folder.

## What a create names

`browser_create` takes one optional field, `profile`:

- omitted: the shared profile `default`;
- a name of the profile rule: that shared profile;
- the reserved word `private`: a folder of this panel alone.

A panel that a person creates from the picker, from a shortcut or from a link
uses `default`. The context menu of a browser panel has one item,
`Private panel`, that uses `private` and keeps the URL of the panel.

The field reaches the same place for the MCP tools, the host wire, the CLI and
the UI: the profile name becomes the session id of the panel
(`PanelOptions::browser_session_id`), which the board already persists and
restores.

## One browser process, one page more

A shared profile is one browser process, and each panel of it is one page. This
is the existing `SharedBrowserSession` for Chromium and `SharedFirefoxSession`
for Firefox; `browser_duplicate` was its only caller before this change.

A create that cannot join the browser of that profile is refused before the
browser starts, with words that name the profile:

- the running browser of the profile is of the other engine;
- the launch settings of the create differ from the pinned settings of the
  running browser (headless, the executable, the extra arguments, the
  disclosure of automation, background windows);
- the folder holds a lock of a browser process on Linux.

## Two removal paths, and the rule that keeps a profile

Two paths remove a profile folder: the close of a panel
(`BrowserPanelState::close_permanently`) and the delete of a saved session
(`session_store`). Each removes the folder of a private panel and neither
removes the folder of a shared profile.

A panel keeps its kind and its profile across a restart, because the saved board
names the profile.

## The limits that a person meets

- Two panels of one shared profile are two pages of one browser: there is no
  isolation between them, and a sign-out in one signs out both.
- Every panel of a profile writes its downloads into one folder of the profile.
- The profile of a Chromium browser host is shared by its nature. A private
  panel on a browser host is refused.
- The Firefox driver of a browser host serves one page for one profile. A second
  panel of that profile is refused with words.
- A folder of a per-panel profile of an earlier version stays on disk. No sweep
  exists.
