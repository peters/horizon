# Trust boundary audit

This page records the audit of issue #1787. The audit read each trust boundary
against the code at its `applyTo` paths on 2026-10-24. It gives the result and
the untrusted input, the guarantee and the limit of each boundary.

The boundaries moved from `.github/instructions/` to
[`trust-boundaries/`](trust-boundaries/) in the same change. The review workflow
reads them at the new path.

## Method

1. Resolve each entry of the `applyTo` front matter. Each entry must match at
   least one file, and no entry may use a brace list.
2. Read each boundary page against the code at its matched paths.
3. Examine each path that the text names in backticks: each one must exist.
4. Examine each section that the text names in a design page: each one must
   exist.
5. Examine each symbol, flag, limit, format version and contract marker that the
   text names: each one must exist in the code.
6. Compare the claims of the boundary with the code. Correct or remove a claim
   that the code contradicts.

The checks in [`README.md`](README.md) repeat steps 1 and 3 for every boundary.

## Result

- The audit examined 35 boundary pages, 1,688 lines, on 2026-10-24. The entries
  below are the pages that remain.
- Each `applyTo` entry matches at least one file.
- No claim contradicts the code at its matched paths.
- No page uses a design page or a section that is absent.
- No page names a limit, a format version or a contract marker that differs from
  the code.
- Two names in the text are defined just outside the `applyTo` paths of their
  boundary. No claim changes, and each caller is inside the paths:
  - `set_files`, an attachment action in
    `crates/horizon-browser-protocol/src/control.rs`.
  - `HostIo`, in `crates/horizon-ui/src/app/browser_host_io.rs`.
- One name of [`remote-browser-tunnel`](trust-boundaries/remote-browser-tunnel.md)
  is defined outside its paths: `host_port_from_ws_url`, the loopback rule of a
  `DevTools` URL, in `crates/horizon-browser/src/cdp.rs`. Its caller is
  `crates/horizon-browser/src/process/attach.rs`, inside the paths.
- No boundary is left without a group.

## What the audit does not show

- It examines the named mechanisms, paths, symbols, limits and versions. It
  does not derive each guarantee from each line of the code again.
- It cannot show that one review succeeds with the new paths. Only a pull
  request after this change can show that.
- It does not examine the design pages in `docs/architecture/`. A boundary cites
  the design page, and the design page keeps its own text.

## The boundaries

Each entry gives the boundary page, the input that it must not trust, the
guarantee that the code gives, and what the code does not guarantee. This page
keeps the entries of the boundaries that remain in the repository. The other
boundary pages went away with their features.

### Local host and private files

- [`remote-browser-tunnel`](trust-boundaries/remote-browser-tunnel.md)
  (added after this audit, with the SSH forward of #1780)
  - Untrusted: the host machine, its shell, the endpoint file that its browser
    writes, and the results of its page.
  - Guarantees: the settings entry refuses a target that `ssh` reads as an
    option, a space and a control character, and a profile off the folder-name
    rule, before any `ssh` starts; every value reaches the remote shell as one
    quoted word; the forward is loopback on both ends and the endpoint is
    validated as a loopback URL again after the local port replaces the remote
    one; every `ssh` call is bounded and can not ask a question, and none of them
    runs on the UI thread; the panel sends no `Browser.close`, sets no download
    path and refuses a file of this computer with `remote_file_upload_unsupported`.
  - Does not guarantee: every local account of the host machine can drive the
    loopback `DevTools` port and use the sign-ins of the profile; the browser
    keeps running after Horizon exits; the panels of one profile share its
    cookies and storage; a download stays on that machine and is not reported;
    the host key is the trust of the OpenSSH configuration of the person.
- [`standalone-remote-host`](trust-boundaries/standalone-remote-host.md)
  - Untrusted: the MCP client and its tool calls.
  - Guarantees: the client cannot supply an endpoint, a capability or a
    credential; the host claims only the requests of its own agent identity and
    host instance; a credential file that the group or other users can read or
    write is refused; no answer holds a credential value or a credential path.
  - Does not guarantee: another process of the same operating-system user can
    read the credential files and the runtime root.
- [`checkout-search`](trust-boundaries/checkout-search.md)
  - Untrusted: each home-folder entry and each Git metadata file, which can be a
    link, a loop, a pipe, a device, huge, or change while it is read.
  - Guarantees: the search starts no process, stays on its background thread,
    and applies its visit, depth, checkout and file-size bounds to the bytes
    that it reads; on macOS it opens nothing inside a guarded home folder.
  - Does not guarantee: one `read_dir`, `metadata` or open on a mount that hangs
    can block that background thread; a file swapped for a pipe between its
    check and its open can still be opened; a checkout can claim any origin.

### Browser page input

- [`browser-webauthn`](trust-boundaries/browser-webauthn.md)
  - Untrusted: the page message and the output of the authenticator command.
  - Guarantees: the origin, the `clientDataJSON` and the `crossOrigin` value
    come from the frame tree of the host; a frame whose ancestor has another
    origin is refused; only `https://` and `http://localhost` origins are
    served; an empty challenge is a `TypeError` and starts no command; the
    bridge is off unless `browser.webauthn_command` names a command.
  - Does not guarantee: there is no public suffix list, so a two-label parent
    such as `co.uk` passes; the host examines no signature, attestation or user
    verification; a page can detect the page script; a cross-site iframe in
    Chromium keeps the browser's own behavior; a popup in a new top-level
    context is not covered; a child process of the command can outlive a kill.
- [`browser-semantic-actions`](trust-boundaries/browser-semantic-actions.md)
  - Untrusted: the page's own rectangles, hit test, focus and the value it kept
    after a fill; the frame routes the panel recorded.
  - Guarantees: a selector and a fill value are quoted as data in every
    expression; each observation is one number, one boolean or one element name
    from a closed set, never page text; a click point is hit-tested inside the
    frame that dispatches it and a point another element owns, or that no
    element holds, is a typed failure; a click must leave focus in the frame
    document; a fill that the page did not keep is a typed failure, compared by
    the rendered text of an editable host; a frame route is host-owned.
  - Does not guarantee: a page can refuse a click it received and the action
    still reports completion; a page can move focus after taking a click; a
    same-process child shares the page's focus, so its focus cannot prove that
    one click arrived; a fill readback is an immediate postcondition.
- [`browser-downloads`](trust-boundaries/browser-downloads.md)
  - Untrusted: the page, the file names it suggests, the number of downloads it
    starts and the bytes it saves.
  - Guarantees: a download is written by the browser into the private
    (`0700`) folder of its panel, never into a path a page names; a reported
    name keeps only its last path component, loses control characters and is cut
    to 128 bytes; a report holds no download URL; rows are bounded to 64 and the
    rest are counted; a download a child frame or another page of the same
    browser starts is a row too, because that browser owns the folder; a session
    that could not observe downloads states `reporting: false`.
  - Does not guarantee: a page can fill the panel folder and Horizon limits no
    size and inspects no content; Chromium saves the second and later
    page-initiated downloads without the prompt that would let a person refuse
    them; a browser that refused the folder, or a Firefox older than its download
    events, saves without reporting; a panel that shares a browser profile shares
    that profile's folder; a canceled download can leave a partial file.

## Follow-up work

- Closed: `AGENTS.md` named `.github/` as the home of the review instructions,
  in the bullet that starts "Before the first review of work that reads
  untrusted input". That bullet now names `docs/review/` and [`README.md`](README.md),
  and the review section of `AGENTS.md` points to the same folder.
- `scripts/test_detect_ci_build_inputs.py` uses `.github/copilot-instructions.md`
  as a non-Rust fixture path. The fixture is synthetic and still passes. A later
  change can use another non-Rust path, as the issue says.
- One review must succeed with the new paths. Only a pull request after this
  change can show that.
