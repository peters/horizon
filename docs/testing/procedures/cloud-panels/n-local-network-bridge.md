---
procedure: cloud-panels-n-local-network-bridge
feature: Cloud panels smoke test, area N (Local Network Bridge)
platforms: [linux]
cost: rents compute
destructive: no
secrets: none
owner: peters
---

# Cloud panels test procedure, area N: Local Network Bridge

## 1. Purpose

This area makes sure that the Local Network Bridge lets a worker reach a test
service on the local network of the PC. It also makes sure that the scope
limits the access and that the bridge is off after a restart of Horizon.

## 2. Applicability

- Candidate: the frozen candidate of [area S](s-test-fixture.md).
- Platforms: Linux. Cloud: `smoke-a`.
- This area does not test: UDP, IPv6 devices, Windows discovery or a move of
  the PC to another network. See [Local Network Bridge](../../../local-network-bridge.md).

## 3. Safety

> **CAUTION:** SHARE THE LOCAL NETWORK ONLY WITH A TEST CLOUD. While the bridge is
> on, every process on the worker can reach devices on the local network.

> **CAUTION:** USE ONLY A TEST SERVICE THAT THE OPERATOR OWNS. Other devices on the
> local network can belong to other people.

## 4. Equipment and preconditions

- The equipment in the [main procedure](../cloud-panels.md#4-equipment-and-preconditions).
- `smoke-a` shows **Ready**. Its worker image contains the bridge helper.
- The PC is on an IPv4 network that is not wider than `/16`.
- A second device on the local network that the operator owns, with a test HTTP
  service. Record its address as `<device-address>` and its port as `<device-port>`.
- An agent panel in `smoke-a`, for the MCP tools of the `horizon-local-network` server.

## 5. Setup

1. On the second device, start a test HTTP server with a random value.

   ```sh
   python3 -c 'import secrets; print(secrets.token_hex(16))' > nonce && python3 -m http.server <device-port>
   ```

   Result: The server listens on `<device-port>`. Record the value in the private evidence.

2. On the PC, make a test directory with one synthetic file.

   ```sh
   mkdir -p <run>/lnb-test && echo smoke-lnb > <run>/lnb-test/index.txt
   ```

   Result: The directory contains only the synthetic file.

3. On the PC, start a test HTTP server on the loopback address, port 18090.

   ```sh
   python3 -m http.server 18090 --bind 127.0.0.1 --directory <run>/lnb-test
   ```

   Result: The server listens on `127.0.0.1:18090`. N04 uses it.

## 6. Tasks

### 6.1 N01 — Share the local network

1. Open the card of `smoke-a`.

   Result: The card shows **Share local network**. The switch is off.

   > **CAUTION:** THIS STEP CHANGES NETWORK ACCESS. The worker can reach all devices
   > on the local network of the PC until you switch the bridge off.

2. Switch on **Share local network**.

   Result: The card shows **Connecting to share** and the subnet of the PC.

3. Wait until the card shows **Sharing** with the subnet.

   Result: The card shows the subnet, `0 open` and the relayed data.

4. In the worker shell of `smoke-a`, show the bridge status.

   ```sh
   horizon-cloud-worker local-network status
   ```

   Result: The output shows that the bridge is on, the subnet and the SOCKS5 proxy
   address. Record the address as `<proxy-address>`.

### 6.2 N02 — Discover and probe a local service

1. In the agent panel of `smoke-a`, ask the agent to call `local_network_discover`.

   Result: The answer lists devices on the bridged subnet. It does not list the PC.

2. Ask the agent to call `local_network_probe` with `<device-address>` and `<device-port>`.

   Result: The answer shows `<device-port>` as open.

3. In the worker shell, probe the same device with the CLI.

   ```sh
   horizon-cloud-worker local-network probe <device-address> <device-port>
   ```

   Result: The output shows the port as open.

4. In the worker shell, read the test value through the SOCKS5 proxy.

   ```sh
   curl -sS --max-time 20 --socks5-hostname <proxy-address> http://<device-address>:<device-port>/nonce
   ```

   Result: The output is the value from the setup. The card shows the connection
   under **Open connections**.

5. Probe the PC address on the local network.

   ```sh
   horizon-cloud-worker local-network probe <pc-lan-address> 22
   ```

   Result: The bridge refuses the request. The PC is not reachable without an open port.

### 6.3 N03 — Forward and unforward a TCP port

> **CAUTION:** FORWARD ONLY THE TEST SERVICE. A forward lets every process on the
> worker reach that device port.

1. In the agent panel, ask the agent to call `local_network_forward` with `<device-address>` and `<device-port>`.

   Result: The answer gives a worker port on `127.0.0.1`. Record it as `<worker-port>`.

2. In the worker shell, read the test value through the forward.

   ```sh
   curl -sS --max-time 20 http://127.0.0.1:<worker-port>/nonce
   ```

   Result: The output is the value from the setup.

3. Ask the agent to call `local_network_status`.

   Result: The answer lists the forward of `<worker-port>`.

4. Ask the agent to call `local_network_unforward` with `<worker-port>`.

   Result: The answer shows that the forward is closed.

5. In the worker shell, try the forward again.

   ```sh
   curl -sS --max-time 10 http://127.0.0.1:<worker-port>/nonce
   ```

   Result: `curl` cannot connect.

### 6.4 N04 — Limit the reach with the scope

1. On the card of `smoke-a`, open the **Scope** section.

   Result: The section shows **Scope: the whole network**, the **Devices** field
   and the **This computer's own ports** field.

2. Type `<device-address>:<device-port>` in the **Devices** field.

   Result: The field shows one device with one port.

3. Type `18090` in the **This computer's own ports** field.

   Result: The field shows one port.

   > **CAUTION:** THIS STEP CHANGES NETWORK ACCESS. The worker can reach port 18090
   > on the loopback address of the PC.

4. Click **Apply scope**.

   Result: The card shows **Scope: 1 device · 1 port on this computer**.

5. In the worker shell, read the test value from the device.

   ```sh
   curl -sS --max-time 20 --socks5-hostname <proxy-address> http://<device-address>:<device-port>/nonce
   ```

   Result: The output is the value from the setup.

6. In the worker shell, probe another port of the device.

   ```sh
   horizon-cloud-worker local-network probe <device-address> 22
   ```

   Result: The bridge refuses the port, because it is outside the scope.

7. In the worker shell, read a file from the test server on the PC.

   ```sh
   curl -sS --max-time 20 --socks5-hostname <proxy-address> http://localhost:18090/
   ```

   Result: The output shows `index.txt` from the test server on the PC.

8. Clear the **This computer's own ports** field.

   Result: The field is empty.

   > **CAUTION:** THIS STEP CHANGES NETWORK ACCESS. The bridge closes each open
   > connection to a port on the PC.

9. Click **Apply scope**.

   Result: The card shows **Scope: 1 device**.

10. Do step 7 again.

    Result: The bridge refuses the connection to `localhost`.

### 6.5 N05 — Make sure that the bridge is off after a restart

1. Make sure that the card of `smoke-a` shows **Sharing**.

   Result: The bridge is on before the restart.

2. Make the restart marker.

   ```sh
   touch <state>/restart-request
   ```

   Result: The launcher starts the candidate again after the next close.

3. Close the window of the candidate through the window manager.

   Result: The candidate stops. The launcher starts it again.

4. Do S04 again.

   Result: The new child has the frozen SHA-256.

5. Wait until the card of `smoke-a` shows **Ready**.

   Result: The candidate reconnects to the worker.

6. Examine the **Share local network** switch.

   Result: The switch is off. The card shows no scope.

7. In the worker shell, show the bridge status.

   ```sh
   horizon-cloud-worker local-network status
   ```

   Result: The output shows that the bridge is off and that only the owner can turn it on.

## 7. Pass criteria

- The bridge starts only after the switch, and the card shows the subnet and
  the open connections.
- Discovery, probe, the SOCKS5 proxy and a forward reach the test service.
- The bridge refuses the PC without an open port, and a port outside the scope.
- **Apply scope** changes the reach at once.
- The bridge is off after a restart of the candidate.

## 8. Cleanup

1. If the switch **Share local network** is on, switch it off.

   Result: The card shows that the bridge is off.

2. Stop the test HTTP server on the PC with Ctrl-C.

   Result: The server stops.

3. Delete the test directory on the PC.

   ```sh
   rm -r <run>/lnb-test
   ```

   Result: The directory is gone.

4. Stop the test HTTP server on the second device with Ctrl-C.

   Result: The server stops.

## 9. Record of results

Write the results in the report of the run. Use the
[report template](../../reports/TEMPLATE.md). Keep local network addresses and
device names out of the repository.
