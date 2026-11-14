---
procedure: browser-profiles
feature: Browser profiles of browser panels
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Browser profiles test procedure

## 1. Purpose

This procedure tests that one shared browser profile keeps the sign-in of a
person, so a second panel is signed in and a restart keeps the sign-in. It also
tests that a private panel is signed in nowhere, that its folder is gone when it
closes, and that the chrome names the profile of a panel. A browser host gives
the same properties: several panels of one profile, and a private panel.

## 2. Applicability

- Candidate: the exact build of the pull request.
- Platforms: Linux and macOS. Tasks P01 to P05 run on local panels. Tasks P06 to
  P08 need a browser host of the setting **Browser host**.
- Engines: Chromium and Firefox. Do the tasks one time for each engine. Tasks
  P06 to P08 run one time for each engine of a browser host.
- This procedure does not test a remote device, or the profile of another
  computer beyond the page of a browser host.

## 3. Safety

> **CAUTION:** USE A TEST PAGE OF THIS PROCEDURE, NOT A REAL ACCOUNT. A shared
> profile keeps the cookies of a real site on this computer after the panel
> closes, and a sign-out there affects every panel of that profile.

## 4. Equipment and preconditions

- A local Chromium and a local Firefox with `geckodriver` on `PATH`.
- A local HTTP test page. It sets one cookie on load and shows the cookie in the
  page title and in the page body. `python3 -m http.server` serves the folder of
  that page.
- A task-owned isolated desktop that you watch in a live Device panel, from the
  [engine panels procedure](browser-engine-panels.md).

## 5. Setup

1. Write the test page `profile.html` in an empty folder of this computer. The
   page sets `document.cookie = "signin=yes"` on load and writes the result of
   `document.cookie` into the page title.

2. Start the server in that folder with `python3 -m http.server 8765`.

   Result: the server listens on `http://127.0.0.1:8765/`.

3. Start Horizon on the isolated desktop with the fixture of the
   [engine panels procedure](browser-engine-panels.md).

## 6. Tasks

### 6.1 P01 — A shared panel signs in, and a second panel is signed in

1. Open a browser panel of the engine under test. Go to
   `http://127.0.0.1:8765/profile.html`.

   Result: the chrome shows `Shared:` and `default`. The page title shows
   `signin=yes` after the page runs its script.

2. Ask an agent of the same workspace to call `browser_create` with
   `url: "http://127.0.0.1:8765/profile.html"` and no `profile`.

   Result: the new panel shows `Shared: default`, and its page title shows
   `signin=yes` without a new sign-in.

3. Read `browser_list`.

   Result: both panels report the same profile, and no panel reports a failure.

### 6.2 P02 — A private panel is not signed in

1. Right-click the first panel and select `Private panel`.

   Result: a new panel opens at the same address and shows `Private` in its
   chrome.

2. Read the title and the body of the private page.

   Result: the cookie of the shared profile is absent. The page sets its own
   cookie, and that cookie is no cookie of the shared profile.

3. Ask an agent to call `browser_create` with
   `url: "http://127.0.0.1:8765/profile.html"` and `profile: "private"`.

   Result: the new panel shows `Private`, and its page does not read the cookie
   of the shared profile.

### 6.3 P03 — A restart keeps the sign-in

1. Close the second panel of task P01, and keep the first panel open. Sign out
   in nothing.

2. Stop Horizon with the normal close of the window.

3. Start Horizon again on the same isolated desktop and open the saved session.

   Result: the shared panel is back in `Shared: default`, and its page reads the
   cookie of the earlier sign-in.

### 6.4 P04 — A closed private panel leaves no folder

1. Note the folder of the shared profile and the folder of the private panel
   under the browser profile root of this computer. The folder of a shared
   profile is `<root>/<safe id of the profile>`. The folder of a private panel is
   `<root>/private/<safe id of the panel>`, and it holds the file
   `.horizon-private-panel`.

2. Close the private panel with the normal close of the panel.

   Result: the folder of the private panel is gone. The folder of the shared
   profile is there.

3. Open a private panel again, and stop Horizon with the normal close of the
   window while the panel is open. Start Horizon again, and open a session that
   does not hold that panel.

   Result: the folder of that private panel is gone after the start, because no
   saved session names it. The folder of the shared profile is there.

4. Delete the saved session of this run from the session manager.

   Result: the folder of the shared profile is still there.

### 6.5 P05 — A refusal names the profile

1. Open a Chromium panel in `Shared: work` with `browser_create` and
   `profile: "work"`.

2. Ask an agent to call `browser_create` with `profile: "work"` and
   `backend: "firefox"`.

   Result: the create is refused. The words name the profile `work` and the
   engine of the panel that runs, and they ask to close the panels of that
   profile or to use the same setting.

3. Call `browser_create` with `profile: "Private"`.

   Result: the create is refused, and the words name the profile rule and the
   reserved word `private`.

### 6.6 P06 — Two panels of one profile on a browser host

Run this task with a configured browser host of the engine under test.

1. Ask an agent to call `browser_create` with
   `url: "http://127.0.0.1:8765/profile.html"` and no `profile`. Sign in on that
   page.

   Result: the panel shows the page of the host, and it reports the profile of
   the host.

2. Ask an agent to call `browser_create` again with the same `url` and no
   `profile`.

   Result: the second panel opens, and it reads the cookie of the sign-in of the
   first panel. Both panels report the same profile.

3. Do this task one time for Chromium and one time for Firefox. A Firefox host
   answers `page_limit_reached` after eight pages of one profile.

### 6.7 P07 — A private panel of a browser host

1. Right-click a panel of the browser host and select `Private panel`.

   Result: a new panel opens at the same address, on the same host, and its
   chrome shows `Private`.

2. Read the title and the body of the private page.

   Result: the cookie of the shared profile is absent. The private page sets its
   own cookie, and the panels of the shared profile do not read it.

3. Close the private panel. Read the log of the host, which is
   `~/.horizon/browser-profiles/<profile>/host-firefox.log` for a Firefox host
   and the log of the Chromium of that host for a Chromium host.

   Result: the log holds the line that the user context or the browser context
   is removed.

4. Open a private panel of the same host again, and read its page.

   Result: the new private page reads no cookie of the private page of step 3.

### 6.8 P08 — A lost link with two panels comes back to its own page

Run this task with a Firefox host.

1. Open two panels of one profile on the host, and go to a different page in each
   panel. Type text into a field of the first panel.

2. End the `ssh` child of the first panel, as task F3 of the
   [browser host procedure](browser-host-firefox.md) says.

   Result: the first panel says that the link is lost.

3. Wait for the countdown of the panel.

   Result: the first panel shows its own page, and the text that you typed is
   still in the field. The second panel shows its own page, and it is not
   reloaded.

### 6.9 P09 — The rule of a new panel

Run this task with a browser host of the engine under test.

The rule of one new browser panel reads the entry **Browser host** of the
settings, the field `profile` and the field `backend` of the create:

| The create | The panel | Task that proves it |
|------------|-----------|---------------------|
| no `profile` | a page of the profile of the host, in the engine of the host | P06 step 1, P09 step 1 |
| no `profile`, and a `backend` of another engine | a page of this computer in that engine, in the shared profile `default`, and `note` says that it runs on this computer | P09 step 2 |
| `profile: "private"` | a private page of the host, in the engine of the host | P07, P09 step 3 |
| `profile: "private"`, and a `backend` of another engine | a private panel of this computer, and `note` says that it runs on this computer | P09 step 4 |
| a profile name | a panel of this computer in that profile, and `note` says that it runs on this computer | P09 step 5 |

Without a browser host, every create is a panel of this computer in the profile
and the engine that it names. The field `note` is absent when the page runs
where the settings name it.

1. Make a panel from the picker, and then ask an agent to call `browser_create`
   with no `profile` and no `backend`.

   Result: both panels run on the host, in the profile of the entry. Read
   `browser_list`: each panel reports the profile of the host, and no panel has
   `note`.

2. Ask an agent to call `browser_create` with `backend` of an engine that the
   host does not run, and no `profile`.

   Result: the panel runs on this computer. Its `profile` is `default`, and
   `note` says that the page runs on this computer and not on the browser host.

3. Right-click a panel of this computer and select `Private panel`. Then
   right-click a panel of the host and select `Private panel`.

   Result: the first panel is a private panel of this computer with
   `profile: "private"`, and the second is a private page of the host, in the
   engine of the host.

4. Ask an agent to call `browser_create` with `profile: "private"` and a
   `backend` of an engine that the host does not run.

   Result: the panel is a private panel of this computer, and `note` says so.

5. Ask an agent to call `browser_create` with `profile: "work"`.

   Result: the panel runs on this computer in the shared profile `work`, and
   `note` says so.

The checks of the repository prove the same lines without a host:
`app::browser_host_route_tests::a_create_with_no_profile_is_a_page_of_the_host_for_an_agent_and_for_a_person`,
`app::browser_host_route_tests::a_create_with_no_profile_that_names_another_engine_is_a_panel_of_this_computer`,
`app::browser_host_route_tests::a_private_create_is_a_private_page_of_the_host`,
`app::browser_host_route_tests::a_private_create_of_another_engine_is_a_private_panel_of_this_computer`, and
`app::browser_host_route_tests::a_create_that_names_a_profile_keeps_a_profile_of_this_computer`.

## 7. Pass criteria

- A second panel of the shared profile reads the cookie of the first panel, for
  each engine that starts on the platform of the run.
- A private panel reads no cookie of the shared profile.
- A restart puts a shared panel back in the same profile, in `Shared: default`.
- The close of a private panel removes its folder, and no close and no session
  delete removes the folder of a shared profile. A folder of a private panel
  that a run left behind is removed at the next start when no saved session
  names it.
- A browser host gives two panels of one profile that read the same sign-in, and
  a private panel that reads none of it.
- The chrome names the profile of each panel in one line.
- A create with no `profile` runs on the browser host of the settings, and
  `browser_list` reports the profile of that host for it.
- Each refusal names the profile, or the rule that it breaks, and the action
  that the person can take.

## 8. Cleanup

1. Close each test panel and the saved session of this run.

   Result: no test panel is open, and the session manager lists no session of
   this run.

2. Stop the HTTP server with `Ctrl-C`, and remove the folder of the test page.

   Result: the folder and the server are gone.

3. Stop the isolated desktop and close its Device panel.

   Result: the fixture processes of this run are gone.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
