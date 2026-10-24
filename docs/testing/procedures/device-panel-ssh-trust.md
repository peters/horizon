---
procedure: device-panel-ssh-trust
feature: Device panel ssh route host-key trust
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Device panel ssh host-key trust test procedure

## 1. Purpose

This procedure tests the refusal of a Device panel that uses the `ssh` route to
reach a host that `known_hosts` does not trust. It tests the typed failure, the
fingerprint that a person compares, the trust step, the retry, and the changed
host key.

No operation in Horizon trusts a host. No operation accepts a key, a
fingerprint to accept or an ssh option. Only a person changes `known_hosts`.

## 2. Applicability

- Use a current Horizon candidate on Linux.
- Use an agent in the candidate's workspace that has the `device_panel` tool.
- Use an isolated desktop from the local device fixture for the VNC endpoint.
- This procedure does not test a remote cloud machine, a VNC password or video.

## 3. Safety

> **CAUTION: EDIT ONLY THE ENTRY FOR THE TEST HOST IN `~/.ssh/known_hosts`.**
> A wrong entry can make other hosts unreachable or can trust the wrong
> machine. Read the fingerprint before you add the key.

> **CAUTION: USE ONLY THE ISOLATED TEST DESKTOP AND A LOOPBACK SSH SERVER.**
> Do not open the ssh route to a production host for this test. The test
> server binds to `127.0.0.1` only. Do not remove `ListenAddress` from the
> server command.

## 4. Equipment and preconditions

- A frozen candidate `horizon` binary and its SHA-256.
- Docker, Xvfb, Openbox, x11vnc, bubblewrap and Python 3.
- An agent in the candidate's workspace that has the `device_panel` tool.
- A free loopback port for the SSH server, for example `2222`.
- No `known_hosts` entry for `[127.0.0.1]:2222`.

## 5. Setup

1. Start the local device fixture with a private state directory.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <frozen>/horizon \
     --native-view --state /tmp/horizon-device-ssh-trust
   ```

   Result: The fixture prints a manifest with `vnc_address`, for example
   `127.0.0.1:40000`.

2. Create a host key.

   ```sh
   mkdir -p /tmp/ssh-trust && ssh-keygen -q -t ed25519 -N '' \
     -C ssh-trust-test -f /tmp/ssh-trust/host_key
   ```

   Result: The command writes `/tmp/ssh-trust/host_key` and its `.pub` file.

3. Start an SSH server in a container on the host network. The server listens
   on the loopback address only.

   ```sh
   docker run -d --name ssh-trust --network host \
     -v /tmp/ssh-trust:/keys:ro alpine:3 sh -c 'apk add --no-cache openssh-server >/dev/null && \
       cp /keys/host_key /etc/ssh/ssh_host_ed25519_key && \
       chmod 600 /etc/ssh/ssh_host_ed25519_key && mkdir -p /run/sshd && \
       exec /usr/sbin/sshd -D -e -p 2222 -o ListenAddress=127.0.0.1'
   ```

   Result: `docker logs ssh-trust` shows `Server listening on 127.0.0.1 port 2222`.

4. Make sure that `known_hosts` has no entry for the test host.

   ```sh
   ssh-keygen -F '[127.0.0.1]:2222'
   ```

   Result: The command prints nothing.

5. Create a Device panel with the ssh route. Use the `vnc_address` from step 1
   as the endpoint.

   ```json
   {"operation":"create","endpoint":"127.0.0.1:40000","ssh":{"host":"127.0.0.1","port":2222}}
   ```

   Result: The tool returns a `panel_id` and `connection: "connecting"`.

## 6. Tasks

### 6.1 SSH-TRUST-01 — The refusal names the host and the key

1. Inspect the panel until `connection` is `disconnected`.

   Result: `connection_error` holds the ssh lines, and `connection_failure` is
   present.

2. Read the fields of `connection_failure`.

   Result: `code` is `host_not_trusted`. `ssh_host_key.host` is `127.0.0.1`.
   `ssh_host_key.port` is `2222`. `ssh_host_key.observed.key_type` is
   `ssh-ed25519`. `ssh_host_key.observed.sha256` starts with `SHA256:`.
   `action` says that a person compares the fingerprint and adds the key.

3. Look at the Device panel in the workspace.

   Result: The panel header shows `Disconnected`, and a red wrapped line below
   it holds the same host, key type and fingerprint. Open **Connection
   details** to see the same endpoint and ssh host.

### 6.2 SSH-TRUST-02 — The person compares the fingerprint

1. Read the host key from the machine, out of band.

   ```sh
   ssh-keygen -lf /tmp/ssh-trust/host_key.pub
   ```

   Result: The command prints a `SHA256:` fingerprint.

2. Compare that fingerprint with `ssh_host_key.observed.sha256` from task
   SSH-TRUST-01.

   Result: The two fingerprints are equal.

3. Compare the observation with a keyscan.

   ```sh
   ssh-keyscan -T 5 -p 2222 127.0.0.1 > /tmp/ssh-trust-keyscan.txt
   ssh-keygen -lf /tmp/ssh-trust-keyscan.txt
   ```

   Result: The fingerprints are equal. A keyscan alone does not identify the
   machine. Use the out-of-band key from step 1 to decide.

4. Inspect the panel again before the trust step.

   Result: `connection_failure` is still `host_not_trusted`. A retry does not
   change it, because no operation trusts a host.

### 6.3 SSH-TRUST-03 — The person trusts the host and the viewer connects

> **CAUTION: EDIT ONLY THE ENTRY FOR THE TEST HOST IN `~/.ssh/known_hosts`.**
> Use the fingerprint that you compared in task SSH-TRUST-02. Do not use a
> fingerprint from another source.

1. Add the key for the test host to `known_hosts`.

   ```sh
   printf '[127.0.0.1]:2222 %s\n' \
     "$(cut -d' ' -f1,2 /tmp/ssh-trust/host_key.pub)" >> ~/.ssh/known_hosts
   ssh-keygen -F '[127.0.0.1]:2222'
   ```

   Result: The second command prints the entry and its fingerprint.

2. Call `reconnect` for the owned panel and inspect it.

   Result: `connection` becomes `connected`, `connection_failure` is absent,
   and `frame_sequence` advances while the isolated desktop changes.

### 6.4 SSH-TRUST-04 — A changed host key is a verification failure

1. Stop the server and start it again with a different host key.

   ```sh
   docker rm -f ssh-trust
   ssh-keygen -q -t ed25519 -N '' -C ssh-trust-test-2 -f /tmp/ssh-trust/host_key_2
   docker run -d --name ssh-trust --network host \
     -v /tmp/ssh-trust:/keys:ro alpine:3 sh -c 'apk add --no-cache openssh-server >/dev/null && \
       cp /keys/host_key_2 /etc/ssh/ssh_host_ed25519_key && \
       chmod 600 /etc/ssh/ssh_host_ed25519_key && mkdir -p /run/sshd && \
       exec /usr/sbin/sshd -D -e -p 2222 -o ListenAddress=127.0.0.1'
   ```

   Result: The server presents a new key on `127.0.0.1:2222`.

2. Call `reconnect` for the owned panel and inspect it.

   Result: `connection` is `disconnected`, `connection_failure.code` is
   `host_key_changed`, and `ssh_host_key.observed` is absent. The action says
   that a changed key is a verification failure and that a person must check the
   machine out of band.

3. Read the Device panel.

   Result: The red line names the verification failure. It does not offer a way
   to accept the key.

## 7. Pass criteria

- The untrusted host returns `host_not_trusted` with the host, the port and the
  key type with its `SHA256` fingerprint.
- The fingerprint that Horizon reports is equal to the fingerprint that
  `ssh-keygen -lf` prints for the key of the machine.
- No operation trusts a host, and no operation accepts a key.
- The viewer connects after the person adds the key to `known_hosts`.
- A changed host key returns `host_key_changed` and keeps the tunnel refused.

## 8. Cleanup

1. Close the Device panel with `close`.

   Result: The tool reports the panel as closed.

2. Remove the test entry from `known_hosts`.

   ```sh
   ssh-keygen -R '[127.0.0.1]:2222'
   ```

   Result: The command removes the entry and writes a backup file.

3. Stop the fixture with Ctrl-C and stop the container.

   ```sh
   docker rm -f ssh-trust
   ```

   Result: No test process or container remains.

4. Delete `/tmp/ssh-trust`, `/tmp/ssh-trust-keyscan.txt` and the fixture state
   directory.

   Result: No test file remains.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
