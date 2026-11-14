---
applyTo: "crates/horizon-browser/src/profile.rs,crates/horizon-browser/src/lib.rs,crates/horizon-browser/src/session/startup.rs,crates/horizon-browser/src/session/startup/page.rs,crates/horizon-browser/src/session/shared.rs,crates/horizon-browser/src/webdriver/shared.rs,crates/horizon-core/src/browser/host_route.rs,crates/horizon-core/src/browser/mod.rs,crates/horizon-core/src/browser/private_profiles.rs,crates/horizon-core/src/browser/shared_session.rs,crates/horizon-core/src/browser/host_driver/**,crates/horizon-core/src/session_store.rs,crates/horizon-core/src/panel.rs,crates/horizon-core/src/panel/spawn.rs,crates/horizon-core/src/panel/spawn/browser.rs,crates/horizon-core/src/runtime_state.rs,crates/horizon-core/src/runtime_state/models.rs,crates/horizon-core/src/config/browser_host.rs,crates/horizon-browser-control/src/manifest.rs,crates/horizon-browser-control/src/manifest/create.rs,crates/horizon-browser-mcp/src/model.rs,crates/horizon-browser-mcp/src/server.rs,crates/horizon-browser-cli/src/standalone.rs,crates/horizon-browser-cli/src/main.rs,crates/horizon-ui/src/app/actions/support.rs,crates/horizon-ui/src/app/browser_create_dispatch.rs,crates/horizon-ui/src/app/browser_host_login.rs,crates/horizon-ui/src/app/panels/interaction.rs,crates/horizon-ui/src/app/panels.rs,crates/horizon-ui/src/browser_widget/chrome.rs,crates/horizon-ui/src/main.rs"
---

# Browser profiles

Read `docs/architecture/browser-profiles.md` before reviewing this boundary.

## What is not trusted

1. The `profile` field of a create. An agent, a script or a person writes it,
   and it decides two things: the name of a folder on this computer and which
   profile of the browser a new page reads. A profile holds the cookies and the
   site storage of the person's sign-ins. The `backend` field of the same create
   decides which engine of this computer serves the page when the browser host of
   the settings does not serve it.
2. The saved board. It names the profile of a panel, and a name in it comes back
   at the next start.
3. The pages of a shared profile. A page that the profile carries can read the
   cookies of that profile, and can sign the person out of every site.
4. The greeting of a link of a browser host, which names the page of the
   panel and asks for a private page or a page of the profile.

## What the code guarantees

- A profile name is 1 to 40 of `a-z`, `0-9` and `-`, with a letter or a digit
  at each end (`horizon_browser::valid_profile_name`). The name is never a path,
  a shell word or an absolute path of its own.
- `private` is a reserved word. It names no profile, and the rule of
  `horizon_browser::valid_profile_name` refuses it, so the settings entry of a
  browser host refuses it too.
- A shared profile is the folder `browser-profiles/<safe_local_id>` under the
  Horizon home. A page that keeps nothing is the folder
  `browser-profiles/private/<safe_local_id>`
  (`horizon_browser::PRIVATE_PROFILES_FOLDER`). The two namespaces are apart by
  their folder, so no name of a shared profile and no id of a panel can give
  the same folder.
- The folder of a shared profile is never removed by the close of a panel
  (`BrowserPanelState::close_permanently`) and never removed by the delete of a
  saved session (`session_store`). Only the folder of a private panel goes away,
  and only when its last panel closes.
- One sweep removes a folder that a run left behind
  (`horizon_core::browser::sweep_private_profiles`). It removes only a folder
  under the folder of the private pages that carries the mark of this program
  (`.horizon-private-panel`), that no saved session of this Horizon home names,
  and that no browser holds. It reads the folders of shared profiles to count
  them, and it never removes one.
- A private panel never joins the group of a shared profile, and a shared
  profile never takes the folder of a private panel.
- A create refuses, before the browser starts, a profile that the running
  browser of that profile cannot join: another engine, another launch setting,
  or a folder that another browser holds on Linux. The words name the profile
  and the action for the person.
- Every panel of one profile on a browser host is its own page of the one
  browser of that profile's session, and each link drives one page. A page that
  the session kept comes back to the link of its panel only, with its typed text
  and its scroll place. One session serves at most eight pages, and the next one
  reads the number and the way out.

## Where the page of a create runs

One rule reads the entry `browser_host` of the settings, the `profile` field and
the `backend` field of a create, and it decides the machine and the profile of
the new panel (`horizon_core::browser::new_browser_panel`). The code guarantees
these properties:

- A create with no `profile` field is a page of the profile of the browser host,
  in the engine of that host. A panel that a person makes writes no `profile`
  field, so it is a page of that host too.
- A create that names an engine other than the engine of the host is a panel of
  this computer in that engine, in the shared profile `default` of this computer
  when it names no profile, and in a private page when it names `private`. The
  answer of the create says in words that the page runs on this computer.
- A create that names a profile of the profile rule keeps a profile of this
  computer, and the answer says so. The host serves the one profile of its entry.
  The name `default` is such a name: the queue writes no default name into a
  create that named none, so the rule reads a create with no field and a create
  with the field `default` apart.
- A saved panel takes no part in the rule: it comes back on the host and in the
  profile that served it, and the entry of today's settings does not move it.
- The field `profile` reaches the rule once. The MCP tool, the host wire, the CLI,
  the picker, the shortcut, a link of a card and the item `Private panel` of a
  panel menu all read the same rule, so no path opens a panel that its own answer
  does not describe.

## A private panel of a browser host

A private panel of a browser host is a page of an isolated context of the
browser that already runs there: `Target.createBrowserContext` of the DevTools
protocol for Chromium, and `browser.createUserContext` of the driver protocol
for Firefox. The code guarantees these properties:

- The page runs on the other machine, in the browser process of the shared
  profile. The profile folder of that machine is the one that it already had.
- The page reads no cookie and no site storage of the shared profile, and the
  pages of the shared profile read none of it.
- The context goes at the close of the page: `Target.disposeBrowserContext` or
  `browser.removeUserContext`. A context whose page and link are gone does not
  stay: the driver removes it when its page ends or its session stops.
- The panel reports `private` as its profile, and the chrome of the panel shows
  `Private`.
- A private page whose link is gone for ten minutes is closed by the session of
  the host, and its context is removed with it. A shared page of the same session
  keeps its page and its context, and the session ends when it holds no page.

The code does not guarantee these properties:

- The page is not hidden from the machine of the host or from its owner. That
  machine, its browser process and its network see the traffic of the page, and
  its profile folder holds the disk cache that a private context writes there.
- A download of a private page goes to the download folder of that host, as a
  download of every page of that host does. It is not apart from the profile.
- A context that a browser of the host keeps after the page ended stays until
  the browser stops. Horizon asks the browser for the removal and holds no
  handle to a context that its process no longer reaches. The bound of ten
  minutes applies to a page of a session of Horizon, not to a browser of the host
  that another program started.
- The isolation is the isolation of the browser that the host runs: a defect of
  that engine in the separation of a browser context is not compensated here.

## What the code does not guarantee

- No isolation between two panels of one shared profile. They are two pages of
  one browser, and they read the same cookies.
- A sign-out in one panel signs out every panel of that profile.
- Every panel of a profile writes its downloads into one folder of that
  profile.
- The switch of a saved panel to another engine fails, but an old folder of a
  per-panel profile of an earlier version is not removed: it is not under the
  folder of the private pages, so no sweep reads it.
- A profile of a browser host is not under this boundary. Its folder is on that
  machine, the tunnel owns it, and this computer never removes it.
- The pre-check of a held folder reads the lock of a Chromium or Firefox on
  Linux. On another system the engine reports the lock itself, after it starts.
- The profile is no backup and no secret store of Horizon. A person who signs in
  accepts that the browser writes its own cookies into that folder.

## Rules for a review

- Do not accept a profile name that the rule refuses, and do not add a second
  rule for one caller.
- Do not send a private panel to a folder or a group of a shared profile, and do
  not move the folder of a private page out of `browser-profiles/private/`.
- Do not add a path that removes a folder of a shared profile. The paths that
  remove a folder today are the close of a panel, the delete of a saved session
  and the sweep of the folders of private pages, and each has a test that keeps
  a shared folder on disk.
- Do not add a second isolation mechanism for a local private panel: a local
  panel is a throwaway folder. A private page of a browser host uses the
  isolated context of that browser, because the profile of the host belongs to
  that machine and has no throwaway folder of this computer.
- Do not read or copy the cookies of a profile into a log, a manifest or an
  MCP answer.
