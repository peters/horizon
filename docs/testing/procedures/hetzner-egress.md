---
procedure: hetzner-egress
feature: Outbound allowlist of a Hetzner worker (profile `egress`, used by Upkeep)
platforms: [linux]
cost: rents compute
destructive: yes (EGR-07 deletes the server, the volume and the SSH key that the setup made)
secrets: [Hetzner API token file of Cloud settings, Anthropic API key file of Cloud settings]
owner: peters
---

# Outbound allowlist test procedure

## 1. Purpose

This procedure proves that the host of an allowlisted Hetzner worker lets the worker
container connect only to the names on the allowlist. It also proves that the
container loses its way out, and never gets an open one, when the egress proxy, the
resolver or the firewall rules stop.

## 2. Applicability

- Candidate: a branch where `horizon_cloud::host::Plan` renders the `egress`
  allowlist.
- Platforms: a Linux computer and one Hetzner `cx23` server with the `docker-ce`
  image.
- The check server is a variant of the real user data: host SSH stays on so that
  you can stop the services, and the container publishes SSH on port 2200. All
  other files and rules are the rendered ones.
- This procedure does not test: the deployment through New cloud or New Upkeep
  (their tests check that the profile carries `egress` into the plan), and a
  tailnet.

## 3. Safety

> **CAUTION:** THE SETUP RENTS A HETZNER SERVER AND A VOLUME. The provider bills
> them until EGR-07 deletes them.

> **CAUTION:** DO NOT PRINT THE HETZNER TOKEN OR THE API KEY. Read each one from its
> file into a request header or standard input only. Delete the header file after
> EGR-07.

> **CAUTION:** THE CHECK SERVER ALLOWS ROOT LOGIN OVER SSH ON THE HOST. Use a new key
> for it only, and delete the server after the check.

## 4. Equipment and preconditions

- A Rust toolchain for this repository, `curl`, `ssh` and `python3`.
- Cloud settings with a Hetzner project (`hetzner.token_file`) and, for EGR-04, an
  Anthropic API key file (`anthropic_api_key_file`).
- A folder `<work>` that only you can read, outside the repository.

## 5. Setup

1. Make an SSH key: `ssh-keygen -q -t ed25519 -N '' -f <work>/key`.

   Result: `<work>/key` and `<work>/key.pub` exist.

2. Write the header `Authorization: Bearer <token>` to `<work>/hdr` from the token
   file, with mode 0600. Do not show the file.

3. Create a 10 GB volume with `format: ext4` in `hel1` (`POST /v1/volumes`) and
   register `<work>/key.pub` (`POST /v1/ssh_keys`). Give both the label
   `owner=<you>`.

   Result: The volume has a `linux_device` such as
   `/dev/disk/by-id/scsi-0HC_Volume_<id>`.

4. Write `<work>/plan.json` with `image` (the quick-start pin in
   `crates/horizon-core/src/cloud_runtime/repository/launch/quick_start.rs`),
   `device` (the `linux_device`), `public_key` (the content of `<work>/key.pub`) and
   `out` (`<work>/user-data.yml`).

5. Run `HORIZON_TEST_EGRESS_PLAN=<work>/plan.json cargo test -p horizon-cloud a_live_check_can_render`.

   Result: `<work>/user-data.yml` exists.

6. Make the check variant: in `<work>/user-data.yml`, change `disable_root: true`
   to `disable_root: false`, remove the lines `systemctl mask --now ssh.socket
   ssh.service` and `passwd --lock root`, and change `-p 22:22` to `-p 2200:22`.
   Save it as `<work>/user-data-check.yml`.

7. Create the server: `POST /v1/servers` with `server_type: cx23`, `location: hel1`,
   `image: docker-ce`, the SSH key, the volume, `automount: false`, the label and
   `user_data` from `<work>/user-data-check.yml`.

   Result: The server has a public IPv4 address `<ip>`.

8. Wait until `ssh -i <work>/key root@<ip> 'cloud-init status --wait'` shows
   `status: done`, and `docker ps` on the host shows `horizon-worker`.

## 6. Tasks

### 6.1 EGR-01: The host has the rules and the network

1. On the host, run `iptables -S DOCKER-USER`, `iptables -S HORIZON-EGRESS`,
   `iptables -S HORIZON-EGRESS-IN`, `iptables -t nat -S HORIZON-EGRESS` and
   `docker network inspect horizon-egress`.

   Result: `DOCKER-USER` sends `-i horizon0` to `HORIZON-EGRESS`, which returns
   only `RELATED,ESTABLISHED` and drops the rest. `HORIZON-EGRESS-IN` accepts only
   TCP 3129 and port 53 on `172.30.0.1`. The nat chain redirects TCP 443 to 3129. The
   network has `enable_ip_masquerade` `false`.

### 6.2 EGR-02: Other names and addresses fail

1. Copy [agent-checks.sh](hetzner-egress/agent-checks.sh) into the container and
   run it as the agent account:
   `docker exec horizon-worker setpriv --reuid=10001 --regid=10001 --clear-groups sh /tmp/agent-checks.sh`.

   Result: `example.com` and `danger.example.org` do not resolve. A connection to
   `example.com` with a forced address ends in the TLS handshake. Plain HTTP to
   `1.1.1.1` and DNS to `1.1.1.1` over UDP and TCP time out.

### 6.3 EGR-03: Allowed services work

1. Read the rest of the output of EGR-02.

   Result: `github.com` resolves. `git ls-remote` and a shallow `git clone` from
   GitHub work. The crate download from `static.crates.io` gives 200. `npm install
   left-pad` adds one package. PyPI and NuGet give 200. The model APIs answer with
   an HTTP status (401 or 403 without a login). The GitHub API gives 200.

### 6.4 EGR-04: An agent model call works

1. Run `claude -p --model claude-haiku-4-5 "Reply with exactly: egress ok"` in the
   container as the agent account. Give the key on standard input:
   `read -r ANTHROPIC_API_KEY; export ANTHROPIC_API_KEY`.

   Result: The answer is `egress ok`.

### 6.5 EGR-05: The container fails closed

1. On the host, run [fail-closed.sh](hetzner-egress/fail-closed.sh).

   Result:
   - With the proxy stopped, HTTPS to `api.github.com` fails, and names still
     resolve.
   - After `kill -9` of the proxy, systemd starts a new proxy, and HTTPS works
     again.
   - With the resolver stopped, no name resolves.
   - With every egress rule removed, a direct connection to a GitHub address and DNS
     to `1.1.1.1` get no answer, because the network has no address translation.
   - A restart of `horizon-worker` installs the rules again, and HTTPS works.

### 6.6 EGR-06: Refused names are logged, SSH and a reboot work

1. On the host, show `/mnt/horizon-volume/workspace/.horizon-egress/refused.jsonl`
   and its folder.

   Result: The folder and the file belong to root, with modes 0755 and 0644. The
   file has lines such as `{"at": ..., "name": "example.com", "via": "dns"}`.

2. Run `ssh -i <work>/key -p 2200 root@<ip> hostname`.

   Result: The container answers with its host name.

3. Reboot the host. When the container runs again, run HTTPS to `api.github.com`
   and to `example.com` as the agent account.

   Result: `api.github.com` gives 200 and `example.com` does not resolve.

4. In the container, run `curl -m 6 http://169.254.169.254/hetzner/v1/userdata`.

   Result: The connection times out.

### 6.7 EGR-07: Delete the resources

1. Delete the server. Wait until the volume has no server. Delete the volume and the
   SSH key.

   Result: Lists with the label selector `owner=<you>` show no server, volume or
   key.

2. Delete `<work>/hdr`.

## 7. Pass criteria

- EGR-01 to EGR-06 give their results.
- No refused connection reaches its destination.

## 8. Cleanup

EGR-07 deletes every resource of the setup.

## 9. Record of results

Put the results in the pull request. Keep the server address and private evidence
out of the repository.
