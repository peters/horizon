#!/usr/bin/env bash
# Provision disposable client VM A for the Azure client-off lane (#474 / #475).
#
# Creates one exact task-owned resource group and one Ubuntu VM with key-only SSH, a
# virtual display and an isolated persistent Horizon home on the OS disk (retained
# across deallocation), tagged for the existing spike reaper with the manifest's
# cleanup deadline. Copies the exact Horizon Linux build by SHA. Never touches any
# other resource, never logs in, registers a provider or installs an extension.
#
# Usage: provision-client.sh --manifest manifest.json --ssh-private-key key \
#            --horizon-binary /path/to/horizon --build-record client-build.json \
#            --ssh-source-cidr <this controller's public address>/32 --out client.json
# Diagnostics go to stderr; the client descriptor is written to --out as standalone JSON.
# SSH is admitted only from that source range; the VM is never open to the Internet.
# The key is a fresh Ed25519 pair (`key` and `key.pub`); every SSH and SCP call uses it
# explicitly with IdentitiesOnly, BatchMode and bounded connection attempts.
# The build record comes from record-client-build.sh in the clean checkout the binary
# was built from; the manifest's client_sha and client_binary_sha256 must match it and
# the binary's actual digest, so a binary from another commit is refused.
set -euo pipefail

fail() { printf 'provision-client: %s\n' "$1" >&2; exit 1; }
for tool in az jq ssh scp sha256sum timeout; do command -v "$tool" >/dev/null 2>&1 || fail "required command not found: $tool"; done
# One absolute bound for the whole provisioning and readiness path, started before the
# first cloud call; every step gets only what is left of it, so a hung call can never
# hold a paid VM beyond the bound.
# The absolute bound on the whole provisioning path: 30 minutes. Tests may shorten it
# through the environment; the variable can never lengthen it.
startup_seconds=${HORIZON_CLIENT_OFF_STARTUP_SECONDS:-1800}
[[ "$startup_seconds" =~ ^[0-9]{1,4}$ ]] && [ "$startup_seconds" -ge 1 ] && [ "$startup_seconds" -le 1800 ] \
  || fail "HORIZON_CLIENT_OFF_STARTUP_SECONDS must be an integer between 1 and 1800 (the 30-minute bound)"
# Elapsed time comes from the shell's own counter, not the wall clock, so a clock
# correction can neither stretch nor cut the paid-resource bound; the wall clock is used
# only for the manifest's cleanup timestamp.
SECONDS=0
remaining() { echo $(( startup_seconds - SECONDS )); }
KILL_GRACE=10
bounded() { # bounded <step-cap-seconds> <command...>
  local cap=$1 left budget; shift; left=$(remaining)
  # The kill grace is taken out of what is left, so a step that ignores SIGTERM is
  # killed before the bound, never after it: the 30-minute bound is absolute.
  budget=$(( (left < cap ? left : cap) - KILL_GRACE ))
  [ "$budget" -gt 0 ] || fail "the 30-minute startup bound is exhausted; the group stays tagged for the reaper, delete it by hand"
  timeout -k "$KILL_GRACE" "$budget" "$@"
}
azb() { bounded 600 az "$@"; }
# A create is sent only when, after its own bound, at least this much of the startup
# bound is guaranteed for reading the resource back: an accepted create whose answer is
# lost must always end journaled, never untracked because the deadline ran out.
RECONCILE_WINDOW=${HORIZON_CLIENT_OFF_RECONCILE_SECONDS:-120}
[[ "$RECONCILE_WINDOW" =~ ^[0-9]{1,3}$ ]] && [ "$RECONCILE_WINDOW" -ge 1 ] && [ "$RECONCILE_WINDOW" -le 120 ] \
  || fail "HORIZON_CLIENT_OFF_RECONCILE_SECONDS must be an integer between 1 and 120 (tests may shorten the window, never lengthen it)"
create() { # create <command...>: a mutation that leaves the reconciliation window behind it
  local left cap; left=$(remaining)
  cap=$(( left - KILL_GRACE - RECONCILE_WINDOW ))
  [ "$cap" -ge 30 ] || fail "not enough of the startup bound left to create and then reconcile a resource; nothing created"
  bounded "$(( cap < 600 ? cap : 600 ))" az "$@"
}
nap() { # nap <seconds>: never sleep past the bound
  local left; left=$(remaining)
  [ "$left" -gt 0 ] || fail "the 30-minute startup bound is exhausted; the group stays tagged for the reaper, delete it by hand"
  sleep "$(( left < $1 ? left : $1 ))"
}

manifest= private_key= binary= record= source_cidr= out=
while (($# > 0)); do
  case "$1" in
    --manifest) manifest=$2; shift 2 ;;
    --ssh-private-key) private_key=$2; shift 2 ;;
    --horizon-binary) binary=$2; shift 2 ;;
    --build-record) record=$2; shift 2 ;;
    --ssh-source-cidr) source_cidr=$2; shift 2 ;;
    --out) out=$2; shift 2 ;;
    *) fail "unknown argument: $1" ;;
  esac
done
[ -n "$manifest" ] && [ -n "$private_key" ] && [ -n "$binary" ] && [ -n "$record" ] && [ -n "$source_cidr" ] && [ -n "$out" ] \
  || fail "--manifest, --ssh-private-key, --horizon-binary, --build-record, --ssh-source-cidr and --out are required"
say() { printf 'provision-client: %s\n' "$1" >&2; }
# Everything this run writes locally (journal, descriptor, pinned host key) names the
# run's resources and endpoint: private to this user, whatever the caller's umask.
umask 077
# Every local output is reserved before the first cloud call: a descriptor path that
# cannot be written, or a stale creation journal, must fail here, never after a paid
# group exists without its record.
out_dir=$(dirname "$out")
[ -d "$out_dir" ] && [ -w "$out_dir" ] || fail "the directory for --out ($out_dir) does not exist or is not writable"
# Every local file this run writes later is reserved now with an exclusive create, so
# no pre-existing file or symlink at any of these paths can be followed or overwritten
# once a paid resource exists.
reserve() { [ ! -L "$1" ] && ( set -o noclobber; : >"$1" ) 2>/dev/null || fail "$1 already exists or cannot be reserved; each run needs a fresh directory"; }
reserve "$out.tmp"
reserve "$out"
journal=created-groups.json
for alias in "$out" "$out.tmp"; do
  [ "$(realpath -m "$alias")" != "$(realpath -m "$journal")" ] || fail "--out must not be the creation journal ($journal)"
done
# Exclusive creation: two runs in one directory, or a dangling symlink, can never
# share or overwrite the only cleanup record.
[ ! -L "$journal" ] || fail "$journal is a symlink; each run needs a fresh directory or journal"
( set -o noclobber; echo '[]' >"$journal" ) 2>/dev/null || fail "$journal already exists or cannot be created; each run needs a fresh directory or journal"
reserve "$journal.tmp"
python3 - "$source_cidr" <<'PY' || fail "--ssh-source-cidr must be a globally routable unicast IPv4 range no wider than /24"
import ipaddress, sys
network = ipaddress.IPv4Network(sys.argv[1], strict=True)
# Global and unicast: multicast ranges count as global to ipaddress but can never name
# the controller as an SSH source.
sys.exit(0 if 24 <= network.prefixlen <= 32 and network.is_global and not network.is_multicast else 1)
PY
public_key=$private_key.pub
[ -f "$private_key" ] && [ -f "$public_key" ] || fail "the private key and its .pub must both exist"
# Only the key type and payload reach the VM; a comment or extra line never enters YAML.
# Records, not newline characters: an unterminated second line is still a second line.
[ "$(awk 'END { print NR }' "$public_key")" -le 1 ] || fail "the public key file must be a single line"
client_key=$(awk 'NR==1 && $1=="ssh-ed25519" && $2 ~ /^[A-Za-z0-9+\/]{68}$/ {print $1" "$2}' "$public_key")
[ -n "$client_key" ] || fail "the public key must be one Ed25519 key line"
# The pair must actually be a pair: a public half that the private key cannot answer
# for would create a VM no pinned session can ever enter.
command -v ssh-keygen >/dev/null 2>&1 || fail "required command not found: ssh-keygen"
# Stdin closed and a short timeout: a passphrase-protected key cannot prompt, and could
# never be used by the BatchMode sessions later anyway.
# A failing derivation yields an empty result here (never an exit under `set -e`), so
# the pair check below is what reports it.
derived=$( (timeout 15 ssh-keygen -y -P "" -f "$private_key" 2>/dev/null </dev/null || true) | awk '{print $1" "$2}')
[ "$derived" = "$client_key" ] || fail "the .pub does not belong to the private key, or the key needs a passphrase (BatchMode cannot use it)"
python3 -B "$(dirname "$0")/client_off.py" --manifest "$manifest" validate >/dev/null || fail "manifest is not runnable; fix it before renting"

sub=$(jq -r .subscription_id "$manifest"); location=$(jq -r .location "$manifest")
group=$(jq -r .client_group "$manifest"); size=$(jq -r .client_vm_size "$manifest")
reserve "$group.known_hosts"
sha=$(jq -r .client_sha "$manifest"); deadline=$(jq -r .cleanup_deadline_utc "$manifest")
[ "$(azb group exists -n "$group" --subscription "$sub" -o tsv)" = false ] || fail "client group already exists; choose a fresh exact name"
# The SKU must exist in the location before anything is created, or the group would be
# journaled and paid for while the VM create fails.
# The SKU must be offered without restrictions and be x86-64: the client binary the
# build record describes is an x86-64 ELF, so an Arm64 SKU would only fail after a
# paid VM exists.
sku_state=$(azb vm list-skus -l "$location" --size "$size" --resource-type virtualMachines --subscription "$sub" \
  --query "[?name=='$size'] | [0].[length(restrictions), capabilities[?name=='CpuArchitectureType'].value | [0]]" -o tsv 2>/dev/null \
  | tr -s '[:space:]' ' ' | sed 's/ $//' || true)
[ "$sku_state" = "0 x64" ] || fail "VM size $size is not an unrestricted x86-64 size in $location for this subscription (got '${sku_state:-nothing}')"
# The run identity was drawn when the manifest was frozen and names A's group
# (`horizon-client-<run_id>`, checked by `validate`): no other run can create or
# delete this name, which is what makes the group-create PUT below safe. ARM's create
# is an idempotent PUT that would overwrite a same-named group's tags, and the
# `exists` check above is only a preflight; the name being this run's is the guarantee.
run_id=$(jq -r .run_id "$manifest")
[[ "$run_id" =~ ^[0-9a-f]{32}$ ]] && [ "$group" = "horizon-client-$run_id" ] || fail "manifest run_id and client_group disagree"
binary_sha=$(sha256sum "$binary" | cut -d' ' -f1)
# Provenance: the record's commit and digest must equal the manifest's, and the digest
# must be the bytes about to be uploaded.
[ "$(jq -r .client_sha "$record")" = "$sha" ] || fail "build record commit differs from the manifest's client_sha"
[ "$(jq -r .client_binary_sha256 "$record")" = "$binary_sha" ] || fail "build record digest differs from the binary"
[ "$(jq -r .client_binary_sha256 "$manifest")" = "$binary_sha" ] || fail "manifest client_binary_sha256 differs from the binary"

cloud_init=$(mktemp)
trap 'rm -f "$cloud_init"' EXIT
cat >"$cloud_init" <<CLOUD
#cloud-config
package_update: true
# The runtime the snap ships for the client, plus a software Vulkan driver for wgpu.
packages: [xvfb, openbox, xdotool, x11-utils, imagemagick, openssh-client, git, tmux,
           mesa-vulkan-drivers, libvulkan1, libegl1, libgl1, libgles2, libgbm1, libfontconfig1, libfreetype6,
           libwayland-client0, libwayland-cursor0, libwayland-egl1, libx11-6, libx11-xcb1, libxcb1,
           libxcb-render0, libxcb-shape0, libxcb-xfixes0, libxcursor1, libxi6, libxinerama1, libxrandr2,
           libxkbcommon0, libxkbcommon-x11-0, xdg-utils, fonts-dejavu-core]
users:
  - name: horizon
    shell: /bin/bash
    sudo: ALL=(ALL) NOPASSWD:ALL
    ssh_authorized_keys:
      - $client_key
write_files:
  - path: /etc/systemd/system/horizon-display.service
    content: |
      [Unit]
      Description=Virtual display for the disposable Horizon client
      After=network.target
      [Service]
      User=horizon
      ExecStart=/usr/bin/Xvfb :99 -screen 0 1600x1000x24 -nolisten tcp
      Restart=always
      [Install]
      WantedBy=multi-user.target
  - path: /etc/systemd/system/horizon-wm.service
    content: |
      [Unit]
      Description=Window manager on the disposable Horizon client's display
      Requires=horizon-display.service
      After=horizon-display.service
      [Service]
      User=horizon
      Environment=DISPLAY=:99
      ExecStartPre=/bin/sh -c 'for i in \$(seq 1 50); do xdpyinfo -display :99 >/dev/null 2>&1 && exit 0; sleep 0.2; done; exit 1'
      ExecStart=/usr/bin/openbox
      Restart=always
      [Install]
      WantedBy=multi-user.target
runcmd:
  - [ systemctl, daemon-reload ]
  - [ systemctl, enable, --now, horizon-display.service, horizon-wm.service ]
  - [ install, -d, -o, horizon, -g, horizon, -m, '0700', /home/horizon/.horizon-client-home, /home/horizon/horizon-client/$sha ]
CLOUD

say "creating group $group in $location (deadline $deadline)"
# A pending ownership record goes into the journal before the create is sent: it names
# the exact ARM ID and tag set this run is about to create, so even if the create is
# accepted and every read afterwards is lost, cleanup can still re-attest the group
# from ARM and delete it (a group that was never created reads as unknown and is left
# alone). The record is replaced by ARM's own answer once the group is confirmed.
# The window check comes before the pending record: a run that cannot create and then
# reconcile leaves the journal empty, since nothing was sent.
[ $(( $(remaining) - KILL_GRACE - RECONCILE_WINDOW )) -ge 30 ] \
  || fail "not enough of the startup bound left to create and then reconcile a resource; nothing created"
expected_tags=$(jq -n --arg d "$deadline" --arg s "$sha" --arg b "$binary_sha" --arg r "$run_id" \
  '{issue:"475",lane:"azure-client-off",purpose:"horizon-azure-vm-spike",deadline:$d,client_sha:$s,client_binary_sha256:$b,run_id:$r}')
jq --arg name "$group" --arg id "/subscriptions/$sub/resourceGroups/$group" --argjson tags "$expected_tags" \
  '. + [{name: $name, id: $id, tags: $tags}]' "$journal" >"$journal.tmp" \
  || fail "the pending ownership record could not be written; nothing created"
mv "$journal.tmp" "$journal" || fail "the pending ownership record could not be placed; nothing created"
reserve "$journal.tmp"
# The create may be accepted while the CLI loses the answer: never trust the exit code
# alone. Reconcile read-only and confirm the group only when it exists with this run's
# tags; a group that exists with other tags is not ours and stops the run.
create group create -n "$group" -l "$location" --subscription "$sub" \
  --tags issue=475 lane=azure-client-off purpose=horizon-azure-vm-spike "deadline=$deadline" client_sha="$sha" client_binary_sha256="$binary_sha" run_id="$run_id" >/dev/null || true
# Reconcile for as long as the startup bound allows: a create that ARM accepted while
# the read path is slow must still end in a journaled group, never an untracked one.
reconciled=false
while [ "$(remaining)" -gt "$KILL_GRACE" ]; do
  shown=$(azb group show -n "$group" --subscription "$sub" -o json 2>/dev/null || true)
  if [ -n "$shown" ]; then
    # Parsed comparison: Azure may return the tag keys in any order.
    jq -e --arg d "$deadline" --arg s "$sha" --arg b "$binary_sha" --arg r "$run_id" \
      '(.tags // {}) == {issue:"475",lane:"azure-client-off",purpose:"horizon-azure-vm-spike",deadline:$d,client_sha:$s,client_binary_sha256:$b,run_id:$r}' \
      <<<"$shown" >/dev/null || fail "group $group exists but its tags are not exactly this run's; refusing to adopt it"
    # The record must identify exactly this group: its name and its ARM ID under this subscription.
    [ "$(jq -r '.name // empty' <<<"$shown")" = "$group" ] \
      && [ "$(jq -r '.id // empty' <<<"$shown" | tr '[:upper:]' '[:lower:]')" = "$(printf '/subscriptions/%s/resourcegroups/%s' "$sub" "$group" | tr '[:upper:]' '[:lower:]')" ] \
      || fail "group $group answered with an unexpected name or ARM ID; refusing to journal it"
    reconciled=true; break
  fi
  nap 5
done
[ "$reconciled" = true ] || fail "group $group could not be confirmed after the create attempt; its pending record stays in $journal for cleanup to re-attest; check the subscription by hand before retrying"
# Replace the pending record with the exact identity ARM reports (ID and full tag set)
# the moment the group is confirmed.
jq --argjson shown "$shown" --arg name "$group" \
  'map(if .name == $name then {name: $shown.name, id: $shown.id, tags: ($shown.tags // {})} else . end)' "$journal" >"$journal.tmp" \
  || fail "the creation journal could not be written; group $group exists (ID $(jq -r .id <<<"$shown")), its pending record stays for cleanup"
mv "$journal.tmp" "$journal" || fail "the creation journal could not be replaced; group $group exists, its pending record stays for cleanup"
say "creating client VM ($size), SSH admitted from $source_cidr only"
create vm create -g "$group" -n client --subscription "$sub" --image Ubuntu2404 --size "$size" \
  --admin-username horizon --ssh-key-values "$client_key" --public-ip-sku Standard --nsg-rule NONE \
  --os-disk-size-gb 32 --custom-data "$cloud_init" \
  --tags issue=475 lane=azure-client-off purpose=horizon-azure-vm-spike "deadline=$deadline" client_sha="$sha" client_binary_sha256="$binary_sha" run_id="$run_id" >/dev/null || true
# The create is an ambiguous mutation once sent: read the exact VM back with this run's
# tags before going on, and fail (with the group journaled) only when it is not there.
vm_seen=false
while [ "$(remaining)" -gt "$KILL_GRACE" ]; do
  vm_json=$(azb vm show -g "$group" -n client --subscription "$sub" -o json 2>/dev/null || true)
  if [ -n "$vm_json" ]; then
    jq -e --arg d "$deadline" --arg s "$sha" --arg b "$binary_sha" --arg r "$run_id" \
      '(.tags // {}) == {issue:"475",lane:"azure-client-off",purpose:"horizon-azure-vm-spike",deadline:$d,client_sha:$s,client_binary_sha256:$b,run_id:$r}' \
      <<<"$vm_json" >/dev/null || fail "VM client exists in $group but its tags are not exactly this run's; refusing to adopt it"
    vm_seen=true; break
  fi
  nap 10
done
[ "$vm_seen" = true ] || fail "the client VM could not be confirmed after the create attempt; the group is journaled for cleanup"
azb network nsg rule create -g "$group" --nsg-name clientNSG -n ssh-from-controller --subscription "$sub" \
  --priority 1000 --direction Inbound --access Allow --protocol Tcp --destination-port-ranges 22 \
  --source-address-prefixes "$source_cidr" >/dev/null
# Every identity value is read back and checked for shape; a `null`, an empty answer or
# a value of the wrong form never reaches the pin or the descriptor.
# The address can lag the VM while the network converges: poll it under the bound.
host=
while [ "$(remaining)" -gt "$KILL_GRACE" ]; do
  host=$(azb vm show -d -g "$group" -n client --subscription "$sub" --query publicIps -o tsv 2>/dev/null || true)
  [[ "$host" =~ ^[0-9]{1,3}(\.[0-9]{1,3}){3}$ ]] && break
  nap 10
done
vm_id=$(jq -r '.id // empty' <<<"$vm_json")
instance_id=$(jq -r '.vmId // empty' <<<"$vm_json")
group_id=$(jq -r '.id // empty' <<<"$shown")
[[ "$host" =~ ^[0-9]{1,3}(\.[0-9]{1,3}){3}$ ]] || fail "the client VM has no usable public address (got '${host:-empty}')"
# The IDs must be exactly the manifest-determined paths (ARM may vary their casing):
# a `client` VM anywhere else is not this run's trust anchor.
lower() { tr '[:upper:]' '[:lower:]' <<<"$1"; }
expected_group_id="/subscriptions/$sub/resourcegroups/$group"
[ "$(lower "$vm_id")" = "$(lower "$expected_group_id/providers/Microsoft.Compute/virtualMachines/client")" ] \
  || fail "the client VM ID is not the manifest's VM under the manifest subscription (got '${vm_id:-empty}')"
[[ "$instance_id" =~ ^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$ ]] || fail "the client VM instance identity (vmId) could not be read"
[ "$(lower "$group_id")" = "$(lower "$expected_group_id")" ] || fail "the client group ID is not the manifest's group under the manifest subscription"
# A's host key comes through the control plane, never from the first TCP handshake: the
# run-command channel executes inside the exact VM, so the pin is bound to the resource.
say "reading the client's SSH host key through the control plane"
host_key=
while [ "$(remaining)" -gt 0 ]; do
  message=$(azb vm run-command invoke -g "$group" -n client --subscription "$sub" --command-id RunShellScript \
    --scripts 'cat /etc/ssh/ssh_host_ed25519_key.pub' --query 'value[0].message' -o tsv 2>/dev/null || true)
  # The complete answer must be exactly one Ed25519 key line (the classic run-command
  # message wraps stdout in `[stdout]`/`[stderr]` markers, which are stripped first,
  # and stderr must be empty); any other line makes the answer ambiguous, not a pin.
  # The whole message must have exactly the expected shape: an optional
  # `Enable succeeded:` line, `[stdout]`, exactly one Ed25519 key line, `[stderr]`,
  # and nothing but blank lines anywhere else. Any other line, before, between or
  # after the markers, is an ambiguous answer and never a pin.
  candidate=$(printf '%s\n' "$message" | awk '
    /^[[:space:]]*$/ { next }
    state == 0 && /^Enable succeeded: *$/ { next }
    state == 0 && $0 == "[stdout]" { state = 1; next }
    state == 1 && NF >= 2 && NF <= 3 && $1 == "ssh-ed25519" && $2 ~ /^[A-Za-z0-9+\/]{68}$/ && key == "" { key = $1 " " $2; next }
    state == 1 && $0 == "[stderr]" && key != "" { state = 2; next }
    { bad = 1 }
    END { if (!bad && state == 2) print key }')
  if [ -n "$candidate" ]; then
    host_key=$candidate; break
  fi
  # An answer that arrived but does not have the shape is a failure, not a retry.
  [ -z "$(printf '%s' "$message" | tr -d '[:space:]')" ] || fail "the control plane answered with something other than exactly one Ed25519 host key line between the markers; refusing to pin an ambiguous host key"
  nap 10
done
[ -n "$host_key" ] || fail "the client's Ed25519 host key could not be read through the control plane; the group stays tagged for the reaper, delete it by hand"
# The key was read through a name-based run-command: the exact VM must still be the
# one captured above (instance identity and tags) before its key is pinned or recorded.
vm_after=$(azb vm show -g "$group" -n client --subscription "$sub" -o json)
[ "$(jq -r '.vmId // empty' <<<"$vm_after")" = "$instance_id" ] && jq -e --argjson before "$(jq '.tags // {}' <<<"$vm_json")" '(.tags // {}) == $before' <<<"$vm_after" >/dev/null \
  || fail "the client VM changed identity while its host key was read; refusing to pin"
# OpenSSH looks a default-port host up by its bare address, not the bracketed form.
printf '%s %s\n' "$host" "$host_key" >"$group.known_hosts"
ssh_opts=(-F /dev/null -i "$private_key" -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=10
          -o UserKnownHostsFile="$group.known_hosts" -o GlobalKnownHostsFile=/dev/null -o StrictHostKeyChecking=yes
          -o ServerAliveInterval=15 -o ServerAliveCountMax=4)
say "waiting for SSH on the client (pinned)"
connected=false
while [ "$(remaining)" -gt 0 ]; do
  if bounded 60 ssh "${ssh_opts[@]}" "horizon@$host" true 2>/dev/null; then connected=true; break; fi
  nap 5
done
[ "$connected" = true ] || fail "the client VM never accepted pinned SSH within the readiness bound; it stays tagged for the reaper, delete the group by hand"
# sshd answers before cloud-init finished: wait for it, then require the display service
# and the destination directory the runcmd stage creates. Every remote step is bounded
# on the controller side as well.
bounded 960 ssh "${ssh_opts[@]}" "horizon@$host" \
  "timeout 900 cloud-init status --wait >/dev/null && systemctl is-active --quiet horizon-display.service && systemctl is-active --quiet horizon-wm.service && test -d /home/horizon/horizon-client/$sha" \
  || fail "cloud-init did not complete with the display and window-manager services active and the client directory present"
bounded 900 scp "${ssh_opts[@]}" "$binary" "horizon@$host:/home/horizon/horizon-client/$sha/horizon" \
  || fail "the client binary upload did not complete within its bound"
bounded 120 ssh "${ssh_opts[@]}" "horizon@$host" \
  "chmod 0755 /home/horizon/horizon-client/$sha/horizon && sha256sum /home/horizon/horizon-client/$sha/horizon" \
  | grep -q "^$binary_sha " || fail "client binary digest mismatch after upload"
# Launch gate: the uploaded client must open a window on A's display with the installed
# runtime and software Vulkan before A counts as ready. An ephemeral run writes nothing.
say "launch gate: starting the client once on the virtual display"
bounded 180 ssh "${ssh_opts[@]}" "horizon@$host" "bash -s" <<GATE || fail "the client did not open a window on the virtual display; A is not ready"
set -eu
export DISPLAY=:99 XDG_RUNTIME_DIR=/run/user/\$(id -u) HOME=/home/horizon/.horizon-client-home
# A fresh client is the only Horizon on this VM; anything already running is a fault.
! pgrep -x horizon >/dev/null || { echo "a Horizon process is already running on A" >&2; exit 1; }
mkdir -p "\$XDG_RUNTIME_DIR" 2>/dev/null || true
/home/horizon/horizon-client/$sha/horizon --ephemeral >/tmp/horizon-launch-gate.log 2>&1 &
pid=\$!
for _ in \$(seq 1 120); do
  # Only a window owned by the process just started counts; a stale window does not.
  if xdotool search --pid "\$pid" --name Horizon >/dev/null 2>&1; then kill "\$pid" 2>/dev/null || true; wait "\$pid" 2>/dev/null || true; exit 0; fi
  if ! kill -0 "\$pid" 2>/dev/null; then echo "client exited before opening a window" >&2; tail -n 20 /tmp/horizon-launch-gate.log >&2; exit 1; fi
  sleep 1
done
kill "\$pid" 2>/dev/null || true
echo "no window within the gate" >&2
exit 1
GATE
# The exact A: off and return attest this identity before any mutation.
jq -n --arg group "$group" --arg group_id "$group_id" --arg vm_id "$vm_id" --arg instance "$instance_id" --arg run "$run_id" \
  --arg host "$host" --arg sha "$sha" --arg digest "$binary_sha" --arg journal "$journal" \
  '{client_group:$group, client_group_id:$group_id, client_vm_id:$vm_id, client_instance_id:$instance, run_id:$run, client_host:$host, client_sha:$sha, client_binary_sha256:$digest, client_home:"/home/horizon/.horizon-client-home", client_state_root:"/home/horizon/.horizon-client-home/.horizon", display:":99", created_journal:$journal}' \
  >"$out.tmp" || fail "the client descriptor could not be written; the group is journaled for cleanup"
mv -f "$out.tmp" "$out" || fail "the client descriptor could not be placed at $out; the group is journaled for cleanup"
say "client descriptor written to $out"
