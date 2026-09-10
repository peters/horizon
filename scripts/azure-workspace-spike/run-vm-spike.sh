#!/usr/bin/env bash
# Bounded three-sample Azure Linux VM spike for issue #474.
#
# One exact task-owned resource group per sample. Measures create, image pull,
# endpoint, verified key-only SSH and delete timing; runs the worker's authoritative
# storage qualifier; records detach independence, deallocate/start data retention
# and exact deletion with an unchanged-inventory proof. Every timestamp is UTC and
# journaled privately. Nothing here registers a provider, publishes an image or
# touches resources outside the sample resource group. Run the read-only preflight first.
#
# Exit codes: 0 every gate held; 1 setup or provider failure before the gates (cleanup attempted);
# 3 usage; 4 lifetime bound reached (cleanup attempted); 5 worker never published a host key;
# 6 deletion not proven; 7 a functional gate failed (evidence journaled, resources deleted);
# 130 interrupted (cleanup attempted). Any other status is normalized to 1.
set -euo pipefail

usage() {
  cat <<'EOF'
usage: run-vm-spike.sh --subscription <uuid> --image <registry/repo@sha256:...> \
         --puller-identity-id <user-assigned-identity-resource-id> --journal-dir <private-dir> \
         --preflight-report <json from scripts/azure-workspace-preflight --candidate vm --live> \
         [--region northeurope] [--vm-size Standard_D4s_v3] [--sample 1] [--max-minutes 120] \
         [--allow-ssh-from <cidr>] [--keep] [--dry-run]

Requires: az (logged in), ssh, ssh-keygen, ssh-keyscan, nc, jq, curl. Paid creation is
bounded by --max-minutes; the sample resource group is deleted at the end unless --keep.
EOF
}

SUBSCRIPTION="" IMAGE="" PULLER_ID="" JOURNAL_DIR="" PREFLIGHT="" REGION=northeurope VM_SIZE=Standard_D4s_v3
SAMPLE=1 MAX_MINUTES=120 ALLOW_SSH_FROM="" KEEP=0 DRY_RUN=0
while [ $# -gt 0 ]; do
  case "$1" in
    --subscription|--image|--puller-identity-id|--journal-dir|--preflight-report|--region|--vm-size|--sample|--max-minutes|--allow-ssh-from)
      [ $# -ge 2 ] || { echo "$1 requires a value" >&2; usage; exit 3; } ;;
  esac
  case "$1" in
    --subscription) SUBSCRIPTION=$2; shift 2 ;;
    --image) IMAGE=$2; shift 2 ;;
    --puller-identity-id) PULLER_ID=$2; shift 2 ;;
    --journal-dir) JOURNAL_DIR=$2; shift 2 ;;
    --preflight-report) PREFLIGHT=$2; shift 2 ;;
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
[[ $SAMPLE =~ ^[1-9][0-9]?$ ]] || { echo "--sample must be 1-99 without leading zeros" >&2; exit 3; }
[[ $MAX_MINUTES =~ ^[1-9][0-9]{1,2}$ ]] || { echo "--max-minutes must be 10-999 without leading zeros" >&2; exit 3; }
[ -n "$JOURNAL_DIR" ] || { echo "--journal-dir is required" >&2; exit 3; }
for tool in az ssh ssh-keygen ssh-keyscan nc jq curl timeout; do command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 3; }; done
# The read-only preflight must precede paid creation: require its report for this exact candidate,
# region and size, with no blockers, and carry its provenance into the sample journal.
if [ "$DRY_RUN" = 0 ] || [ -n "$PREFLIGHT" ]; then
  [ -n "$PREFLIGHT" ] && [ -r "$PREFLIGHT" ] || { echo "--preflight-report must point at a readable preflight JSON report" >&2; exit 3; }
  # The report carries a stable digest of its subscription instead of the identifier itself.
  SUB_DIGEST=$(printf 'horizon-preflight:%s' "$SUBSCRIPTION" | sha256sum | cut -c1-16)
  PREFLIGHT_FACTS=$(jq -c --arg region "$REGION" --arg size "$VM_SIZE" --arg digest "$SUB_DIGEST" '
    if .schema == "horizon.azure-workspace-preflight.report" and .candidate.id == "vm" and .region == $region
       and .candidate.vm_size == $size and .mode == "live" and .status == "no_blockers_observed"
       and .subscription_digest == $digest
    then {observed_at, tool_version, status, schema_version, subscription_digest} else empty end' "$PREFLIGHT" 2>/dev/null || true)
  [ -n "$PREFLIGHT_FACTS" ] || { echo "--preflight-report is not a live vm preflight for this subscription, $REGION and $VM_SIZE with no blockers observed" >&2; exit 3; }
fi

REGISTRY=${IMAGE%%/*}
RUN_ID=$(date -u +%Y%m%dT%H%M%SZ)
TOKEN=$(head -c 3 /dev/urandom | od -An -tx1 | tr -d ' \n')
RG="horizon-spike-474-s${SAMPLE}-${TOKEN}"
VM="worker-s${SAMPLE}"
DEADLINE_EPOCH=$(( $(date +%s) + MAX_MINUTES * 60 ))
RESTART_WAIT_SECONDS=600
umask 077
mkdir -p "$JOURNAL_DIR"
SAMPLE_DIR="$JOURNAL_DIR/sample-${SAMPLE}-${RUN_ID}"
mkdir "$SAMPLE_DIR"
JOURNAL="$SAMPLE_DIR/journal.jsonl"
KEY="$SAMPLE_DIR/client-ed25519"
KNOWN_HOSTS="$SAMPLE_DIR/known_hosts"
GATE_FAILURES=()
CREATED=0 CLEANED=0 DELETE_PROVEN=0 CLEANUP_DEADLINE=0 PHASE_DEADLINE=0

# Every blocking call runs under the remaining lifetime bound; cleanup shares one fixed 25-minute deadline.
remaining_seconds() {
  local now limit; now=$(date +%s)
  if [ "$CLEANED" = 1 ]; then echo $(( CLEANUP_DEADLINE - now )); return; fi
  limit=$DEADLINE_EPOCH
  [ "$PHASE_DEADLINE" -gt 0 ] && [ "$PHASE_DEADLINE" -lt "$limit" ] && limit=$PHASE_DEADLINE
  echo $(( limit - now ))
}
bounded() {
  local remaining rc; remaining=$(remaining_seconds)
  if [ "$remaining" -lt 1 ]; then say "bound reached before: ${*:1:3}"; return 124; fi
  timeout -k 15 "$remaining" "$@" && return 0 || rc=$?
  [ "$rc" != 124 ] || say "bound reached during: ${*:1:3}"
  return "$rc"
}
# Machine-consumed output is always JSON unless the caller asks for tsv explicitly.
azc() {
  case " $* " in *" -o "*|*" --output "*) ;; *) set -- "$@" --output json ;; esac
  bounded az "$@" --subscription "$SUBSCRIPTION" --only-show-errors
}
now() { date -u +%Y-%m-%dT%H:%M:%S.%3NZ; }
epoch_ms() { date +%s%3N; }
journal() { local data=${2:-'{}'}; jq -cn --arg at "$(now)" --arg event "$1" --argjson data "$data" '{at:$at,event:$event,data:$data}' >>"$JOURNAL"; }
say() { printf '[%s] %s\n' "$(now)" "$*"; }
gate() { local name=$1 ok=$2; [ "$ok" = true ] || GATE_FAILURES+=("$name"); }
deadline_check() { [ "$(date +%s)" -lt "$DEADLINE_EPOCH" ] || { say "lifetime bound reached; forcing cleanup"; exit 4; }; }
# wait_for <max-seconds> <command...>: poll every 3 s under the lifetime bound; returns 1 on the phase bound.
wait_for() {
  local until_epoch=$(( $(date +%s) + ( $1 > 0 ? $1 : 0 ) )); shift
  until "$@" >/dev/null 2>&1; do
    deadline_check
    [ "$(date +%s)" -lt "$until_epoch" ] || return 1
    bounded sleep 3 || return 1
  done
}
# run_command <script>: ARM-authenticated shell on the VM; retries while the extension is still installing.
run_command() {
  local attempt output
  for attempt in $(seq 1 12); do
    if output=$(azc vm run-command invoke --resource-group "$RG" --name "$VM" --command-id RunShellScript --scripts "$1" --query 'value[0].message' -o tsv 2>&1); then
      printf '%s\n' "$output"; return 0
    fi
    grep -qi "in progress\|Conflict\|please wait" <<<"$output" || { printf '%s\n' "$output" >&2; return 1; }
    say "run-command busy (attempt $attempt); retrying"; bounded sleep 10 || return 1
  done
  return 1
}

cleanup() {
  [ "$CLEANED" = 0 ] || return 0
  CLEANED=1
  CLEANUP_DEADLINE=$(( $(date +%s) + 1500 ))
  [ "$CREATED" = 1 ] || return 0
  if [ "$KEEP" = 1 ]; then say "--keep given; leaving $RG in place (delete it yourself)"; journal kept "$(jq -cn --arg rg "$RG" '{resource_group:$rg}')"; return 0; fi
  local started finished exists=unknown unchanged=false
  started=$(epoch_ms)
  say "deleting resource group $RG"
  if azc group delete --name "$RG" --yes --no-wait >/dev/null 2>&1 && azc group wait --name "$RG" --deleted --timeout 1200 >/dev/null 2>&1; then
    exists=$(azc group exists --name "$RG" -o tsv 2>/dev/null || echo unknown)
  fi
  finished=$(epoch_ms)
  azc resource list --query "sort([].id)" >"$SAMPLE_DIR/inventory-after.json" 2>/dev/null || echo 'null' >"$SAMPLE_DIR/inventory-after.json"
  # Proof (#474: exact absence and unchanged pre-existing resources): nothing remains under the
  # sample group and no pre-existing resource disappeared. A disappearance outside the group is
  # most likely another actor in the shared subscription, but the inventory cannot attribute it,
  # so the proof is reported as unverified and the sample must be rerun. Additions are counted.
  local verdict
  verdict=$(jq -cn --arg rg "/resourceGroups/$RG/" --slurpfile before "$SAMPLE_DIR/inventory-before.json" --slurpfile after "$SAMPLE_DIR/inventory-after.json" \
    '($before[0] // null) as $b | ($after[0] // null) as $a
     | if ($b|type) != "array" or ($a|type) != "array" then {proven:false,reason:"inventory_unavailable"}
       else {leftover:[$a[] | select(ascii_downcase | contains($rg|ascii_downcase))],
             removed_outside_group:($b - $a), added_outside_group:(($a - $b)|length)}
            | .proven = ((.leftover|length)==0 and (.removed_outside_group|length)==0)
            | if .proven then . else .reason = (if (.leftover|length)>0 then "resources_left_under_group" else "unverified_concurrent_removal_outside_group" end) end end')
  [ "$(jq -r .proven <<<"$verdict")" = true ] && unchanged=true
  [ "$exists" = false ] && [ "$unchanged" = true ] && DELETE_PROVEN=1
  journal deleted "$(jq -cn --arg exists "$exists" --argjson ms $((finished - started)) --argjson v "$verdict" '{group_exists:$exists,delete_ms:$ms,inventory_proof:$v}')"
  say "resource group exists=$exists inventory_proof=$(jq -c '{proven,reason,leftover:(.leftover|length),removed_outside_group:(.removed_outside_group|length),added_outside_group}' <<<"$verdict") delete_ms=$((finished - started))"
}
finish() {
  local code=$? expired=0
  trap - EXIT
  trap '' INT TERM  # a second interrupt must not cut the cleanup attempt short
  [ "$(date +%s)" -lt "$DEADLINE_EPOCH" ] || expired=1  # decided before cleanup spends its own bound
  cleanup
  if [ "$code" = 0 ] && [ "${#GATE_FAILURES[@]}" -gt 0 ]; then code=7; fi
  if [ "$code" != 0 ] && [ "$expired" = 1 ]; then code=4; fi  # any failure past the active-phase bound is a bound expiry
  case "$code" in 0|3|4|5|6|7|130) ;; *) code=1 ;; esac
  if [ "$CREATED" = 1 ] && [ "$KEEP" = 0 ] && [ "$DELETE_PROVEN" = 0 ]; then code=6; fi  # an unproven delete outranks every other outcome
  journal end "$(jq -cn --argjson code "$code" --arg gates "${GATE_FAILURES[*]:-}" '{exit_code:$code,failed_gates:($gates|split(" ")|map(select(length>0)))}')"
  say "sample $SAMPLE finished with exit $code${GATE_FAILURES[*]:+ (failed gates: ${GATE_FAILURES[*]})}; journal $JOURNAL"
  exit "$code"
}

say "sample $SAMPLE run $RUN_ID journal $SAMPLE_DIR"
HARNESS_COMMIT=$(git -C "$(dirname "$0")" rev-parse HEAD 2>/dev/null || echo unknown)
journal start "$(jq -cn --arg region "$REGION" --arg size "$VM_SIZE" --arg image "$IMAGE" --arg rg "$RG" --argjson max "$MAX_MINUTES" --arg commit "$HARNESS_COMMIT" --arg pf "${PREFLIGHT:-}" --argjson facts "${PREFLIGHT_FACTS:-null}" '{region:$region,vm_size:$size,image:$image,resource_group:$rg,max_minutes:$max,harness_commit:$commit,preflight_report:$pf,preflight:$facts}')"

ssh-keygen -q -t ed25519 -N "" -C "horizon-spike-474-s${SAMPLE}" -f "$KEY"
PUBKEY=$(cat "$KEY.pub")
if [ "$DRY_RUN" = 1 ]; then CLIENT_ID=00000000-0000-0000-0000-000000000000; else
  trap finish EXIT
  trap 'exit 130' INT TERM
  CLIENT_ID=$(azc identity show --ids "$PULLER_ID" --query clientId -o tsv)
fi

CLOUD_INIT="$SAMPLE_DIR/cloud-init.yaml"
cat >"$CLOUD_INIT" <<EOF
#cloud-config
package_update: true
packages: [docker.io]
bootcmd:
  - [ sh, -c, 'printf "{\"event\":\"boot\",\"at\":\"%s\"}\n" "\$(date -u +%FT%T.%3NZ)" >> /var/log/horizon-spike-timing.jsonl' ]
write_files:
  - path: /etc/systemd/system/docker.service.d/horizon-workspace.conf
    permissions: '0644'
    owner: root:root
    content: |
      [Unit]
      RequiresMountsFor=/mnt/horizon-workspace
  - path: /usr/local/sbin/horizon-spike-bootstrap.sh
    permissions: '0700'
    owner: root:root
    content: |
      #!/bin/bash
      set -euo pipefail
      J=/var/log/horizon-spike-timing.jsonl
      stamp() { printf '{"event":"%s","at":"%s"}\n' "\$1" "\$(date -u +%FT%T.%3NZ)" >> "\$J"; }
      field() { python3 -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]])' "\$1"; }
      stamp bootstrap_start
      DISK=/dev/disk/azure/scsi1/lun0
      for _ in \$(seq 1 90); do [ -e "\$DISK" ] && break; sleep 1; done
      [ -e "\$DISK" ] || { stamp data_disk_missing; exit 1; }
      if [ "\$(blkid -o value -s TYPE "\$DISK" || true)" != ext4 ]; then mkfs.ext4 -q -L horizonws "\$DISK"; fi
      mkdir -p /mnt/horizon-workspace
      UUID=\$(blkid -o value -s UUID "\$DISK")
      grep -q "\$UUID" /etc/fstab || echo "UUID=\$UUID /mnt/horizon-workspace ext4 defaults,nofail 0 2" >> /etc/fstab
      systemctl daemon-reload
      mountpoint -q /mnt/horizon-workspace || mount /mnt/horizon-workspace
      chown root:root /mnt/horizon-workspace && chmod 0755 /mnt/horizon-workspace
      stamp data_disk_ready
      systemctl enable --now docker
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

# finish() already covers every bounded call; nothing paid exists until CREATED=1.
azc resource list --query "sort([].id)" >"$SAMPLE_DIR/inventory-before.json"
journal inventory_before "$(jq -c '{count:length}' "$SAMPLE_DIR/inventory-before.json")"

# Read-only provider preflight for the auto-shutdown schedule; registration is a separate approval.
DEVTESTLAB=$(azc provider show --namespace Microsoft.DevTestLab --query registrationState -o tsv 2>/dev/null || echo unknown)
[ "$DEVTESTLAB" = Registered ] || { journal devtestlab_not_registered "$(jq -cn --arg s "$DEVTESTLAB" '{registration_state:$s}')"; say "Microsoft.DevTestLab is $DEVTESTLAB; the platform-side stop cannot be scheduled, so nothing is created"; exit 1; }
EXISTS=$(azc group exists --name "$RG" -o tsv 2>/dev/null || echo unknown)
[ "$EXISTS" = false ] || { journal group_name_not_free "$(jq -cn --arg e "$EXISTS" '{group_exists:$e}')"; say "refusing to claim $RG (exists=$EXISTS)"; exit 1; }
T0=$(epoch_ms)
CREATED=1  # cleanup owns the verified-absent group from this point, even if the create response is lost
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
[[ $IP =~ ^[0-9.]+$ ]] || { journal vm_create_no_ip '{}'; say "vm create returned no public IP"; exit 1; }
journal vm_created "$(jq -cn --argjson ms $((T_CREATE - T0)) --arg ip "$IP" '{ms_from_t0:$ms,public_ip:$ip}')"
say "vm created in $((T_CREATE - T0)) ms"
# Independent cloud-side stop: Azure deallocates the VM at the lifetime deadline even if this
# controller dies. Disks and the public IP remain until the tagged group is deleted by hand.
SHUTDOWN_AT=$(date -u -d @"$(( DEADLINE_EPOCH + 60 ))" +%H%M)
if azc vm auto-shutdown --resource-group "$RG" --name "$VM" --time "$SHUTDOWN_AT" >/dev/null 2>&1; then
  journal auto_shutdown_scheduled "$(jq -cn --arg at "$SHUTDOWN_AT" '{utc_hhmm:$at}')"
else
  # Without the platform-side bound the sample may not continue and may not be kept.
  journal auto_shutdown_not_scheduled '{}'; say "fatal: cloud-side auto-shutdown could not be scheduled; deleting the sample"
  KEEP=0; exit 1
fi

NSG=$(azc network nsg list --resource-group "$RG" --query "[0].name" -o tsv)
SOURCE=${ALLOW_SSH_FROM:-}
if [ -z "$SOURCE" ]; then
  EGRESS=$(bounded curl -s --max-time 10 https://ifconfig.me || true)
  [[ $EGRESS =~ ^[0-9.]+$ ]] || { journal egress_unknown '{}'; say "could not learn the egress address; pass --allow-ssh-from"; exit 1; }
  SOURCE="$EGRESS/32"
fi
azc network nsg rule create --resource-group "$RG" --nsg-name "$NSG" --name worker-ssh --priority 100 \
  --direction Inbound --access Allow --protocol Tcp --source-address-prefixes "$SOURCE" \
  --destination-port-ranges 2222 >/dev/null
journal nsg_rule "$(jq -cn --arg src "$SOURCE" '{source:$src,port:2222}')"

say "waiting for worker endpoint on port 2222"
wait_for $(( DEADLINE_EPOCH - $(date +%s) )) bounded nc -z -w 3 "$IP" 2222 || { journal endpoint_never_opened '{}'; exit 4; }
T_PORT=$(epoch_ms)
journal endpoint_open "$(jq -cn --argjson ms $((T_PORT - T0)) '{ms_from_t0:$ms}')"
say "endpoint open in $((T_PORT - T0)) ms"

say "reading worker host key out of band through run-command"
RC_START=$(epoch_ms)
RC_OK=true
RC_TEXT=$(run_command 'cat /mnt/horizon-workspace/.horizon-worker/ssh/*.pub 2>/dev/null; echo ---TIMING---; cat /var/log/horizon-spike-timing.jsonl' 2>&1) || RC_OK=false
RC_END=$(epoch_ms)
printf '%s\n' "$RC_TEXT" >"$SAMPLE_DIR/run-command-hostkey.txt"
[ "$RC_OK" = true ] || { journal run_command_failed "$(jq -cn --argjson ms $((RC_END - RC_START)) '{run_command_ms:$ms}')"; say "run-command failed; see run-command-hostkey.txt"; exit 1; }
HOST_KEY=$(printf '%s\n' "$RC_TEXT" | grep -m1 '^ssh-ed25519 ' | awk '{print $1" "$2}' || true)
printf '%s\n' "$RC_TEXT" | sed -n '/---TIMING---/,$p' | grep '^{' >"$SAMPLE_DIR/guest-timing.jsonl" || true
[ -n "$HOST_KEY" ] || { journal host_key_missing "$(jq -cn --argjson ms $((RC_END - RC_START)) '{run_command_ms:$ms}')"; say "no ed25519 host key published yet; aborting sample"; exit 5; }
printf '[%s]:2222 %s\n' "$IP" "$HOST_KEY" >"$KNOWN_HOSTS"
journal host_key_pinned "$(jq -cn --argjson ms $((RC_END - RC_START)) --arg fp "$(ssh-keygen -lf "$KNOWN_HOSTS" | awk '{print $2}')" '{run_command_ms:$ms,fingerprint:$fp}')"

ssh_worker() { bounded ssh -T -F /dev/null -p 2222 -i "$KEY" -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile="$KNOWN_HOSTS" \
  -o PasswordAuthentication=no -o KbdInteractiveAuthentication=no -o ConnectTimeout=10 -o BatchMode=yes "root@$1" "${@:2}"; }
wait_for $(( DEADLINE_EPOCH - $(date +%s) )) ssh_worker "$IP" true || { journal ssh_never_verified '{}'; exit 4; }
T_SSH=$(epoch_ms)
journal ssh_verified "$(jq -cn --argjson ms $((T_SSH - T0)) --argjson excl $((T_SSH - T0 - (RC_END - RC_START))) '{ms_from_t0:$ms,ms_excluding_run_command:$excl}')"
say "key-only SSH verified in $((T_SSH - T0)) ms (excluding run-command: $((T_SSH - T0 - (RC_END - RC_START))) ms)"

say "collecting on-worker storage evidence and running the authoritative qualifier"
ssh_worker "$IP" 'set -e; echo "fs_type=$(stat -f -c %T /workspace)"; DEV=$(df --output=source /workspace | tail -1); echo "device=$DEV"; NAME=$(basename "$(readlink -f "$DEV")"); echo "---OPTIONS---"; cat "/proc/fs/ext4/$NAME/options"; echo "---MOUNT---"; grep " /workspace " /proc/self/mounts' >"$SAMPLE_DIR/storage-evidence.txt" 2>&1 || true
OPTIONS=$(sed -n '/---OPTIONS---/,/---MOUNT---/p' "$SAMPLE_DIR/storage-evidence.txt" | grep -v -- '---' || true)
MIRROR=false
if [ -n "$OPTIONS" ] && grep -qx rw <<<"$OPTIONS" && grep -qx barrier <<<"$OPTIONS" && ! grep -qx ro <<<"$OPTIONS" && ! grep -qx nobarrier <<<"$OPTIONS" \
   && [ "$(grep -c '^data=' <<<"$OPTIONS")" = 1 ] && grep -qxE 'data=(ordered|journal)' <<<"$OPTIONS" && grep -q 'fs_type=ext2/ext3' "$SAMPLE_DIR/storage-evidence.txt"; then MIRROR=true; fi
# The image's horizon-repository binary runs storage::qualify read-only on the retained root:
# "absent" means qualified storage with no claim; an overlay root is the negative control.
QUALIFIER_REQUEST='{"version":1,"retained_root":"%s","workspace_local_id":"workspace_1","objects_directory":"/workspace/spike-input/objects","bundle_store":"/workspace/spike-input/bundles","bundle_manifest":"0000000000000000000000000000000000000000000000000000000000000000","destination":"repository"}'
QUALIFIER_OUT=$(ssh_worker "$IP" "set -e; mkdir -p -m 0755 /workspace/spike-input/objects /workspace/spike-input/bundles; mkdir -p -m 0700 /workspace/spike-retained /root/spike-overlay; printf '$QUALIFIER_REQUEST' /workspace/spike-retained | /usr/local/bin/horizon-repository setup-status; echo; echo ---OVERLAY---; printf '$QUALIFIER_REQUEST' /root/spike-overlay | /usr/local/bin/horizon-repository setup-status || true; echo" 2>&1 || true)
printf '%s\n' "$QUALIFIER_OUT" >"$SAMPLE_DIR/qualifier-output.txt"
DATA_STATUS=$(printf '%s\n' "$QUALIFIER_OUT" | sed '/---OVERLAY---/,$d' | jq -r '.status // empty' 2>/dev/null | head -1 || true)
OVERLAY_STATUS=$(printf '%s\n' "$QUALIFIER_OUT" | sed -n '/---OVERLAY---/,$p' | grep -v -- '---' | jq -r '"\(.status // "none"):\(.reason // "")"' 2>/dev/null | head -1 || true)
QUALIFIED=false; [ "$DATA_STATUS" = absent ] && QUALIFIED=true
# Only the storage discriminator's own refusal counts; a decoding "rejected" or another error does not.
CONTROL_REJECTED=false; [ "$OVERLAY_STATUS" = "error:retained setup requires supported journaled storage and confinement" ] && CONTROL_REJECTED=true
journal storage_evidence "$(jq -cn --argjson mirror $MIRROR --argjson q $QUALIFIED --argjson c $CONTROL_REJECTED --arg ds "${DATA_STATUS:-none}" --arg os "${OVERLAY_STATUS:-none}" --arg opts "$OPTIONS" '{shell_mirror_passes:$mirror,rust_qualifier_status_on_data_disk:$ds,rust_qualifier_status_on_overlay:$os,rust_qualifier_accepts_data_disk:$q,overlay_control_rejected:$c,kernel_options:($opts|split("\n")|map(select(length>0)))}')"
gate storage_qualifier "$QUALIFIED"
gate storage_negative_control "$CONTROL_REJECTED"
say "rust qualifier on data disk: ${DATA_STATUS:-none} (overlay control: ${OVERLAY_STATUS:-none}); shell mirror: $MIRROR"

say "detach independence: start a heartbeat, disconnect, reconnect"
BEFORE=$(ssh_worker "$IP" 'tmux new-session -d -s spike "while true; do date -u +%FT%TZ >> /workspace/spike-heartbeat; sleep 2; done"; sleep 1; wc -l < /workspace/spike-heartbeat' 2>/dev/null || echo 0)
bounded sleep 20 || true
AFTER=$(ssh_worker "$IP" 'tmux has-session -t spike 2>/dev/null && wc -l < /workspace/spike-heartbeat' 2>/dev/null || echo 0)
PROGRESSED=false; [ "${AFTER:-0}" -gt "${BEFORE:-0}" ] 2>/dev/null && PROGRESSED=true
journal detach_independence "$(jq -cn --argjson b "${BEFORE:-0}" --argjson a "${AFTER:-0}" --argjson p $PROGRESSED '{lines_before:$b,lines_after:$a,progressed:$p}')"
gate detach_independence "$PROGRESSED"

say "retention: marker, deallocate, start, verify"
MARKER="spike-$RUN_ID-$TOKEN"
ssh_worker "$IP" "printf '%s\n' '$MARKER' > /workspace/spike-marker; sync" || true
deadline_check
T_STOP=$(epoch_ms)
STOP_OK=true; azc vm deallocate --resource-group "$RG" --name "$VM" >/dev/null || STOP_OK=false
T_STOPPED=$(epoch_ms)
POWER=$(azc vm get-instance-view --resource-group "$RG" --name "$VM" --query "instanceView.statuses[?starts_with(code,'PowerState/')].code | [0]" -o tsv 2>/dev/null || echo unknown)
journal deallocated "$(jq -cn --argjson ms $((T_STOPPED - T_STOP)) --argjson ok $STOP_OK --arg power "$POWER" '{stop_ms:$ms,call_succeeded:$ok,power_state:$power}')"
gate deallocated "$([ "$STOP_OK" = true ] && [ "$POWER" = PowerState/deallocated ] && echo true || echo false)"
T_START=$(epoch_ms)
# The whole restart phase (start call, lookup, both polls, evidence) shares one 10-minute bound.
RESTART_DEADLINE=$(( $(date +%s) + RESTART_WAIT_SECONDS )); PHASE_DEADLINE=$RESTART_DEADLINE
START_OK=true; azc vm start --resource-group "$RG" --name "$VM" >/dev/null || START_OK=false
IP_AFTER=$(azc vm show -d --resource-group "$RG" --name "$VM" --query publicIps -o tsv 2>/dev/null || echo unknown)
IP_SAME=false; [ "$IP" = "$IP_AFTER" ] && IP_SAME=true
KEY_SAME=false SESSION=unknown RETAINED_MARKER=false HEARTBEAT=0 MOUNTED=false
if [ "$START_OK" = true ] && [ "$IP_SAME" = true ] && wait_for $(( RESTART_DEADLINE - $(date +%s) )) bounded nc -z -w 3 "$IP_AFTER" 2222; then
  SCANNED=$(bounded ssh-keyscan -p 2222 -t ed25519 -T 10 "$IP_AFTER" 2>/dev/null | awk '{print $2" "$3}' | head -1 || true)
  [ "$SCANNED" = "$HOST_KEY" ] && KEY_SAME=true
  if [ "$KEY_SAME" = true ] && wait_for $(( RESTART_DEADLINE - $(date +%s) )) ssh_worker "$IP_AFTER" true; then
    OUT=$(ssh_worker "$IP_AFTER" "cat /workspace/spike-marker 2>/dev/null || echo missing; tmux has-session -t spike 2>/dev/null && echo session-alive || echo session-gone; wc -l < /workspace/spike-heartbeat 2>/dev/null || echo 0; grep -q ' /workspace ' /proc/self/mounts && echo mounted || echo unmounted" 2>/dev/null || printf 'missing\nunknown\n0\nunknown\n')
    [ "$(sed -n 1p <<<"$OUT")" = "$MARKER" ] && RETAINED_MARKER=true
    SESSION=$(sed -n 2p <<<"$OUT"); HEARTBEAT=$(sed -n 3p <<<"$OUT"); [ "$(sed -n 4p <<<"$OUT")" = mounted ] && MOUNTED=true
  fi
fi
T_RESTARTED=$(epoch_ms)
PHASE_DEADLINE=0
if [ "$KEY_SAME" != true ] || [ "$RETAINED_MARKER" != true ] || [ "$MOUNTED" != true ]; then
  say "restart verification failed; capturing guest diagnostics out of band"
  run_command 'systemctl is-system-running; systemctl list-units --failed --no-legend; systemctl status docker --no-pager | head -20; findmnt /mnt/horizon-workspace; docker ps -a; docker logs --tail 20 horizon-worker 2>&1; journalctl -b -u docker --no-pager | tail -20' >"$SAMPLE_DIR/restart-diagnostics.txt" 2>&1 || true
fi
journal restarted "$(jq -cn --argjson ms $((T_RESTARTED - T_START)) --argjson started $START_OK --argjson ip $IP_SAME --argjson key $KEY_SAME --argjson marker $RETAINED_MARKER --arg session "$SESSION" --argjson hb "${HEARTBEAT:-0}" --argjson mounted $MOUNTED '{start_to_verified_ms:$ms,start_call_succeeded:$started,public_ip_unchanged:$ip,same_pinned_host_key:$key,marker_retained:$marker,session_after_restart:$session,heartbeat_lines:$hb,data_disk_mounted:$mounted}')"
gate retention "$([ "$KEY_SAME" = true ] && [ "$RETAINED_MARKER" = true ] && [ "$MOUNTED" = true ] && echo true || echo false)"
say "after deallocate/start: ip_unchanged=$IP_SAME host_key_same=$KEY_SAME marker_retained=$RETAINED_MARKER mounted=$MOUNTED session=$SESSION"
journal timings_complete "$(jq -cn --argjson total $(( $(epoch_ms) - T0 )) '{ms_from_t0:$total}')"
