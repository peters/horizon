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

- 35 boundary pages, 1,688 lines. The issue counts 34. The tree held 35 files
  on 2026-10-24 09:57, when the issue opened, so the audit covers all 35.
- Each `applyTo` entry matches at least one file.
- No claim contradicts the code at its matched paths.
- One routing defect was found and is corrected: `tailnet-node-cleanup` had the
  brace list `production/{cards*,close*,lifecycle.rs}`. A plain glob reader does
  not match a brace list, so the boundary did not route to those UI files. The
  six plain globs `cards.rs`, `cards/**`, `close.rs`, `close/**`,
  `lifecycle.rs` and `lifecycle/**` replace it.
- No page uses a design page or a section that is absent.
- No page names a limit, a format version or a contract marker that differs from
  the code. Examples: the marker `horizon-tailnet-contract=4` in
  `worker_contract.rs`, deployment format 7 with paired allocation format 5 in
  `state/exposure/mod.rs`, deployment format 5 with paired allocation format 3
  for a publication record, exposure ports 600–663 in `exposure/ingress.rs`, and
  the limits 20 KiB and 144 KiB of a control frame.
- Three names in the text are defined just outside the `applyTo` paths of their
  boundary. No claim changes, and each caller is inside the paths:
  - `set_files`, an attachment action in
    `crates/horizon-browser-protocol/src/control.rs`.
  - `app_tunnel_unconfigured`, the message of `Error::TunnelUnconfigured` in
    `crates/horizon-app-host/src/lib.rs`, which `bootstrap.rs` raises.
  - `HostIo`, in `crates/horizon-ui/src/app/browser_host_io.rs`.
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
guarantee that the code gives, and what the code does not guarantee.

### Local host and private files

- [`browser-profile-host`](trust-boundaries/browser-profile-host.md)
  - Untrusted: the create, close and visibility requests of each connect
    client, the pages that a client opens, and a host file that the group or
    other users can write.
  - Guarantees: the host claims only the requests of its own host instance; one
    actor holds at most 4 pages and the host runs at most 16; a screenshot goes
    only to the actor that holds the page; the host refuses a host file of more
    than 256 KiB, and a host file that the group or other users can write.
  - Does not guarantee: the pages of one browser profile share its cookies and
    its storage; a process of the same operating-system user can read the
    runtime root and write a request as any actor; the host instance in
    `profile-host.json` is not a secret.
- [`browser-remote-bridge`](trust-boundaries/browser-remote-bridge.md) (added
  after this audit, with #1780)
  - Untrusted: the remote host and its record, the results of that host, and the
    files of this computer that an attachment call names.
  - Guarantees: the settings entry refuses a target that `ssh` reads as an
    option, a space and a control character, and a profile off the host-file
    rule; only an authorized path is staged, with a 512 MiB limit for one file
    and for one call; the bridge removes the staging folder of a call after its
    answer and after a failure; the host instance always comes from the record
    of the remote machine; a `browser_create` that names no target gets the
    profile of the entry, and a create that names one keeps it; a capture
    crosses inline only up to 8 MiB; the bridge starts no listener and holds no
    credential.
  - Does not guarantee: a staged file is readable on the remote machine for the
    duration of the call; a process of the operating-system user on that machine
    can read the staged copies and the runtime root; the bridge does not check
    the profile of its entry against the host file, so an unknown profile is
    reported by the host as `target_unknown`.
- [`browser-remote-panel`](trust-boundaries/browser-remote-panel.md)
  - Untrusted: the record and the results of the remote browser host, and the
    input of the person that a panel sends there.
  - Guarantees: the actor of every page of a panel is the constant
    `horizon:person:browser-panel`; the host instance always comes from the
    record of the remote machine; a frame is a bounded inline PNG and the panel
    reads no remote path; a pointer position, a local dropped file, a video
    capture and a handoff are refused with a typed reason and never sent as page
    script; one line of the channel is bounded to 24 MiB.
  - Does not guarantee: the host decides which profile and which page an actor
    sees; one profile of the host shares its cookies with every page of that
    profile; a compromised remote machine can send any PNG as a frame; a process
    of the same operating-system user on the remote machine can write a request
    as any actor.
- [`standalone-remote-host`](trust-boundaries/standalone-remote-host.md)
  - Untrusted: the MCP client and its tool calls.
  - Guarantees: the client cannot supply an endpoint, a capability or a
    credential; the host claims only the requests of its own agent identity and
    host instance; a credential file that the group or other users can read or
    write is refused; no answer holds a credential value or a credential path.
  - Does not guarantee: another process of the same operating-system user can
    read the credential files and the runtime root.
- [`native-client-file`](trust-boundaries/native-client-file.md)
  - Untrusted: the MCP client.
  - Guarantees: each credential file opens with no link in its path, is a
    regular file of the effective user, has no group or other permission bit and
    is at most 4 KiB; a missing or an extra reference, a relative path and a
    provider with `credential_bindings` in the configuration refuse before a
    tool; the host never opens the OS keyring; no answer holds a value or a
    path.
  - Does not guarantee: a process of the same operating-system user can read the
    credential files.
- [`checkout-search`](trust-boundaries/checkout-search.md)
  - Untrusted: each home-folder entry and each Git metadata file, which can be a
    link, a loop, a pipe, a device, huge, or change while it is read.
  - Guarantees: the search starts no process, stays on its background thread,
    and applies its visit, depth, checkout and file-size bounds to the bytes
    that it reads; on macOS it opens nothing inside a guarded home folder.
  - Does not guarantee: one `read_dir`, `metadata` or open on a mount that hangs
    can block that background thread; a file swapped for a pipe between its
    check and its open can still be opened; a checkout can claim any origin.
- [`docker-host-provider`](trust-boundaries/docker-host-provider.md)
  - Untrusted: the output of the docker CLI, the containers and volumes on a
    host, and a host that is removed or saved again under the same name.
  - Guarantees: each argument of a remote docker command is one POSIX shell
    word; host names, SSH addresses and docker paths are examined before a
    command is built; the remote CLI talks to the Unix socket of the host; a
    worker runs only on a Docker Engine 28 or later and publishes its SSH on the
    loopback; the engine ID is recorded and compared at each lifecycle step.
  - Does not guarantee: the docker limits are the worker limits, and a volume
    size is not enforced; the capacity check is a reading that can be 30 seconds
    old; a person or a program with Docker access on the host can read, change
    or remove the worker.

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
- [`cloud-browser-uploads`](trust-boundaries/cloud-browser-uploads.md)
  - Untrusted: the page and its `accept` and `origin` text, the agents of the
    worker, and the control service toward the host.
  - Guarantees: a chooser waits only while a host polls, and for at most 20
    minutes; a browser gets only complete files that the service staged for it,
    whose size and SHA-256 agree with the host; below the browser history folder
    the service makes the folders and the file through descriptors that never
    follow a symbolic link; a drop names the page generation, so a worker
    refuses a drop after a navigation.
  - Does not guarantee: the control service cannot tell a host from a process on
    the worker; the service, the browsers and the agents use one account, and
    without agent isolation that account is `root`; the removal of a batch uses
    its path; a used batch stays until its browser closes; a remote device of a
    provider gets no uploads.
- [`cloud-file-drop`](trust-boundaries/cloud-file-drop.md)
  - Untrusted: the worker and its agents.
  - Guarantees: a name goes to the worker only as one quoted shell argument of
    at most 255 bytes, with control characters replaced by `_`; the upload
    accepts only the host key that is recorded for that worker; the worker
    writes only new files, refuses a symbolic link and a folder of another user,
    and moves a file into the batch folder only when its size is the local size;
    the paths go only to the panel of the same session on the same worker.
  - Does not guarantee: the files are not private between the sessions of one
    worker; they stay until the cloud is deleted; a folder is not uploaded; a
    failed cleanup leaves an empty batch folder or partial files in the private
    folder, and Horizon pasted no path of them; an agent without agent isolation
    is `root`.

### Cloud records and durable evidence

- [`publication-records`](trust-boundaries/publication-records.md)
  - Untrusted: the saved publisher record, and an image repository or a profile
    name that differs from that record.
  - Guarantees: the record holds no credential; the retained identity is
    authenticated and compared before each new build and before the credentials
    load; an invalid record cannot replace the previous saved cloud; a
    publication record uses deployment format 5 and paired allocation format 3,
    which fences an older reader.
  - Does not guarantee: the record is not authentication and not proof of
    current registry permission; a reconnect or a resize that reuses an existing
    image starts no publication.
- [`exposure-records`](trust-boundaries/exposure-records.md)
  - Untrusted: saved exposure data and the fields that a reader decodes from it.
  - Guarantees: the outcome retains bounded comparison data through deployment
    format 7 and paired allocation format 5; an absent record keeps the earlier
    formats and encodings; the `cloud-exposure` feature stays off by default and
    starts no runtime operation.
  - Does not guarantee: a model value, a path, a UUID or an SSH argument makes
    no live client, approval or transport; provider progress authorizes no
    effect and claims no provider absence; current operation uniqueness is not a
    complete historical replay ledger.
- [`one-device-admission-records`](trust-boundaries/one-device-admission-records.md)
  - Untrusted: all saved JSON, which is a fact and never authority.
  - Guarantees: the outcome stores metadata only; the fields and the decoder
    stay private; scope construction examines the shape only; deployment format
    6 and allocation format 4 apply only to nonempty admission evidence, and
    missing evidence keeps the earlier versions.
  - Does not guarantee: the record adds no owner decision, runtime constructor,
    gateway factory, worker command or public sharing operation; a decoded
    Closed record proves no physical cleanup and grants no fresh operation.
- [`tailnet-enrollment-receipts`](trust-boundaries/tailnet-enrollment-receipts.md)
  - Untrusted: the worker status output and the serialized ledger entries.
  - Guarantees: only typed local IDs and opaque bounded device IDs enter the
    ledger; a changed device ID never overwrites an earlier receipt; nonempty
    evidence fences an older reader with a new deployment and allocation
    version; the root reader bounds stdout before it parses JSON and suppresses
    raw status and errors.
  - Does not guarantee: the ledger is not proof of remote ownership; no
    credential, peer list, device name or address is deletion authority; the
    full deployment reader is not a bounded streaming reader.
- [`rebuild-uncommitted`](trust-boundaries/rebuild-uncommitted.md)
  - Untrusted: the working copy of the repository, its Git configuration, and
    the file names and links in the build context.
  - Guarantees: `collect` lists each uncommitted file of the build context and
    `Uncommitted::hash` covers each path, change and digest; `overlay` collects
    the files again and refuses another hash; a rebuild starts only with a
    confirmed plan, and a file that is added later is refused; a link, a hard
    link, a `..` part, another `core.worktree` and a common secret name are
    refused; git2 reads the working copy, so no configured program runs in the
    process of Horizon.
  - Does not guarantee: the contents of a file are not examined, so a secret in
    an ordinary file goes into the image after a person confirms it;
    `.dockerignore` is applied by Docker and not here, so the list can name a
    file that the image does not get.
- [`cloud-removal`](trust-boundaries/cloud-removal.md)
  - Untrusted: removal records, journals, filesystem timing and display
    snapshots.
  - Guarantees: each removal eligibility operation of the filesystem and of the
    Store stays on the worker; Core `can_remove` stays authoritative; a fresh
    final check keeps its Store on the worker through the exact UI application,
    and the UI owns only a release sender; a cancel keeps the job slot of the
    record until its channel disconnects; a dismissal clears the
    automatic-close intent and restores the hidden panels.
  - Does not guarantee: a display snapshot cannot authorize a discard; Pending
    is not a failed removal; a cancellation cannot interrupt blocking OS I/O;
    the UI change adds no provider action and no MCP grant.

### Tailnet identity

- [`tailnet-api-credentials`](trust-boundaries/tailnet-api-credentials.md)
  - Untrusted: API credential drafts and stored bytes, a workspace actor, a
    tool argument and a pending request.
  - Guarantees: an API credential stays on the desktop and is independent of an
    enrollment auth key; a secret reaches no debug output, public metadata,
    worker payload, project configuration, command argument or log; the OS
    store is the secret authority and the JSON is nonsecret binding metadata; a
    save or a removal starts only with an exact native owner decision; status
    rendering does no filesystem, keychain or network work.
  - Does not guarantee: a save does not call Tailscale and examines no remote
    scope, expiry or tag; stored presence proves no validity; API credential
    management captures no enrollment ID, removes no device and grants no
    remote cleanup authority; a tag selection is not proof of a provider-side
    restriction.
- [`tailnet-enrollment`](trust-boundaries/tailnet-enrollment.md)
  - Untrusted: workspace code and agents toward the private key and the node
    state, and the status that a worker reports.
  - Guarantees: enrollment advertises `tag:horizon-worker` and examines
    `Self.Tags` after `up`; a selected tailnet refuses an image without
    `horizon-tailnet-contract=4`; an auth key reaches no command argument, log,
    agent environment or retained file; an agent cannot read the private node
    state or use the administrative socket.
  - Does not guarantee: there is no continuous tag monitoring, watcher or
    quarantine; there is no tag for each workspace and no automatic tailnet
    policy edit; no page promises that proxy listeners are absent before the tag
    check; a failed logout makes no administrative node deletion and no
    persistent revocation marker; the contract declaration of an image does not
    prove that the image is honest.
- [`tailnet-node-api`](trust-boundaries/tailnet-node-api.md)
  - Untrusted: the provider HTTP replies.
  - Guarantees: a fixed HTTPS origin, no proxy and no redirect, bounded headers,
    body and time, one encoded opaque device segment, an OAuth scope and tag
    subset, and value-free typed errors; a missing device, an offline node, a
    denied request and a 404 stay Retained.
  - Does not guarantee: the SDK alone supplies no owner authentication, no
    durable confirmation and no automatic cloud deletion; offline fixtures
    establish no real API authority; a production origin override is test-only.
- [`tailnet-node-cleanup`](trust-boundaries/tailnet-node-cleanup.md)
  - Untrusted: caller paths, names, peers, tags, an SDK string parameter and the
    provider journals.
  - Guarantees: the owner controller and the captured root daemon supply the
    authority; the original locked cloud, allocation and provider binding and
    the settled provider deletion evidence are required before an API request;
    a group is confirmed only after a 200 or 204 reply and is saved before the
    next request; a missing, a foreign or an unsettled journal refuses the
    dispatch; all 404 stays uncertain.
  - Does not guarantee: text matching preserves the UI detail only and never an
    authority; a synthetic test cannot claim a native, provider or API
    qualification; an agent and a companion MCP get no provider-delete
    authority.

### Exposure policy and control

- [`exposure-policy`](trust-boundaries/exposure-policy.md)
  - Untrusted: policy JSON and receipt JSON from an agent, a provider or a saved
    file.
  - Guarantees: the component is a default-off pure comparison contract with no
    public dispatch; unknown and duplicate fields are refused, and a policy JSON
    of more than 16 KiB is refused; a CIDR, a DNS name and a tag are examined; a
    management port, a reserved ingress port and a physical port in the range
    600–663 never become a target; an HTTPS route accepts only the ports 443,
    8443 and 10000.
  - Does not guarantee: a named-rule comparison, a saved URL and a tailnet-range
    IP become no authenticated membership, owner approval or runtime authority;
    the component activates no worker, provider or public owner runtime; a
    receipt claim is not a publication and not a readiness proof.
- [`exposure-control`](trust-boundaries/exposure-control.md)
  - Untrusted: the raw control frames.
  - Guarantees: a request frame is at most 20 KiB and a reply frame at most 144
    KiB; an unknown or duplicate field, trailing JSON and an invalid or non-nil
    operation identity are refused; the contract implements no keeper,
    transport, membership issuer or approval.
  - Does not guarantee: a direct serde decode does not enforce the frame-size
    gate and not the full semantic validation; a later stream reader must bound
    the bytes during the input and keep the original live authority separate;
    the contract adds no public activation, root process or provider effect.

### Worker ingress and provider absence

- [`worker-exposure-gate`](trust-boundaries/worker-exposure-gate.md)
  - Untrusted: the peer address, the caller identity, a PROXY header, a loopback
    tuple and stored metadata.
  - Guarantees: the off state creates no listener; an empty Restricted policy
    and an unsupported or malformed rule fail before the bind; a CIDR uses the
    `peer_addr` of the same accepted stream; the gate uses exclusive fresh
    listeners in a fixed pool, with separate IPv4 and IPv6, and the targets are
    literal loopback addresses with explicit nonmanagement ports.
  - Does not guarantee: public access needs an explicit choice; DNS, tag and
    `TailnetOnly` dispatch stay unsupported until a trusted producer is
    qualified against the actual daemon; raw TCP adds no TLS and no
    authentication; a counter and a decoded policy authorize no provider
    release, owner approval or retry.
- [`trusted-exposure-gate`](trust-boundaries/trusted-exposure-gate.md)
  - Untrusted: an accepted stream and each peer fact that is not original.
  - Guarantees: the adapter consumes the original `Producer` and the opaque
    `AcceptedPeer`; `Producer::accept` alone issues a `PendingStream`; the
    private original-accept cookie is compared through
    `AcceptedStream::matches_pending`; the weak handle cancels the payload and
    the installed control; the byte charges, the pending limits and the grace
    remain as they were.
  - Does not guarantee: copied DNS names, tags, a PROXY header and a loopback
    tuple alone authorize no peer; a direct P admission is the actual socket
    CIDR only; a custody refusal and a missing connection cannot satisfy a named
    denial; no owned Serve, Funnel, keeper or public path belongs to this cut.
- [`trusted-tcp-producer`](trust-boundaries/trusted-tcp-producer.md)
  - Untrusted: a caller-selected path, a decoded identity, a PROXY header, the
    accepted stream and the SCM_RIGHTS control.
  - Guarantees: the original UNIX credentials, the pidfd, the executable, the
    process start and the namespace are examined; the exact private per-bind
    origin identifies a pending stream; one control FD with the exact family,
    type and unnamed addresses is admitted; a DRAINED byte with EOF settles a
    flow, and the ancillary allocation has a bound.
  - Does not guarantee: a fixed runtime endpoint and a root check do not prove a
    protected host publication, an owner approval or a workspace membership;
    same-daemon descriptor ownership does not prove Serve ingress; a component
    test does not replace the protected-runtime or M2 qualification.
- [`tailscale-unix-proxy`](trust-boundaries/tailscale-unix-proxy.md)
  - Untrusted: the Serve requests of a workspace and the flow-cancellation
    control.
  - Guarantees: the patch is reviewed as production source, with the archive
    and compiler pins, the module checksums and the Go-only build stages; the
    cancellation is Serve-only, on Linux raw TCP, absolute Unix, PROXY1, with a
    nonzero virtual port and no TLS, HTTP or Funnel; one control DRAINED byte
    follows two successful copies and both CloseWrite operations.
  - Does not guarantee: an ACK proves the daemon copy and the frontend
    half-close, not the delivery to the remote application; equal markers are
    not binary reproducibility; the runtime grants no source authority and no
    M2 activation; an old TCP pass does not qualify the new contract.
- [`exposure-firewalls`](trust-boundaries/exposure-firewalls.md)
  - Untrusted: the provider comparison data and the provider replies.
  - Guarantees: an existing mutation examines the exact ID, the full ownership
    labels and no attachment or the original numeric server; a create is
    unattached and is one POST; an ID precedes an action wait; a selector and a
    foreign target are refused.
  - Does not guarantee: a successful action is not proof of the current state
    and not proof that the physical gate is closed; the SDK implements no owner
    approval and no atomic preflight with the mutation; a lower-level operation
    alone preserves no policy.
- [`exposure-management`](trust-boundaries/exposure-management.md)
  - Untrusted: journals, decoded DTO facts, provider lookups and attachment
    snapshots.
  - Guarantees: the original client is consumed and is never reconstructed from
    a journal or a DTO fact; the exact original allocation, numeric server, SSH
    endpoint and settled attachment snapshots are examined; a selector, an
    unknown enabled attachment field, a duplicate ID and a foreign drift are
    refused; each sibling contribution and each foreign rule is preserved; an
    intent precedes each mutation and a returned ID precedes a wait.
  - Does not guarantee: `ManagementRules` examines the syntax only, so the real
    owner must cover the pinned SSH policy; a failed current verification
    latches a cleanup-only uncertainty; this outcome adds no activation, no
    credential factory and no restored controller.
- [`hetzner-observed-absence`](trust-boundaries/hetzner-observed-absence.md)
  - Untrusted: the provider replies.
  - Guarantees: an observation is produced only for an initially `Bound`,
    canonical nonzero server ID, which the same immutable client reads; the
    exact ID, operation name and ownership label are examined; the observation
    is opaque, cannot be serialized and has no public constructor; an error, a
    cancellation and a failed save return no observation.
  - Does not guarantee: an initial HTTP 404 is not an inherited identity;
    `Prepared`, a resolved `Requested`, `Terminated`, the 404 of another client
    and a failed reply stay outside the contract; a saved path makes no durable
    authority.
- [`runpod-exposure-ports`](trust-boundaries/runpod-exposure-ports.md)
  - Untrusted: the runtime data of the provider.
  - Guarantees: the decoder is default-off; it requires a RUNNING Pod, nonzero
    TCP port pairs and the numeric SSH public address.
  - Does not guarantee: a requested port is not a live mapping; ambiguous
    runtime data cannot replace the original SSH endpoint and cannot supply an
    exposure endpoint; the component changes no provisioning and qualifies no
    public readiness.

### Protected ingress and the local network bridge

- [`protected-host-ingress`](trust-boundaries/protected-host-ingress.md)
  - Untrusted: the profile intent, the receipt JSON, the host inspection output
    and the provider sets.
  - Guarantees: an absent or false intent changes no host configuration and
    upgrades no publication; the SDK fabricates no Worker mapping and no
    readiness; the host inspection reads the actual Docker publication; the
    original CID, image, PID and start, pidfd, namespace, sysctl, effective root
    capability and generation are examined; a cancellation and a failed
    readback keep an exact original cleanup.
  - Does not guarantee: a receipt cannot be overwritten, cannot refresh a
    generation, cannot rehydrate an authority and implies no source preservation
    without an external qualification; a successor is never trusted; the code
    claims no absence after a failed daemon I/O.
- [`protected-ingress-reader`](trust-boundaries/protected-ingress-reader.md)
  - Untrusted: the receipt file, the request JSON and the environment.
  - Guarantees: the default-off reader keeps the original local custody; it uses
    the canonical protocol validation; a symbolic link, a writable mount, a
    non-root file, a broad mode, an extra link and an oversized read are
    refused; the held and the freshly opened metadata, timestamps, content and
    fixed-path identity are examined again.
  - Does not guarantee: a path, a request JSON, an environment fallback and a
    caller allocation mint no live reader; a receipt publication is not source
    preservation, named admission or owner approval; the desktop cannot qualify
    the actual root readback, which needs the original task-owned VM.
- [`one-service-gateway`](trust-boundaries/one-service-gateway.md)
  - Untrusted: the endpoint and SOCKS input, the DNS answers, the routes and the
    client replies.
  - Guarantees: malformed input is refused before the lookup; the engine admits
    only the exact captured endpoint and connects to its examined literal
    address without a second resolution; it examines the complete canonical
    IPv4 set of a LAN answer before the policy filters; the complete binding is
    compared at the start, at the admission and at the source checks; a lost
    source closes the old relays and refuses a new admission.
  - Does not guarantee: the feature is a default-off prerequisite (M1-1) and not
    completed one-device sharing; a parsed endpoint, a path and a receipt grant
    no authority; a component test is not owner or worker qualification; a
    missing real counterexample is blocked and is not a pass.
- [`one-device-owner-admission`](trust-boundaries/one-device-owner-admission.md)
  - Untrusted: the endpoint, source and worker replies, and the serialized
    records.
  - Guarantees: `OwnerScope` examines the syntax only; only the trusted native
    caller supplies a verified original owner and an explicit endpoint
    approval; a validated Armed intent is saved before a listener or an SSH
    starts; the original OS Store lock is held once across the controller
    aliases; an exact owner, operation and generation govern the close; the
    physical source is revoked before the observers and the SSH wait.
  - Does not guarantee: no provider, API, OS-store, enrollment, policy,
    credential grant, public ingress or M1 expiry belongs to this outcome; the
    component is not a real owner, LAN, tailnet, SSH, native or public parity
    qualification; the Engine DNS and source checks and the default bridge
    behavior stay the same.

### Image publishing and the setup agent

- [`image-publishing`](trust-boundaries/image-publishing.md)
  - Untrusted: repository names, profile names, stored selections, source
    identities and the working directories of an agent.
  - Guarantees: only a host account binding authorizes a publication
    credential; the retained account or the exact publication grant is
    authenticated on each new build; the retained identity is compared before
    the credentials load; a registry destination holds no credential, tag or
    digest; a temporary Docker configuration does not read a stored personal
    chain; a receipt-backed operation needs the canonical Git root; the desktop
    destination refuses a cloud-owned panel.
  - Does not guarantee: the policy performs no authentication and no network
    operation and proves no registry permission; a local preference and a
    cached sign-in are not a confirmed publishing account; a listing returns no
    token and resolves no registry credential; a cancellation does not prove a
    rollback.
- [`repository-setup`](trust-boundaries/repository-setup.md)
  - Untrusted: each file that the setup agent writes, which includes `.horizon`,
    the status file, the run folder and the Git configuration, refs and hooks.
  - Guarantees: these files run no code in the process of Horizon and in its git
    commands, because `git_command` turns the hooks, the monitors and the
    replace refs off; Horizon reads an agent-written file with the readers in
    `untrusted.rs`, which follow no link, do not wait on a FIFO and stop at a size
    limit; each git and docker command has a time limit and an output limit;
    **Use this setup** commits only the setup that the proof examined, which
    Horizon reads again after the agent ended; the rented builder uses its own
    login key and host key, and a journal covers it from before the create
    request until its deletion is proven.
  - Does not guarantee: the proof shows that the setup works and not that the
    image is honest; the systemd scope is not a sandbox; the opt-in private
    NuGet restore and the private Rust git clones are off by default and keep
    their documented risk; a container gets no git folder and no credential of
    the person.
- [`claude-setup-token`](trust-boundaries/claude-setup-token.md)
  - Untrusted: the worker and its agent toward the setup token.
  - Guarantees: this computer and its private credential files are trusted; a
    worker is trusted with the token only while Claude is bound with it; the
    token reaches no `settings.json`, debug output, log, progress line, command
    argument, environment of a host process or fixture; a reconnect with
    another Claude sign-in removes `/workspace/credentials/claude-oauth-token`
    from the worker; the expiry is counted from the saved date.
  - Does not guarantee: Horizon does not know the real expiry and does not
    examine the token; an image layer's session environment must not set
    `CLAUDE_CODE_OAUTH_TOKEN`; the token reaches no worker whose profile does
    not permit Claude.
- [`cloud-create-request`](trust-boundaries/cloud-create-request.md) (added after
  this audit, with this change)
  - Untrusted: the `cloud_create` request of an agent panel and its repository
    path, the Git content of that repository, the provider's price and its
    GitHub sign-in prompt, and the saved cloud record.
  - Guarantees: only a live agent panel of this Horizon answers, in its own
    workspace; the machine's `agent_lifecycle` section decides every create, and
    a repository, a provider or a rate it does not allow is a refusal; a rate
    that cannot be quoted is a refusal; quick start alone is offered, and a
    commit with its own `.horizon/cloud.yml` is refused; the settings file is
    read on the operation's thread; the card is registered and the record that
    names the cloud is durable before any worker is rented; one operation owns
    one cloud, so the card's start and restore paths refuse while it runs; the
    outcome reaches the agent before the card's watch, so a ready cloud is done
    while that watch runs; a cancel never replaces a cancelled token; a GitHub
    sign-in prompt gets the non-UI skip only when the policy allows it.
  - Does not guarantee: a cancel cannot recall a provider request that left this
    computer; the policy is not read again on the deployment thread; the price
    is one quote and excludes storage and other charges; the operation table is
    in memory, so a restart answers `cloud_create_unknown_operation`; no worker
    choice, plan token, sibling, publishing selection or CLI plan entry is
    offered.
## Follow-up work

- Closed: `AGENTS.md` named `.github/` as the home of the review instructions,
  in the bullet that starts "Before the first review of work that reads
  untrusted input". That bullet now names `docs/review/` and [`README.md`](README.md),
  and the review section of `AGENTS.md` points to the same folder.
- `scripts/test_detect_ci_build_inputs.py` uses `.github/copilot-instructions.md`
  as a non-Rust fixture path. The fixture is synthetic and still passes. A later
  change can use another non-Rust path, as the issue says.
- The pointer `.github/instructions/rebuild-uncommitted.instructions.md` can go
  when the procedure `docs/testing/procedures/cloud-uncommitted-build-context.md`
  links to the new page.
- One review must succeed with the new paths. Only a pull request after this
  change can show that.
