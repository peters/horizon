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
| Private | `browser-profiles/private/%<panel id>` | Yes, with its last panel | Never |

The folder name uses the existing `safe_local_id` form, which starts with `%`.
A private page lives under the folder of the private pages, so the folder of a
shared profile and the folder of a private page never share a name.

## What a create names

A create takes one optional field, `profile`, and one optional field, `backend`.
One rule decides the machine, the engine and the profile of the panel:
`horizon_core::browser::new_browser_panel`
(`crates/horizon-core/src/browser/host_route.rs`). Every path reads that rule:
the agent tool `browser_create`, the picker, the shortcut and a link of a card,
and the item `Private panel` of a panel menu. The panel that opens, the profile
that it saves and the answer of the create therefore cannot disagree.

The rule reads the entry `browser_host` of the settings, the `profile` field and
the `backend` field. A panel that a person makes from the picker writes no
`profile` field and names no engine, so the engine of its preset is the configured
engine of this computer and not a create field.

The rule reads the two fields as the caller wrote them. The queue of the host
keeps a create that names no `profile` as a create with no field, so a create that
names no profile and a create that writes the name `default` are two requests:
the first asks for the profile of the browser host, and the second asks for the
shared profile `default` of this computer.

### With a browser host in the settings

| The create | The panel | The answer |
|------------|-----------|------------|
| no `profile` | a page of the profile of the host, in the engine of the host | no note |
| no `profile`, and a `backend` other than the engine of the host | a page of this computer in that engine, in the shared profile `default` of this computer | a note that says that it runs on this computer |
| `profile: "private"` | a private page of the host, in the engine of the host | no note |
| `profile: "private"`, and a `backend` other than the engine of the host | a private panel of this computer in that engine | a note that says that it runs on this computer |
| a name of the profile rule | a panel of this computer in that shared profile | a note that says that it runs on this computer |
| `profile: "default"` | a panel of this computer in the shared profile `default` | a note that says that it runs on this computer |

A page of this computer in another engine is how a person checks a page in
Chromium while the host runs Firefox. A named profile stays on this computer,
because the host serves the one profile of its entry.

### Without a browser host

| The create | The panel |
|------------|-----------|
| no `profile` | a page of this computer in the shared profile `default` |
| `profile: "private"` | a private page of this computer |
| a name of the profile rule | a page of this computer in that shared profile |

The engine of the panel is the `backend` field when the create names one, and
the configured engine of this computer otherwise.

A saved panel takes no part in the rule: it comes back on the machine and in the
profile that served it, and the entry of today's settings does not move it.

The field reaches the same place for the MCP tools, the host wire, the CLI and
the UI: the profile name becomes the session id of the panel
(`PanelOptions::browser_session_id`), which the board already persists and
restores, and the machine of the page becomes `PanelOptions::browser_place`.

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

A third path removes a folder that a run left behind: the sweep of the folders
of private pages at the next start
(`horizon_core::browser::sweep_private_profiles`). It runs on a worker thread,
so the window never waits on the disk. It removes only a folder under
`browser-profiles/private/` that carries the mark `.horizon-private-panel`, that
no saved session of this Horizon home names, and that no browser of this
computer holds. The folder of a shared profile sits at the root of the profiles,
so the sweep reads it to count it and never removes it.

A panel keeps its kind and its profile across a restart, because the saved board
names the profile.

## A browser host

A browser host serves the browser of one profile of that machine. Every panel of
that profile on the host is its own page of the one browser, for Chromium and
for Firefox:

- a Chromium host opens the page beside the pages of the profile in that
  browser;
- a Firefox host runs the driver of
  [`crates/horizon-core/src/browser/host_driver/`](../../crates/horizon-core/src/browser/host_driver/mod.rs),
  and one session of that driver serves every panel of its profile.

One link drives one page. A link names the page of its panel, so a link that
comes back after a loss attaches to the page that it had, with its typed text and
its scroll place, and never to the page of another panel. A panel that closes
ends its page, and the session and its browser end with the last page. One
session serves at most eight pages (`pages::MAX_PAGES`): eight pages keep the
memory of the host under about 1.5 GB, and the next create reads the number and
the way out.

A private panel of a browser host is a page of an isolated context of the browser
that already runs there, so it needs no second browser process and no folder of
this computer: `Target.createBrowserContext` and
`Target.disposeBrowserContext` of the DevTools protocol for Chromium, and
`browser.createUserContext` and `browser.removeUserContext` of the driver
protocol for Firefox. The context reads no cookie and no site storage of the
profile, and the driver removes it when its page ends.

The panel asks for the context, and the session of the page proves it
(`horizon_browser::PageIsolation`, one fact of the session and not a copy of the
request). A page of this computer proves a folder of its own instead
(`PageIsolation::PrivateFolder`), and a page that no session proved is
`PageIsolation::Shared`: the chrome shows `Private` only for a page that keeps
nothing, and `browser_create` reports `private_page_unavailable` when the browser
of the host refuses a context, with no page of the shared profile opened in its
place.

A private page that comes back after a lost link comes back inside its own
context: the panel carries the page target and the context id, and the session
adopts that page only when the browser still holds both. A browser that restarted
holds no such context, so the session makes a new context and a new page, and the
panel says that the private page was lost. A page of the default context is never
adopted for a private panel.

## The limits that a person meets

- Two panels of one shared profile are two pages of one browser: there is no
  isolation between them, and a sign-out in one signs out both.
- Every panel of a profile writes its downloads into one folder of the profile.
- A private panel of a browser host runs on that machine, in the browser of the
  shared profile: the machine and its owner see its traffic, and a download of
  the page goes to the download folder of that host.
- A folder of a per-panel profile of an earlier version stays on disk: it is not
  under the folder of the private pages, so no sweep reads it.
