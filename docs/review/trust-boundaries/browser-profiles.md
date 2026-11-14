---
applyTo: "crates/horizon-browser/src/profile.rs,crates/horizon-browser/src/lib.rs,crates/horizon-core/src/browser/mod.rs,crates/horizon-core/src/browser/shared_session.rs,crates/horizon-core/src/session_store.rs,crates/horizon-core/src/panel.rs,crates/horizon-core/src/panel/spawn.rs,crates/horizon-core/src/config/browser_host.rs,crates/horizon-browser-control/src/manifest.rs,crates/horizon-browser-control/src/manifest/create.rs,crates/horizon-browser-mcp/src/model.rs,crates/horizon-browser-mcp/src/server.rs,crates/horizon-browser-cli/src/standalone.rs,crates/horizon-browser-cli/src/main.rs,crates/horizon-ui/src/app/actions/support.rs,crates/horizon-ui/src/app/browser_create_dispatch.rs,crates/horizon-ui/src/app/panels/interaction.rs,crates/horizon-ui/src/app/panels.rs,crates/horizon-ui/src/browser_widget/chrome.rs"
---

# Browser profiles

Read `docs/architecture/browser-profiles.md` before reviewing this boundary.

## What is not trusted

1. The `profile` field of a create. An agent, a script or a person writes it,
   and it decides two things: the name of a folder on this computer and which
   profile of the browser a new page reads. A profile holds the cookies and the
   site storage of the person's sign-ins.
2. The saved board. It names the profile of a panel, and a name in it comes back
   at the next start.
3. The pages of a shared profile. A page that the profile carries can read the
   cookies of that profile, and can sign the person out of every site.

## What the code guarantees

- A profile name is 1 to 40 of `a-z`, `0-9` and `-`, with a letter or a digit
  at each end (`horizon_browser::valid_profile_name`). The name is never a path,
  a shell word or an absolute path of its own.
- `private` is a reserved word. It names no profile, and the rule of
  `horizon_browser::valid_profile_name` refuses it, so the settings entry of a
  browser host refuses it too.
- A shared profile is a folder `browser-profiles/<id>` under the Horizon home,
  and the folder of a panel is `browser-profiles/<safe_local_id>`. The
  `safe_local_id` form starts with `%`, so a folder of a panel never equals a
  folder of a named profile.
- The folder of a shared profile is never removed by the close of a panel
  (`BrowserPanelState::close_permanently`) and never removed by the delete of a
  saved session (`session_store`). Only the folder of a private panel goes away,
  and only when its last panel closes.
- A private panel never joins the group of a shared profile, and a shared
  profile never takes the folder of a private panel.
- A create refuses, before the browser starts, a profile that the running
  browser of that profile cannot join: another engine, another launch setting,
  or a folder that another browser holds on Linux. The words name the profile
  and the action for the person.
- A create of a private panel on a browser host is refused, because the profile
  of that machine belongs to that machine.

## What the code does not guarantee

- No isolation between two panels of one shared profile. They are two pages of
  one browser, and they read the same cookies.
- A sign-out in one panel signs out every panel of that profile.
- Every panel of a profile writes its downloads into one folder of that
  profile.
- The switch of a saved panel to another engine fails, but an old folder of a
  per-panel profile of an earlier version is not removed. No sweep exists.
- A profile of a browser host is not under this boundary. Its folder is on that
  machine, the tunnel owns it, and this computer never removes it.
- The pre-check of a held folder reads the lock of a Chromium or Firefox on
  Linux. On another system the engine reports the lock itself, after it starts.
- The profile is no backup and no secret store of Horizon. A person who signs in
  accepts that the browser writes its own cookies into that folder.

## Rules for a review

- Do not accept a profile name that the rule refuses, and do not add a second
  rule for one caller.
- Do not send a private panel to a folder or a group of a shared profile.
- Do not add a path that removes a folder of a shared profile. The two paths
  that remove a folder today are the close of a panel and the delete of a saved
  session, and each has a test that keeps a shared folder on disk.
- Do not add an incognito context (`Target.createBrowserContext` or
  `browser.createUserContext`). A private panel is a throwaway folder, and a
  second isolation mechanism doubles the surface without a need.
- Do not read or copy the cookies of a profile into a log, a manifest or an
  MCP answer.
