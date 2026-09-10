#!/usr/bin/env bash
# Bounded three-sample Azure Linux VM spike for issue #474.
#
# One exact task-owned resource group per sample. Measures create, image pull,
# endpoint, verified key-only SSH and delete timing; records on-worker ext4 kernel
# options, detach independence, deallocate/start data retention and exact deletion
# with an unchanged-inventory proof. Every timestamp is UTC and journaled privately.
# Nothing here registers a provider, publishes an image or touches resources outside
# the sample resource group. Run the read-only preflight first.
set -euo pipefail

usage() {
  cat <<'EOF'
usage: run-vm-spike.sh --subscription <uuid> --image <registry/repo@sha256:...> \
         --puller-identity-id <user-assigned-identity-resource-id> --journal-dir <private-dir> \
         [--region northeurope] [--vm-size Standard_D4s_v3] [--sample 1] [--max-minutes 120] \
         [--allow-ssh-from <cidr>] [--keep] [--dry-run]

Requires: az (logged in), ssh, ssh-keygen, nc, jq, curl. Paid creation is bounded by
--max-minutes; the sample resource group is deleted at the end unless --keep is given.
EOF
}

SUBSCRIPTION="" IMAGE="" PULLER_ID="" JOURNAL_DIR="" REGION=northeurope VM_SIZE=Standard_D4s_v3
SAMPLE=1 MAX_MINUTES=120 ALLOW_SSH_FROM="" KEEP=0 DRY_RUN=0
while [ $# -gt 0 ]; do
  case "$1" in
    --subscription) SUBSCRIPTION=$2; shift 2 ;;
    --image) IMAGE=$2; shift 2 ;;
    --puller-identity-id) PULLER_ID=$2; shift 2 ;;
    --journal-dir) JOURNAL_DIR=$2; shift 2 ;;
    --region) REGION=$2; shift 2 ;;
    --vm-size) VM_SIZE=$2; shift 2 ;;
    --sample) SAMPLE=$2; shift 2 ;;
    --max-minutes) MAX_MINUTES=$2; shift 2 ;;
    --allow-ssh-from) ALLOW_SSH_FROM=$2; shift 2 ;;
    --keep) KEEP=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage; exit 3 ;;
  esac
done

[[ $SUBSCRIPTION =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]] || { echo "--subscription must be a UUID" >&2; exit 3; }
[[ $IMAGE =~ ^[a-z0-9.-]+\.azurecr\.io/[a-z0-9._/-]+@sha256:[0-9a-f]{64}$ ]] || { echo "--image must be an ACR digest reference" >&2; exit 3; }
[[ $PULLER_ID =~ ^/subscriptions/[0-9a-f-]{36}/resource[Gg]roups/[^/]+/providers/Microsoft\.ManagedIdentity/userAssignedIdentities/[A-Za-z0-9_-]+$ ]] || { echo "--puller-identity-id must be a user-assigned identity resource id" >&2; exit 3; }
[[ $REGION =~ ^[a-z][a-z0-9]{1,63}$ ]] || { echo "--region must be a lowercase region name" >&2; exit 3; }
[[ $VM_SIZE =~ ^Standard_[A-Za-z0-9_-]+$ ]] || { echo "--vm-size must look like Standard_D4s_v3" >&2; exit 3; }
[[ $SAMPLE =~ ^[1-9]$ ]] || { echo "--sample must be 1-9" >&2; exit 3; }
[[ $MAX_MINUTES =~ ^[0-9]{1,3}$ ]] && [ "$MAX_MINUTES" -ge 10 ] || { echo "--max-minutes must be 10-999" >&2; exit 3; }
[ -n "$JOURNAL_DIR" ] || { echo "--journal-dir is required" >&2; exit 3; }
for tool in az ssh ssh-keygen nc jq curl; do command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 3; }; done

REGISTRY=${IMAGE%%/*}
RUN_ID=$(date -u +%Y%m%dT%H%M%SZ)
TOKEN=$(head -c 3 /dev/urandom | od -An -tx1 | tr -d ' \n')
RG="horizon-spike-474-s${SAMPLE}-${TOKEN}"
VM="worker-s${SAMPLE}"
DEADLINE_EPOCH=$(( $(date +%s) + MAX_MINUTES * 60 ))
umask 077
mkdir -p "$JOURNAL_DIR"
SAMPLE_DIR="$JOURNAL_DIR/sample-${SAMPLE}-${RUN_ID}"
mkdir "$SAMPLE_DIR"
JOURNAL="$SAMPLE_DIR/journal.jsonl"
KEY="$SAMPLE_DIR/client-ed25519"
KNOWN_HOSTS="$SAMPLE_DIR/known_hosts"

azc() { az "$@" --subscription "$SUBSCRIPTION" --only-show-errors; }
now() { date -u +%Y-%m-%dT%H:%M:%S.%3NZ; }
epoch_ms() { date +%s%3N; }
journal() { local data=${2:-'{}'}; jq -cn --arg at "$(now)" --arg event "$1" --argjson data "$data" '{at:$at,event:$event,data:$data}' >>"$JOURNAL"; }
say() { printf '[%s] %s\n' "$(now)" "$*"; }
deadline_check() { [ "$(date +%s)" -lt "$DEADLINE_EPOCH" ] || { say "lifetime bound reached; forcing cleanup"; cleanup; exit 4; }; }

cleanup() {
  local started finished exists
  if [ "$KEEP" = 1 ]; then say "--keep given; leaving $RG in place (delete it yourself)"; journal kept '{"resource_group":"'"$RG"'"}'; return; fi
  if [ "$DRY_RUN" = 1 ]; then return; fi
  started=$(epoch_ms)
  say "deleting resource group $RG"
  azc group delete --name "$RG" --yes >/dev/null 2>&1 || true
  for _ in $(seq 1 60); do
    exists=$(azc group exists --name "$RG" -o tsv 2>/dev/null || echo unknown)
    [ "$exists" = false ] && break
    sleep 10
  done
  finished=$(epoch_ms)
  azc resource list --query "sort([].id)" >"$SAMPLE_DIR/inventory-after.json"
  local unchanged=false
  cmp -s "$SAMPLE_DIR/inventory-before.json" "$SAMPLE_DIR/inventory-after.json" && unchanged=true
  journal deleted "$(jq -cn --arg exists "$exists" --argjson ms $((finished - started)) --argjson unchanged $unchanged '{group_exists:$exists,delete_ms:$ms,inventory_unchanged:$unchanged}')"
  say "resource group exists=$exists inventory_unchanged=$unchanged delete_ms=$((finished - started))"
}

say "sample $SAMPLE run $RUN_ID journal $SAMPLE_DIR"
journal start "$(jq -cn --arg region "$REGION" --arg size "$VM_SIZE" --arg image "$IMAGE" --arg rg "$RG" --argjson max "$MAX_MINUTES" '{region:$region,vm_size:$size,image:$image,resource_group:$rg,max_minutes:$max}')"

ssh-keygen -q -t ed25519 -N "" -C "horizon-spike-474-s${SAMPLE}" -f "$KEY"
PUBKEY=$(cat "$KEY.pub")
CLIENT_ID=$(azc identity show --ids "$PULLER_ID" --query clientId -o tsv)

CLOUD_INIT="$SAMPLE_DIR/cloud-init.yaml"
cat >"$CLOUD_INIT" <<EOF
#cloud-config
package_update: true
packages: [docker.io]
bootcmd:
  - [ sh, -c, 'printf "{\"event\":\"boot\",\"at\":\"%s\"}\n" "\$(date -u +%FT%T.%3NZ)" >> /var/log/horizon-spike-timing.jsonl' ]
write_files:
  - path: /usr/local/sbin/horizon-spike-bootstrap.sh
    permissions: '0700'
    owner: root:root
    content: |
      #!/bin/bash
      set -euo pipefail
      J=/var/log/horizon-spike-timing.jsonl
      stamp() { printf '{"event":"%s","at":"%s"}\n' "\$1" "\$(date -u +%FT%T.%3NZ)" >> "\$J"; }
      stamp bootstrap_start
      DISK=/dev/disk/azure/scsi1/lun0
      for _ in \$(seq 1 90); do [ -e "\$DISK" ] && break; sleep 1; done
      [ -e "\$DISK" ] || { stamp data_disk_missing; exit 1; }
      if [ "\$(blkid -o value -s TYPE "\$DISK" || true)" != ext4 ]; then mkfs.ext4 -q -L horizonws "\$DISK"; fi
      mkdir -p /mnt/horizon-workspace
      UUID=\$(blkid -o value -s UUID "\$DISK")
      grep -q "\$UUID" /etc/fstab || echo "UUID=\$UUID /mnt/horizon-workspace ext4 defaults,nofail 0 2" >> /etc/fstab
      mountpoint -q /mnt/horizon-workspace || mount /mnt/horizon-workspace
      chown root:root /mnt/horizon-workspace && chmod 0755 /mnt/horizon-workspace
      stamp data_disk_ready
      systemctl enable --now docker
      field() { python3 -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]])' "\$1"; }
      AAD=\$(curl -sf -H Metadata:true "http://169.254.169.254/metadata/identity/oauth2/token?api-version=2018-02-01&resource=https%3A%2F%2Fmanagement.azure.com%2F&client_id=${CLIENT_ID}" | field access_token)
      REFRESH=\$(curl -sf -X POST "https://${REGISTRY}/oauth2/exchange" --data-urlencode grant_type=access_token --data-urlencode service=${REGISTRY} --data-urlencode "access_token=\$AAD" | field refresh_token)
      printf '%s' "\$REFRESH" | docker login ${REGISTRY} -u 00000000-0000-0000-0000-000000000000 --password-stdin >/dev/null
      unset AAD REFRESH
      stamp registry_login
      docker pull -q ${IMAGE} >/dev/null
      stamp image_pulled
      docker create --name horizon-worker --restart unless-stopped -p 2222:22 \\
        --mount type=bind,src=/mnt/horizon-workspace,dst=/workspace \\
        -e "HORIZON_SSH_PUBLIC_KEY=${PUBKEY}" ${IMAGE} >/dev/null
      stamp container_created
      docker start horizon-worker >/dev/null
      stamp container_started
      docker logout ${REGISTRY} >/dev/null 2>&1 || true
runcmd:
  - [ bash, /usr/local/sbin/horizon-spike-bootstrap.sh ]
EOF

if [ "$DRY_RUN" = 1 ]; then
  say "dry run: would create $RG in $REGION with $VM_SIZE, image $IMAGE, cloud-init at $CLOUD_INIT"
  journal dry_run '{}'
  exit 0
fi

trap 'say "interrupted; cleaning up"; cleanup' INT TERM

azc resource list --query "sort([].id)" >"$SAMPLE_DIR/inventory-before.json"
journal inventory_before "$(jq -c '{count:length}' "$SAMPLE_DIR/inventory-before.json")"

T0=$(epoch_ms)
azc group create --name "$RG" --location "$REGION" \
  --tags issue=474 sample="$SAMPLE" run="$RUN_ID" purpose=horizon-azure-vm-spike deadline="$(date -u -d @"$DEADLINE_EPOCH" +%FT%TZ)" >/dev/null
journal group_created "$(jq -cn --argjson ms $(( $(epoch_ms) - T0 )) '{ms_from_t0:$ms}')"

CREATE_JSON="$SAMPLE_DIR/vm-create.json"
azc vm create --resource-group "$RG" --name "$VM" --location "$REGION" --size "$VM_SIZE" \
  --image Canonical:ubuntu-24_04-lts:server:latest --admin-username azureuser \
  --authentication-type ssh --ssh-key-values "$KEY.pub" \
  --public-ip-sku Standard --public-ip-address-allocation static --nsg-rule NONE \
  --assign-identity "$PULLER_ID" --os-disk-size-gb 30 --data-disk-sizes-gb 32 \
  --custom-data "$CLOUD_INIT" --tags issue=474 sample="$SAMPLE" run="$RUN_ID" >"$CREATE_JSON"
T_CREATE=$(epoch_ms)
IP=$(jq -r .publicIpAddress "$CREATE_JSON")
journal vm_created "$(jq -cn --argjson ms $((T_CREATE - T0)) --arg ip "$IP" '{ms_from_t0:$ms,public_ip:$ip}')"
say "vm created in $((T_CREATE - T0)) ms"

NSG=$(azc network nsg list --resource-group "$RG" --query "[0].name" -o tsv)
SOURCE=${ALLOW_SSH_FROM:-$(curl -s --max-time 10 https://ifconfig.me)/32}
azc network nsg rule create --resource-group "$RG" --nsg-name "$NSG" --name worker-ssh --priority 100 \
  --direction Inbound --access Allow --protocol Tcp --source-address-prefixes "$SOURCE" \
  --destination-port-ranges 2222 >/dev/null
journal nsg_rule "$(jq -cn --arg src "$SOURCE" '{source:$src,port:2222}')"

say "waiting for worker endpoint on $IP:2222"
until nc -z -w 3 "$IP" 2222 2>/dev/null; do deadline_check; sleep 3; done
T_PORT=$(epoch_ms)
journal endpoint_open "$(jq -cn --argjson ms $((T_PORT - T0)) '{ms_from_t0:$ms}')"
say "endpoint open in $((T_PORT - T0)) ms"

say "reading worker host key out of band through run-command"
RC_START=$(epoch_ms)
RC_JSON="$SAMPLE_DIR/run-command-hostkey.json"
azc vm run-command invoke --resource-group "$RG" --name "$VM" --command-id RunShellScript \
  --scripts 'cat /mnt/horizon-workspace/.horizon-worker/ssh/*.pub 2>/dev/null; echo ---TIMING---; cat /var/log/horizon-spike-timing.jsonl' >"$RC_JSON"
RC_END=$(epoch_ms)
RC_TEXT=$(jq -r '.value[0].message' "$RC_JSON")
HOST_KEY=$(printf '%s\n' "$RC_TEXT" | grep -m1 '^ssh-ed25519 ' || true)
printf '%s\n' "$RC_TEXT" | sed -n '/---TIMING---/,$p' | grep '^{' >"$SAMPLE_DIR/guest-timing.jsonl" || true
[ -n "$HOST_KEY" ] || { journal host_key_missing '{}'; say "no ed25519 host key published yet; aborting sample"; cleanup; exit 5; }
printf '[%s]:2222 %s\n' "$IP" "$HOST_KEY" >"$KNOWN_HOSTS"
journal host_key_pinned "$(jq -cn --argjson ms $((RC_END - RC_START)) --arg fp "$(ssh-keygen -lf "$KNOWN_HOSTS" | awk '{print $2}')" '{run_command_ms:$ms,fingerprint:$fp}')"

SSH=(ssh -p 2222 -i "$KEY" -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile="$KNOWN_HOSTS" -o PasswordAuthentication=no -o ConnectTimeout=10 -o BatchMode=yes "root@$IP")
until "${SSH[@]}" true 2>/dev/null; do deadline_check; sleep 3; done
T_SSH=$(epoch_ms)
journal ssh_verified "$(jq -cn --argjson ms $((T_SSH - T0)) --argjson excl $((T_SSH - T0 - (RC_END - RC_START))) '{ms_from_t0:$ms,ms_excluding_run_command:$excl}')"
say "key-only SSH verified in $((T_SSH - T0)) ms (excluding run-command: $((T_SSH - T0 - (RC_END - RC_START))) ms)"

say "collecting on-worker storage evidence"
"${SSH[@]}" 'set -e; echo "fs_type=$(stat -f -c %T /workspace)"; DEV=$(df --output=source /workspace | tail -1); echo "device=$DEV"; NAME=$(basename "$(readlink -f "$DEV")"); echo "kernel_options_path=/proc/fs/ext4/$NAME/options"; echo "---OPTIONS---"; cat /proc/fs/ext4/$NAME/options; echo "---MOUNT---"; grep " /workspace " /proc/self/mounts; echo "---SYSFS---"; readlink /sys/dev/block/$(stat -c %d /workspace | awk "{printf \"%d:%d\", int(\$1/256), \$1%256}") 2>&1 || true' >"$SAMPLE_DIR/storage-evidence.txt" 2>&1 || true
OPTIONS=$(sed -n '/---OPTIONS---/,/---MOUNT---/p' "$SAMPLE_DIR/storage-evidence.txt" | grep -v -- '---')
QUALIFIES=false
if grep -qx rw <<<"$OPTIONS" && grep -qx barrier <<<"$OPTIONS" && ! grep -qx ro <<<"$OPTIONS" && ! grep -qx nobarrier <<<"$OPTIONS" \
   && [ "$(grep -c '^data=' <<<"$OPTIONS")" = 1 ] && grep -qxE 'data=(ordered|journal)' <<<"$OPTIONS" && grep -q 'fs_type=ext2/ext3' "$SAMPLE_DIR/storage-evidence.txt"; then QUALIFIES=true; fi
journal storage_evidence "$(jq -cn --argjson q $QUALIFIES --arg opts "$OPTIONS" '{shell_mirror_of_qualifier_passes:$q,kernel_options:($opts|split("\n"))}')"
say "ext4 kernel options mirror check: $QUALIFIES (authoritative qualifier is the Rust code, not this script)"

say "detach independence: start a heartbeat, disconnect, reconnect"
"${SSH[@]}" 'tmux new-session -d -s spike "while true; do date -u +%FT%TZ >> /workspace/spike-heartbeat; sleep 2; done"; sleep 1; wc -l < /workspace/spike-heartbeat' >"$SAMPLE_DIR/heartbeat-before.txt"
sleep 20
"${SSH[@]}" 'tmux has-session -t spike && wc -l < /workspace/spike-heartbeat' >"$SAMPLE_DIR/heartbeat-after.txt"
journal detach_independence "$(jq -cn --arg b "$(cat "$SAMPLE_DIR/heartbeat-before.txt")" --arg a "$(cat "$SAMPLE_DIR/heartbeat-after.txt")" '{lines_before:($b|tonumber),lines_after:($a|tonumber),progressed:(($a|tonumber)>($b|tonumber))}')"

say "retention: marker, deallocate, start, verify"
MARKER="spike-$RUN_ID-$TOKEN"
"${SSH[@]}" "printf '%s\n' '$MARKER' > /workspace/spike-marker; sync"
deadline_check
T_STOP=$(epoch_ms)
azc vm deallocate --resource-group "$RG" --name "$VM" >/dev/null
POWER=$(azc vm get-instance-view --resource-group "$RG" --name "$VM" --query "instanceView.statuses[?starts_with(code,'PowerState/')].code | [0]" -o tsv)
T_STOPPED=$(epoch_ms)
journal deallocated "$(jq -cn --argjson ms $((T_STOPPED - T_STOP)) --arg power "$POWER" '{stop_ms:$ms,power_state:$power}')"
T_START=$(epoch_ms)
azc vm start --resource-group "$RG" --name "$VM" >/dev/null
IP_AFTER=$(azc vm show -d --resource-group "$RG" --name "$VM" --query publicIps -o tsv)
until nc -z -w 3 "$IP" 2222 2>/dev/null; do deadline_check; sleep 3; done
until "${SSH[@]}" true 2>/dev/null; do deadline_check; sleep 3; done
T_RESTARTED=$(epoch_ms)
RETAINED=$("${SSH[@]}" "cat /workspace/spike-marker 2>/dev/null; tmux has-session -t spike 2>/dev/null && echo session-alive || echo session-gone; wc -l < /workspace/spike-heartbeat")
journal restarted "$(jq -cn --argjson ms $((T_RESTARTED - T_START)) --arg ip_same "$([ "$IP" = "$IP_AFTER" ] && echo true || echo false)" --arg out "$RETAINED" --arg marker "$MARKER" '{start_to_ssh_ms:$ms,public_ip_unchanged:($ip_same=="true"),marker_retained:($out|split("\n")[0]==$marker),session_after_restart:($out|split("\n")[1]),heartbeat_lines:($out|split("\n")[2]|tonumber),same_pinned_host_key:true}')"
say "after deallocate/start: $(tr '\n' ' ' <<<"$RETAINED")"

cleanup
journal end "$(jq -cn --argjson total $(( $(epoch_ms) - T0 )) '{total_ms:$total}')"
say "sample $SAMPLE complete; journal at $JOURNAL"
