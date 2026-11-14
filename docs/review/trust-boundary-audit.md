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
    the host key is the trust of the OpenSSH configuration of the person. A
    panel that loses its browser keeps its place and publishes one typed reason
    with the words of that machine for the agents of its workspace.
  - The browser machine lane (`packaging/browser-machine/`) publishes one Docker
    port on the loopback of the destination and installs a wrapper as
    `google-chrome`, which shadows a real browser name there; Chromium runs as
    root with no sandbox and with software graphics, so its profile files belong
    to root and a local account of that machine drives it.
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
- [`markdown-preview-images`](trust-boundaries/markdown-preview-images.md)
  (added after this audit, with the pictures of a markdown preview)
  - Untrusted: the text of a note, which decides the path, the number and the
    content of every picture that the preview reads.
  - Guarantees: no network request for a picture, and one line of text for a
    picture with a scheme other than `file:`; only a regular file, typed on the
    open handle; at most 16 MiB of one file, checked on the open handle before
    the first read; at most 16 777 216 decoded pixels, checked on the header
    before the decode; the PNG and JPEG formats only; no read, file type or
    decode on the UI thread; at most 32 pictures of one note; one bounded cache
    that ends with the panel.
  - Does not guarantee: the correctness of a decoder against a hostile file,
    beyond the memory safety of safe Rust and its CPU time inside the length
    and pixel limits; that the file is the picture that the author intended.

### Browser page input

- [`browser-profiles`](trust-boundaries/browser-profiles.md)
  (added with the shared profile and the private panel)
  - Untrusted: the `profile` field of a create, which an agent or a person
    writes, and the profile name of a saved board; also the pages of a shared
    profile, which can read its cookies and sign the person out of every site.
  - Guarantees: a profile name follows one rule of 1 to 40 of `a-z`, `0-9` and
    `-`, and the reserved word `private` names no profile there, in the
    settings entry and in the CLI; the folder of a named profile and the folder
    of a panel never collide; a private panel never joins the group or the
    folder of a shared profile; the folder of a shared profile is removed by
    neither the close of a panel nor the delete of a saved session; a create
    that cannot join the running browser of its profile is refused before the
    browser starts, with words that name the profile; a private panel on a
    browser host is refused.
  - Does not guarantee: isolation between two panels of one shared profile, so
    a sign-out signs out all of them; one download folder for each profile; the
    removal of a folder of a per-panel profile of an earlier version; a profile
    of a browser host, whose folder belongs to that machine; the lock of a
    browser on another system than Linux, which the engine reports after it
    starts.
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
- [`agent-terminal-open`](trust-boundaries/agent-terminal-open.md)
  - Untrusted: the host, the user, the port, the remote command, the title and
    the machine key name that an agent sends to `open_terminal`, the name of the
    preset, the folder and the title that it sends to `open_panel`, and the key
    name of `machine_key` and `delete_machine_key`.
  - Guarantees: the route is an object with three fields and refuses every other
    field, so no identity file, proxy or ssh option arrives; a host and a user
    pass the same label check as a Device viewer route, a port is 1 to 65535, and
    a remote command is one trimmed line of 512 characters from a fixed set with
    no leading `-`; a key name is 1 to 40 characters of `a-z`, `0-9` and `-` with
    no `-` at either end, and it is never a path; the same checks run in the MCP
    server and again in the UI; Horizon builds the argv itself with `-t` for a
    remote command, with `-i`, `IdentitiesOnly=yes`, `IdentityAgent=none`, a
    per-key known-hosts file and `accept-new` when a machine key is named, and
    the command last; the mark in the titlebar is Horizon's own text, saved with
    the panel; one agent panel opens at most eight terminals in one run; keys
    belong to the Horizon home, stay in a `0700` directory with a `0600` private
    key, are made and read on the coordination worker, and only the public line
    reaches an answer; a machine record passes the same route rule, a session
    name of `A-Z`, `a-z`, `0-9`, `_` and `-`, a key name and a 60-character
    provider line, holds no command line, is re-validated at click time, and its
    click runs the argv that Horizon builds from the record; the record holds no
    key name, so the binding of a key to a machine of this computer is a separate
    `0600` file (`machines/keys.json`) that a record of an older Horizon fills on
    the first load.
  - Does not guarantee: the agent chooses the host and the command, and it names
    the machine of a record, so a click reaches a host of its choice with the
    account in the record; any agent panel of this Horizon may use any machine
    key by name; the command runs with the account the person logs in as; the host key question
    and the login wait for the person in that terminal, or are pinned on first
    contact when a machine key is named; the text the person types there leaves
    this computer; the private key is a file that a local agent running as the
    person can read; Horizon does not put the public key on the machine; and the
    panel is an ordinary SSH terminal afterwards that no operation of
    `block_panel` reads or changes. The `open_panel` half matches the preset
    name and alias exactly, so the panel runs the command, the arguments and the
    kind that the person saved and no field of the request; a preset of a kind
    that runs no local program is refused, the folder is an absolute path of at
    most 1 024 bytes that the coordination worker checks, and one agent panel
    opens at most eight panels in ten minutes.

- [`named-secrets`](trust-boundaries/named-secrets.md)
  (added after this audit, with the named secrets of the Settings window)
  - Untrusted: the name and the value that a person types into the Settings
    window, and a secret row of `named/<NAME>` that a server sends to a computer.
  - Guarantees: a name is 1 to 64 characters of `A-Z`, `0-9` and `_` that starts
    with `A-Z`; the names that Horizon, the agents and the system own (`PATH`,
    `HOME`, `LD_*`, `SSH_*`, `HORIZON*`, `CLAUDE*`, `XDG_*` and the rest of the
    list) are refused; a value is 1 to 16 KiB of UTF-8 text with no NUL byte and
    no line break; at most 64 names; a value becomes one environment variable of
    an agent panel only, so a shell, an ssh, an editor, a browser and a device
    panel get none; the argv of every ssh is Horizon's own text; a saved session
    holds the kind, command and arguments of a panel and no value; a refusal
    names the secret and never quotes it; a computer reads and writes the store
    on a worker thread and publishes the values of one read.
  - Does not guarantee: that an agent cannot read and send the value, which is
    the purpose of the feature; that a second process of the same user cannot
    read the environment of a panel; that a running panel takes a new value; that
    a computer without a credential store holds any value.
- [`server-link`](trust-boundaries/server-link.md)
  - Untrusted: every byte of one link, the device name of its hello, the intents
    and the records that a computer sends, the id and the body of a secret, the
    fields of a body of an internal object, and the machine, binding and secret
    rows that a server sends back to a computer.
  - Guarantees: one writer holds the SQLite file and each push is one transaction
    with no network call inside it, the answer leaves after the commit, and a
    reader uses its own connection; each decided intent takes the next log index;
    the identifier of an intent makes a resend answer its first result; the first
    intent on a version of a secret wins and a later one is refused with both
    numbers; a machine record passes `horizon_core::machines::check_record` at the
    server; a secret id passes `SecretId::parse` at the server and at the computer,
    so it is one machine key or one credential reference and never a path; every
    body of the table `secrets` is sealed with XChaCha20-Poly1305, a random nonce
    per write and the id as associated data, with a 32-byte key in
    `<server home>/secret.key` at `0600` that is never in the database and never
    sent; a live secret that the key cannot open is no row of a pull and the hello
    says `secrets_lost`; a `binding` row passes the machine-name rule and the
    key-name rule; a frame of at most 256 KiB, 256 intents and 8 secrets in one
    push, 256 secrets, 16 KiB of one body, 64 machines, 64 probe objects and 32
    open waits for one link; the server stamps every change with its own clock; the
    server runs no path and no command of a computer, opens no network connection
    and reads no secret body; a link that the server ends stops writing first, so
    the computer reads the typed error; the socket, the queue file and the key file
    are `0600` in a `0700` directory; the computer checks every machine row of a
    pull again before it reaches its mirror, writes a secret only where a value of
    that kind lives, keeps a local value that lost as
    `<name>.conflict-<time>`, holds the id of a secret and not its body in its
    queue, and holds no secret in its mirror; the two modes of the binary run
    before the Horizon home is prepared, so they read no setting, open no window
    and install no agent integration; a second server of one home refuses at its
    lock; a stop ends the links that are open.
  - Does not guarantee: who the computer is, because the device name is a claim
    that the transport does not prove; availability, because a link holds a thread
    and another process of the same user can open the file; the content of a probe
    body, which is bounded JSON that the server does not read; the correctness of
    the clock of the server; that a person or a process which can log in to the
    server account cannot read every machine key and every credential value, since
    the key file is beside the database under that same login; that a lost computer
    is harmless, since every linked computer holds a copy of every secret; which of
    two values of one secret is the correct one when two computers changed it.

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
