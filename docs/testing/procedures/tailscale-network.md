---
procedure: tailscale-network
feature: A rented machine joins a Tailscale network
platforms: [linux, macos]
cost: rents compute
destructive: yes
secrets: TS_<NET>_CLIENT_ID, TS_<NET>_CLIENT_SECRET, TS_<NET>_TAG
owner: peters
---

# Tailscale network procedure

## 1. Purpose

This procedure makes sure that an agent joins one rented machine to one stored
Tailscale network from the card of the machine, and that the agent keeps the
network apart from the other set-up steps. It proves six things:

1. The stored networks appear as pills on the rent card, with `None` first.
2. A client of the scope `auth_keys` only makes one ephemeral key, and the card
   says that the node leaves the network with the machine.
3. The machine answers `tailscale status` with its own name, its tag and its
   network.
4. The card of a computer that is in another network says that the machine is
   reached over its public address, and it offers no close of port 22.
5. `Delete` logs the machine out of the network, and the machine is deleted.
6. The recipe of a repository with the entry `network:` joins the machine as its
   own step, and the ready card holds the row `Network`.

## 2. Applicability

- Candidate: a frozen build of one exact commit, `<candidate-commit>`.
- Platforms: Linux and macOS. On macOS, use Cmd instead of Ctrl in each step.
- Watch the candidate live through a native VNC Device panel in your own Horizon
  workspace, as `AGENTS.md` requires.
- This procedure does not test the sharing of a device with a second network, the
  exit node, the peer relay or the jump host. They belong to the section 12 of
  the page `references/tailscale.md` of the skill `horizon-machines`.

## 3. Safety

> **CAUTION:** THE PROCEDURE RENTS ONE MACHINE. It costs money from the press on
> `Rent this machine` until the delete in task 6.5. Read the price on the card
> before the press.

> **CAUTION:** TASKS 6.2 AND 6.6 JOIN THE MACHINE TO A NETWORK. The machine takes
> the tag of the network, and every device of the network that the policy allows
> reaches it. The key of the machine is the only login of the machine.

> **CAUTION:** TASK 6.5 DELETES THE MACHINE AND ITS DISK. This cannot be undone,
> and the tmux sessions of the machine end with it.

## 4. Equipment and preconditions

- One Tailscale network of your own, with one tag, for example `tag:horizon`. The
  tag is in `tagOwners` of the policy file.
- One OAuth client of that network with the scope `auth_keys` only and that tag.
  Make it in the Tailscale admin console, at **Trust credentials**, **OAuth**.
- The three values of that network in the environment of your shell, as
  `TS_HOME_CLIENT_ID`, `TS_HOME_CLIENT_SECRET` and `TS_HOME_TAG`. Keep them in a
  password manager and export them in the shell of this run. Do not write them in
  a file of the repository.
- A second Tailscale network on this computer, or no network on this computer.
  Task 6.4 needs a computer that is not in the network of the machine.
- A Hetzner login in `hcloud`, a Linux computer with the programs of the
  [local device smoke fixture](../../../scripts/device-smoke/README.md), and your
  own Horizon with an agent panel that can call `device_panel`.
- The candidate binary, `<candidate>`, a frozen copy of it in `<smoke_bin>`, its
  SHA-256 hash, and an empty state folder `<state>`.
- The agent program of the fixture has the bundled skills installed, and its
  panel is the panel of task 6.1.
- The machine key of the machine of this run. The answer of `machine_key` holds
  `private_key_path` and `known_hosts_path`, and those two paths are
  `<private_key_path>` and `<known_hosts_path>` of this procedure. The same files
  are in `<state>/data/home/.horizon/machine-keys/`. Do not write the key.
- `<address>` is the public address of the machine: the row `Address` of its card,
  and the field `ssh.host` of its record. The user of a Hetzner machine is
  `root`.

## 5. Setup

1. Freeze the candidate and record its hash.

   ```sh
   cp <candidate> <smoke_bin>
   sha256sum <smoke_bin>
   ```

   Result: one hash line. Keep it for task 5 of this section and for the report.

2. Start the fixture with the three named secrets from its own environment. The
   fixture reads the value of each variable, and the value is in no argument of a
   process.

   ```sh
   TS_HOME_CLIENT_ID=... TS_HOME_CLIENT_SECRET=... TS_HOME_TAG=... \
     scripts/device-smoke/serve.py --horizon <smoke_bin> --state <state> --native-view \
       --named-secret-env TS_HOME_CLIENT_ID --named-secret-env TS_HOME_CLIENT_SECRET \
       --named-secret-env TS_HOME_TAG
   ```

   Result: the fixture prints its manifest, with `vnc_address` and its owned
   process IDs. The secrets file of the private home holds three rows
   `named/TS_HOME_*`, and no line of the output holds a value. The secret comes
   from the environment of the fixture, and never from an argument of a process.

3. Open the viewer from the printed `vnc_address` with `device_panel`, in the
   workspace of this run.

   Result: the isolated desktop shows Horizon, and `image_displayed` is true with
   an advancing `frame_sequence`.

4. Prove the executable of the running Horizon, as the fixture README says.

   Result: `/proc/<horizon-pid>/exe` is the frozen copy, and its hash is the hash
   of step 1.

5. Make sure that this computer is not in the network of the machine.

   ```sh
   tailscale status --json | jq -r '.BackendState, .CurrentTailnet.MagicDNSSuffix'
   ```

   Result: `Running`, then the suffix of another network, or `Stopped` with an
   empty second line. Keep both lines for task 6.4.

## 6. Tasks

### 6.1 NET-PILLS — the stored networks appear as pills

1. In the agent panel, write:

   ```text
   Use the horizon-setup skill. The person pressed Set up. Goal: rent.
   ```

2. Press the provider `Hetzner`, then continue to the card `How big, and where?`.

   Result: the card holds the caption `Network` below the place pills, and its
   first pill is `None`. The other pills are `home` and the other stored
   networks, in name order.
3. Write `show me the names of my networks` in the panel.

   Result: the agent answers with names such as `TS_HOME_CLIENT_ID`, and no
   value. A name that holds no value is not a pill.

Task 6.1 rents no machine. The card of task 6.2 is the same card.

### 6.2 NET-JOIN — the narrow client makes one ephemeral key

> **CAUTION:** THIS TASK RENTS ONE MACHINE AND JOINS IT TO THE NETWORK. Read the
> price on the card first.

1. On the card `How big, and where?`, press the pill `home`, then press
   `Rent this machine`.

   Result: the steps card holds the step `Join the network home` after
   `Check tmux on the machine`, and before `Add the machine to Machines`.
2. Wait until the card of the machine answers.

   Result: the card holds the rows `Network` with `home`, `Network name` with the
   MagicDNS name, and `Internet SSH` with `open` or `closed`. One `text` line
   says that the node leaves the network with the machine.
3. Read the scope of the client on this computer, in a shell that holds the three
   variables.

   ```sh
   printf '%s' "$TS_HOME_CLIENT_SECRET" | curl --silent --show-error --fail \
     --data-urlencode "client_id=$TS_HOME_CLIENT_ID" --data-urlencode client_secret@- \
     https://api.tailscale.com/api/v2/oauth/token | jq -r '.scope'
   ```

   Result: `auth_keys`, and no other scope. The card of step 2 named the
   ephemeral node, so the agent made an ephemeral key.

### 6.3 NET-PROOF — the machine answers for its own node

1. Read the state of the node on the machine, with the machine key of the
   fixture.

   ```sh
   ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
       -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
       -o BatchMode=yes -o ConnectTimeout=10 root@<address> \
       'tailscale status --json | jq -r ".BackendState, .MagicDNSSuffix, .Self.DNSName, .Self.Tags"'
   ```

   Result: `Running`, the suffix of the network, the MagicDNS name of the
   machine, and `["tag:horizon"]`. The name in the card of task 6.2 is the same
   name.

2. Read the address of the node.

   ```sh
   ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
       -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
       -o BatchMode=yes -o ConnectTimeout=10 root@<address> 'tailscale ip -4'
   ```

   Result: one `100.x.y.z` address, and nothing else.

### 6.4 NET-OTHER — the card of a computer in another network

1. Read the two suffixes: the suffix of step 1 of task 6.3, and the second line
   of step 5 of section 5.

   Result: the two lines differ, or the second line is empty.
2. Read the card of the machine.

   Result: one `text` line says
   `This computer is not in the network home; the machine is reached over its
   public address.` The card offers no close of port 22, and the agent ran no
   command that changed the state of this computer.
3. Prove that port 22 is still open.

   ```sh
   nc -z -w 5 <address> 22
   ```

   Result: the command answers `succeeded`, and the card keeps
   `Internet SSH: open`.
4. Prove that the machine key still logs in over the public address, with the
   form of step 1 of task 6.5.

   Result: exit code 0, because the record of the machine keeps its public
   address.

### 6.5 NET-DELETE — the delete logs the node out

> **CAUTION:** THIS TASK DELETES THE MACHINE AND ITS DISK. This cannot be undone.

1. Prove the login of the machine key over the public address.

   ```sh
   ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
       -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
       -o BatchMode=yes -o ConnectTimeout=10 root@<address> 'true'
   ```

   Result: exit code 0.
2. Press `Delete` on the card of the machine, then `Delete it`.

   Result: the card answers `Deleted`, and the agent ran `tailscale logout` on
   the machine, then the delete of the provider.
3. Read the node of the machine in the admin console, at **Machines**.

   Result: the node is gone, or it leaves after the machine stops. A node that
   stays names the reason on the card.
4. Read the list of the provider.

   ```sh
   hcloud server list -l horizon-managed=true
   ```

   Result: no row for the machine of this run.

### 6.6 NET-RECIPE — the recipe joins the machine as its own step

> **CAUTION:** THIS TASK RENTS ONE MACHINE FOR THE REPOSITORY AND JOINS IT TO THE
> NETWORK. Delete that machine at the end of the task.

1. In `<example-repo>`, add the entry `network: home` to the section
   `## Horizon machine` of `AGENTS.md`, and commit it.
2. In the agent panel, write:

   ```text
   Use the horizon-machines skill. Rent one machine for <example-repo>.
   ```

3. Read the rent card, and press `Rent 1 machine`.

   Result: the pill `home` is selected on the rent card, and the steps card holds
   seven steps, with `Join the network home` after `Install the tools` and
   before `Bring the repository`, and `Trust the repository folder` after
   `Bring the repository`.
4. Wait until the check of the recipe answers.

   Result: the ready card holds the row `Network` with `home`, and its steps list
   holds the step `Join the network home` as `done`.
5. Delete the machine with the delete of the skill.

   Result: the same result as task 6.5, and the record of the machine is gone.

## 7. Pass criteria

- The rent card holds the caption `Network` only when the panel stores one or
  more networks, and its first pill is `None`.
- The agent reads `scope` of the token answer, and a client of `auth_keys` only
  makes one ephemeral key.
- The machine answers `tailscale status` with its own name, its tag and the
  suffix of the network.
- The card of a computer in another network holds the line of task 6.4, and it
  offers no close of port 22, and no command of the run changed the Tailscale
  state of this computer.
- `Delete` logs the machine out before the provider delete, and the node is gone
  from the console.
- The recipe with `network: home` joins the machine before the repository
  arrives, and its ready card holds the row `Network`.
- No card, no panel line and no log line holds a value of the three secrets.

## 8. Cleanup

1. Delete every machine that the procedure rented, with the delete of the skill.

   Result: the list of the provider holds no row with `horizon-managed=true`.

2. Close the Device panel of this run with `device_panel`, and stop the fixture
   process that step 2 of section 5 started.

   Result: the fixture is gone, and its state folder `<state>` stays for the
   report.

3. Remove the entry `network: home` from `AGENTS.md` of `<example-repo>` when the
   repository does not need it.

   Result: the file is as it was before the procedure.

4. Remove `<smoke_bin>` and the state folder.

   Result: no file of this run stays.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep the name of the network, the addresses and the private
evidence out of the repository. Name each task by its ID, and give the hash of
the frozen candidate.
