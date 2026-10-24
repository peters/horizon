---
applyTo: "crates/horizon-browser-cli/src/standalone/profile.rs,crates/horizon-browser-cli/src/standalone/profile/**,crates/horizon-browser-cli/tests/standalone_profile.rs,crates/horizon-browser-mcp/src/screenshot.rs,crates/horizon-browser-mcp/src/screenshot/**"
---

# Trust boundary of the profile host

A profile host keeps named browser profiles for `horizon-browser mcp --connect`
clients on the same computer. The design is in the section "Profile host" of
`crates/horizon-browser-cli/README.md` and in the module comment of
`crates/horizon-browser-cli/src/standalone/profile.rs`.

## Trusted: the host file

The host file sets the engine, the browser arguments and the Firefox
preferences of each browser profile.

- The host refuses a host file that the group or other users can write, and a
  host file of more than 256 KiB.
- The host refuses an argument that the engine manages: the profile folder,
  the automation endpoint and the headless mode.
- The host file holds no credential.

## Untrusted: each connect client

A connect client writes create, close and visibility requests into the
private coordination files of the browser runtime root.

- The host claims only the requests of its own host instance.
- A create must come from a `horizon:` identity. It must name a browser
  profile of the host file. The host refuses a remote target, a backend and an
  orientation.
- The host stamps each page for the one actor that opened it. Another actor
  can not list, control, close or duplicate that page.
- One actor holds at most 4 pages. The host runs at most 16 pages, and only an
  actor with no page gets the 16th page.
- A page with no action for 60 minutes closes.
- A screenshot goes only to the actor that holds the live claim of the page.
  The host writes it as a PNG of mode 600 under the browser runtime root and
  keeps at most 8 for each page.
- `mcp --connect --inline-screenshots` reads the file that a screenshot result
  names only when it is a `.png` under the browser runtime root, of at most
  8 MiB. It adds no image for any other path.

## Not protected

- The pages of one browser profile share its cookies and its storage. Each
  actor that names a browser profile gets its sign-ins. That is the purpose of
  a browser profile.
- A process of the same operating-system user can read the browser runtime
  root and the profiles, and can write requests as any actor. Run the host as
  a user that untrusted processes can not become.
- The host instance in `profile-host.json` is not a secret.

## Lifecycle that a review must keep

- The host never deletes a browser profile. A stop closes each page and lets
  each browser write its profile.
- Only one host runs on a browser runtime root. Its host instance stays the
  same over a restart.
- The host writes `profile-host.json` only when it serves requests.
