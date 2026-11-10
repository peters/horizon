---
applyTo: "crates/horizon-core/src/browser/attached_host.rs,crates/horizon-core/src/browser/attached_host/**,crates/horizon-core/src/config/browser_host.rs,crates/horizon-browser-control/src/ssh_forward.rs,crates/horizon-browser-control/src/ssh_forward/**,crates/horizon-browser/src/process/attach.rs,crates/horizon-browser/src/process/attach/**,crates/horizon-browser/src/session/shared.rs"
---

# Trust boundary of the remote browser tunnel

A browser panel can drive the Chromium of another machine. Horizon reads the
profile of that machine over `ssh`, starts its Chromium when no browser answers,
forwards the loopback `DevTools` port of that browser to a loopback port of this
computer, and then drives it as a local browser. The design is in the module
comments of `crates/horizon-browser-control/src/ssh_forward.rs`,
`crates/horizon-core/src/browser/attached_host.rs` and
`crates/horizon-browser/src/process/attach.rs`. The procedure is
`docs/testing/procedures/remote-browser-tunnel.md`.

## Trusted: the settings entry and the SSH configuration

The settings entry `browser_host` names one SSH target and one profile. It is the
input of a person, and Horizon validates it twice: in
`BrowserHostConfig::validate` when the store is read, and in
`AttachedHost::new` when a panel starts.

- The SSH target is at most 255 characters, holds no whitespace and no control
  character, and does not start with `-`. A target that `ssh` would read as one
  of its own options is refused before any `ssh` process starts.
- The profile is one folder name: 1 to 40 characters of `a-z`, `0-9` and `-`,
  with a letter or a digit at each end. A panel without a profile uses `default`.
- Every value of the entry reaches the remote shell as one quoted word
  (`quote`), so a quote or a shell operator in a value can not leave that word.
  The remote command lines hold only those quoted values and the program that
  the snippet finds on that machine.
- The OpenSSH client of this computer carries the authentication and the host
  key. Horizon adds no trust of its own, holds no credential and reads no
  `~/.ssh` file.
- A panel that a person creates and a panel that an agent creates follow the
  entry. A saved panel restores on the host and the profile that its saved state
  names, and the restore validates both with the same rules.

## Untrusted: the host machine, its shell and its browser

The host machine answers every call, writes the endpoint file and serves the
`DevTools` protocol.

- The snippet that starts a browser removes the endpoint file first, creates the
  profile folder with `umask 077`, starts the browser detached with `nohup`, and
  binds its `DevTools` port to `127.0.0.1` of that machine. Its exit codes 3, 4
  and 5 become the typed reasons `attached_host_no_browser`,
  `attached_host_not_started` and the folder refusal.
- The remote port is read from the endpoint file as one plain number. The local
  side builds the endpoint again with the local port of the forward and refuses
  it unless the host of the URL is loopback (`host_port_from_ws_url`), so text of
  that machine can not point the control link at another address.
- The endpoint is read through a proxy-less loopback HTTP client with a 64 KiB
  bound, and only `/json/version` and its `webSocketDebuggerUrl` are used.
- A reason that `ssh` or the snippet gives reaches the panel unchanged. The
  panel shows that text and does not run it.
- The page, its frames and its results are untrusted as for a local browser, and
  the rules of `docs/review/trust-boundaries/browser-semantic-actions.md` and
  `docs/review/trust-boundaries/browser-downloads.md` apply to the page itself.

## What the tunnel guarantees

- One `ssh -N -L 127.0.0.1:<local>:127.0.0.1:<remote>` child for each panel,
  with `-T`, `-o BatchMode=yes`, `-o ExitOnForwardFailure=yes`,
  `-o ConnectTimeout=5`, `-o ServerAliveInterval=5` and
  `-o ServerAliveCountMax=2`. The local port is one that the kernel gives, and
  the forward listens on `127.0.0.1` only. A failed bind ends the child instead
  of leaving a panel that waits.
- The child is killed and reaped when the session ends, when the panel closes
  and when Horizon exits. The forward starts no listener of its own.
- No `ssh` call runs on the UI thread. A panel shows the state of its connect,
  and a host that drops packets never holds the caller.
- Every call has a deadline: 20 seconds for the endpoint read, 45 seconds for the
  start snippet, 20 seconds for the readiness of a forward, and 3 seconds for the
  reap of a killed child.
- The panel closes only its own page (`Target.closeTarget`) and never sends
  `Browser.close`, so a browser that Horizon did not start keeps running.
- The panel sets no download path (`Browser.setDownloadBehavior` is not sent) and
  reports `reporting: false`. A file upload or a file drop is refused with
  `remote_file_upload_unsupported`, so no file of this computer crosses the
  forward.
- A panel creates no local profile and deletes none.
- The automation disclosure of an attached session is
  `UnsupportedByBackend`, because the launch switches belong to the machine, not
  to Horizon.

## What the tunnel does not guarantee

- **Every local account of the host machine can drive that browser.** The
  `DevTools` port is bound to the loopback interface of that machine, but it
  asks for no credential. A process of any user of that machine can reach the
  port and use the cookies and the sign-ins of the profile. That is a stronger
  privilege than reading the profile folder. Use a machine and a profile that
  you trust for that exposure.
- **The browser keeps running after Horizon exits.** That is the reuse feature:
  a later panel and a restart of Horizon attach to the same browser. Its
  exposure stays until a person stops it.
- **The panels of one profile are not isolated.** They share the cookies, the
  storage and the page list of one Chromium, whether a person or an agent opened
  them.
- **The browser is not stopped, and no page of an earlier session is closed.** A
  connect that loses its link leaves its page behind, and a Retry opens a new
  page in the same browser.
- **The forward gives no confidentiality of its own.** The bytes cross the SSH
  transport that OpenSSH builds with the algorithms of the two machines. Horizon
  adds no layer.
- **The local forward port is reachable by every local process**, exactly as
  the `DevTools` port of a local Chromium is.
- **Horizon does not verify which program wrote the endpoint file.** The start
  snippet replaces a stale file, so a file of a browser that is gone does not
  answer for a new one. A program of that machine that writes the file of the
  profile before Horizon reads it can still name another browser.
- **Downloads and uploads stay on that machine.** A download reaches the disk of
  the host machine, and Horizon reports no file and no size. A file of this
  computer can not reach a page of that browser.
- **The host key is the trust of OpenSSH.** A host key that the configuration of
  the person accepts is accepted for every panel of the entry. Horizon adds no
  pin and shows no fingerprint of its own. The words of a refused host key reach
  the panel.

## The lifecycle that a review must keep

- One `ssh` child per panel, and none of them on the UI thread.
- Every `ssh` call is bounded and can not ask a question.
- The target and the profile are validated before the first `ssh` starts.
- The forward is loopback on both ends, and the endpoint is re-validated as a
  loopback URL after the local port replaces the remote one.
- No `Browser.close`, and no download path, and no file of this computer.
- The forward ends with the session, the panel and the process. The browser of
  the host does not.
