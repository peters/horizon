---
procedure: local-network-bridge-agent-access
feature: Local Network Bridge, agent access on the worker
platforms: [linux, macos]
cost: rents compute
destructive: yes      # cleanup deletes the cloud that this run made
secrets: none
owner: peters
---

# Local Network Bridge agent access test procedure

## 1. Purpose

This procedure makes sure that the agent user on a worker can use the Local
Network Bridge while the owner shares the local network. The agent user must get
the status, discover, probe, forward and unforward. The agent user must not get
the private control socket of the helper, and must not stop the bridge.

## 2. Applicability

- Candidate: each candidate that changes `horizon-cloud-worker local-network`,
  its sockets or the worker image.
- Platforms: a Linux or macOS PC on a home or office IPv4 network. The worker
  runs the image from `examples/cloud-worker`.
- This procedure does not test: the scope editor, network changes, sleep, or the
  **Open connections** list. Other procedures and plans test these items.

## 3. Safety

> **CAUTION:** SHARE ONLY A NETWORK THAT YOU CONTROL. The bridge exposes the
> local network to the worker. Each process on the worker can then reach the
> devices on that network.

> **CAUTION:** SWITCH OFF **Share local network** AT THE END OF THE RUN. If you
> do not, the worker continues to reach the local network.

> **CAUTION:** DELETE THE CLOUD THAT THIS RUN MADE. A Ready cloud rents compute
> until you delete it.

## 4. Equipment and preconditions

- A frozen candidate of Horizon on the PC, with its commit and SHA-256.
- An isolated desktop for the candidate, with a live view in a Device panel.
- A Ready cloud. The worker image contains the `horizon-cloud-worker` of the
  candidate commit. A Hetzner `cx23` profile with only an image is enough.
- A router or other device on the local network with a web page on TCP port 80,
  plain HTTP. In this procedure, `<router>` is its IPv4 address, for example
  `192.168.1.1`.
- A root shell on the worker through the SSH connection of the cloud.
- No model key on the worker. The tasks use the command line of the helper.

## 5. Setup

1. On the PC, record the IPv4 subnet of the default route. In this procedure,
   `<subnet>` is this subnet, for example `192.168.1.0/24`.

   Result: You know `<subnet>` and `<router>`, and `<router>` is in `<subnet>`.

2. Record an IPv4 address that is not in `<subnet>`, for example `203.0.113.1`.
   In this procedure, `<outside>` is this address.

   Result: `<outside>` is not in `<subnet>`.

3. In the root shell on the worker, define a short command for the agent user.

   ```sh
   agent() { runuser -u horizon-agent -- env HOME=/workspace/home "$@"; }
   ```

   Result: The shell accepts the function.

4. Examine the user that the function uses.

   ```sh
   agent id -u
   ```

   Result: The output is `10001`.

5. Make sure that **Share local network** is off on the card of the cloud.

   Result: The card shows the switch in the off position.

## 6. Tasks

### 6.1 N01 — Bridge off, agent status

1. Get the status as the agent user.

   ```sh
   agent horizon-cloud-worker local-network status
   ```

   Result: The output shows `"active": false`. The note says that only the owner
   can turn the bridge on.

2. Get the status as root.

   ```sh
   horizon-cloud-worker local-network status
   ```

   Result: The output shows `"active": false`.

### 6.2 N02 — Owner switches on, agent status shows active

> **CAUTION:** SHARE ONLY A NETWORK THAT YOU CONTROL. The next step exposes the
> local network to the worker.

1. On the card of the cloud, switch on **Share local network**.

   Result: The card shows **Sharing `<subnet>`** in 10 seconds or less.

2. Get the status as the agent user.

   ```sh
   agent horizon-cloud-worker local-network status
   ```

   Result: The output shows `"active": true`, `"subnet"` with `<subnet>` and
   `"proxy"` with `127.0.0.1:<port>`. The `discovery` object shows
   `"available": true`.

3. Get the status as root.

   ```sh
   horizon-cloud-worker local-network status
   ```

   Result: The output shows the same `subnet` and `proxy` as in step 2.

4. Record `<port>` from the `proxy` value.

   Result: You know the SOCKS5 port of the bridge.

### 6.3 N03 — Socket permissions

1. Examine the sockets and the private directory.

   ```sh
   stat -c '%A %U %n' /run/horizon-local-network.sock \
     /run/horizon-local-network /run/horizon-local-network/control.sock
   ```

   Result: The output shows these three lines:

   ```text
   srw-rw-rw- root /run/horizon-local-network.sock
   drwx------ root /run/horizon-local-network
   srw------- root /run/horizon-local-network/control.sock
   ```

2. List the private directory as the agent user.

   ```sh
   agent ls /run/horizon-local-network
   ```

   Result: The command fails with `Permission denied`.

### 6.4 N04 — Agent discovers devices

1. Discover the devices as the agent user.

   ```sh
   agent horizon-cloud-worker local-network discover
   ```

   Result: The output shows a `devices` list. Each device address is in
   `<subnet>`. The output does not show `Local Network Bridge is off`.

### 6.5 N05 — Agent probes a router port

1. Probe port 80 of the router as the agent user.

   ```sh
   agent horizon-cloud-worker local-network probe <router> 80
   ```

   Result: The output shows `<router>` and port `80` in `open`.

2. Probe the address `<outside>` as the agent user.

   ```sh
   agent horizon-cloud-worker local-network probe <outside> 80
   ```

   Result: The command fails. The message says that the address is outside the
   bridged local network.

### 6.6 N06 — Agent uses the proxy

1. Get the router page through the proxy as the agent user.

   ```sh
   agent curl -sS -o /dev/null -w '%{http_code}\n' \
     -x socks5h://127.0.0.1:<port> http://<router>/
   ```

   Result: The output is an HTTP status, for example `200` or `401`.

### 6.7 N07 — Agent forwards and unforwards

1. Forward port 80 of the router as the agent user.

   ```sh
   agent horizon-cloud-worker local-network forward <router> 80
   ```

   Result: The output shows `worker_port`, `host` with `<router>` and `port`
   with `80`. In this procedure, `<forward>` is the `worker_port` value.

2. Get the router page through the forward as the agent user. Send the same
   `Host` header as in N06.

   ```sh
   agent curl -sS -o /dev/null -w '%{http_code}\n' \
     -H 'Host: <router>' http://127.0.0.1:<forward>/
   ```

   Result: The output is the same HTTP status as in N06.

3. Get the status as the agent user.

   ```sh
   agent horizon-cloud-worker local-network status
   ```

   Result: The `forwards` list contains the forward from step 1.

4. Remove the forward as the agent user.

   ```sh
   agent horizon-cloud-worker local-network unforward <forward>
   ```

   Result: The output shows `"active": true` and an empty `forwards` list.

5. Get the router page through the old forward port.

   ```sh
   agent curl -sS -m 5 http://127.0.0.1:<forward>/
   ```

   Result: The command fails. The connection is refused.

### 6.8 N08 — Agent cannot use the private control

1. Connect to the private control socket as the agent user.

   ```sh
   agent python3 -c 'import socket; s = socket.socket(socket.AF_UNIX); s.connect("/run/horizon-local-network/control.sock")'
   ```

   Result: The command fails with `PermissionError`.

2. Send a retire request to the agent socket as the agent user.

   ```sh
   agent python3 -c 'import socket; s = socket.socket(socket.AF_UNIX); s.connect("/run/horizon-local-network.sock"); s.sendall(b"{\"operation\":\"retire\"}\n"); print(s.recv(4096).decode())'
   ```

   Result: The output is `{"error":"Only a bridge helper can ask for that"}`.

3. Get the status as the agent user.

   ```sh
   agent horizon-cloud-worker local-network status
   ```

   Result: The output shows `"active": true` and the same `proxy` as in N02. The
   card on the PC still shows **Sharing `<subnet>`**.

### 6.9 N09 — Owner switches off, agent status shows off

1. Forward port 80 of the router again as the agent user.

   ```sh
   agent horizon-cloud-worker local-network forward <router> 80
   ```

   Result: The output shows a `worker_port`. In this procedure, `<new forward>`
   is this value.

2. On the card of the cloud, switch off **Share local network**.

   Result: The card does not show **Sharing `<subnet>`**.

3. Get the status as the agent user.

   ```sh
   agent horizon-cloud-worker local-network status
   ```

   Result: The output shows `"active": false`. The note says that only the owner
   can turn the bridge on.

4. Examine the agent socket.

   ```sh
   ls -l /run/horizon-local-network.sock
   ```

   Result: The command fails. The socket does not exist.

5. Get the router page through the forward port of step 1.

   ```sh
   agent curl -sS -m 5 http://127.0.0.1:<new forward>/
   ```

   Result: The command fails. The connection is refused.

6. Forward port 80 of the router as the agent user.

   ```sh
   agent horizon-cloud-worker local-network forward <router> 80
   ```

   Result: The command fails. The message says that the bridge is off.

### 6.10 N10 — Owner switches on again

> **CAUTION:** SHARE ONLY A NETWORK THAT YOU CONTROL. The next step exposes the
> local network to the worker again.

1. On the card of the cloud, switch on **Share local network**.

   Result: The card shows **Sharing `<subnet>`**.

2. Get the status as the agent user.

   ```sh
   agent horizon-cloud-worker local-network status
   ```

   Result: The output shows `"active": true` and a `proxy` value. The `forwards`
   list is empty.

3. On the card of the cloud, switch off **Share local network**.

   Result: The card shows the switch in the off position.

## 7. Pass criteria

- In N01 and N09, the agent user gets `"active": false` while the bridge is off.
- In N02 and N10, the agent user gets `"active": true`, the subnet and the proxy
  while the bridge is on. The values are the same as the values for root.
- In N03, only the agent socket is open to each user. The private directory and
  the private control socket are open only to root.
- In N04, N05, N06 and N07, each operation of the agent user succeeds inside
  `<subnet>`. The probe outside `<subnet>` fails.
- In N08, the agent user cannot connect to the private control socket. The
  retire request fails, and the bridge stays on.
- In N09, the switch on the PC stops the bridge, the agent socket and the
  forwards.

## 8. Cleanup

1. Make sure that **Share local network** is off on the card of the cloud.

   Result: The card shows the switch in the off position.

2. Remove the `agent` function from the root shell.

   ```sh
   unset -f agent
   ```

   Result: The shell does not know the `agent` command.

> **CAUTION:** DELETE ONLY THE CLOUD THAT THIS RUN MADE. If you delete other
> clouds, other people lose their work.

3. If this run made the cloud, delete the cloud.

   Result: The provider does not show the worker of this run.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository. Public results use
only example addresses, for example `192.168.1.1` and `192.168.1.0/24`.
