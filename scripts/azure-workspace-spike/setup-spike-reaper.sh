#!/usr/bin/env bash
# One-time, idempotent setup of the subscription-side lifetime reaper for the Azure VM spike.
#
# An Azure Automation runbook runs every 15 minutes with the account's system identity and
# deallocates every VM tagged purpose=horizon-azure-vm-spike whose deadline tag (RFC 3339 UTC)
# has passed. It exists before any spike VM does, and a spike VM cannot exist without those tags
# (they are part of the same create call), so compute is bounded even if the controller that
# created it never runs again. It touches nothing untagged and never deletes anything.
#
# Requires: az logged in with rights to create an Automation account and a subscription-scoped
# role assignment (the built-in power-only role "Desktop Virtualization Power On Off Contributor").
set -euo pipefail

usage() { echo "usage: setup-spike-reaper.sh --subscription <uuid> [--region northeurope] [--resource-group horizon-worker-registry] [--account horizon-spike-reaper]" >&2; }
SUBSCRIPTION="" REGION=northeurope RG=horizon-worker-registry ACCOUNT=horizon-spike-reaper
while [ $# -gt 0 ]; do
  case "$1" in
    --subscription|--region|--resource-group|--account) [ $# -ge 2 ] || { usage; exit 3; } ;;
  esac
  case "$1" in
    --subscription) SUBSCRIPTION=$2; shift 2 ;;
    --region) REGION=$2; shift 2 ;;
    --resource-group) RG=$2; shift 2 ;;
    --account) ACCOUNT=$2; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage; exit 3 ;;
  esac
done
[[ $SUBSCRIPTION =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]] || { usage; exit 3; }
[[ $REGION =~ ^[a-z][a-z0-9]{1,63}$ ]] && [[ $RG =~ ^[A-Za-z0-9._-]{1,90}$ ]] && [[ $ACCOUNT =~ ^[A-Za-z][A-Za-z0-9-]{4,48}$ ]] || { usage; exit 3; }
for tool in az jq python3; do command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 3; }; done

ARM=https://management.azure.com
BASE="$ARM/subscriptions/$SUBSCRIPTION/resourceGroups/$RG/providers/Microsoft.Automation/automationAccounts/$ACCOUNT"
API=2023-11-01
azr() { az rest --subscription "$SUBSCRIPTION" --only-show-errors "$@"; }
say() { printf '[%s] %s\n' "$(date -u +%FT%TZ)" "$*"; }

az group show --name "$RG" --subscription "$SUBSCRIPTION" --only-show-errors -o none 2>/dev/null \
  || az group create --name "$RG" --location "$REGION" --tags purpose=horizon-worker-images issue=474 --subscription "$SUBSCRIPTION" --only-show-errors -o none

say "automation account $ACCOUNT (system identity, Basic)"
azr --method put --url "$BASE?api-version=$API" --body "$(jq -cn --arg loc "$REGION" '{location:$loc,identity:{type:"SystemAssigned"},tags:{purpose:"horizon-azure-vm-spike-reaper",issue:"474"},properties:{sku:{name:"Basic"},publicNetworkAccess:true}}')" >/dev/null
PRINCIPAL=$(azr --method get --url "$BASE?api-version=$API" --query identity.principalId -o tsv)

say "subscription-scoped power-only role for the reaper identity"
ROLE="Desktop Virtualization Power On Off Contributor"
if [ "$(az role assignment list --assignee "$PRINCIPAL" --role "$ROLE" --scope "/subscriptions/$SUBSCRIPTION" --subscription "$SUBSCRIPTION" --only-show-errors --query "length(@)" -o tsv 2>/dev/null || echo 0)" != 0 ]; then
  say "role assignment already present (kept)"
else
  for attempt in $(seq 1 6); do
    if az role assignment create --assignee-object-id "$PRINCIPAL" --assignee-principal-type ServicePrincipal \
         --role "$ROLE" --scope "/subscriptions/$SUBSCRIPTION" --subscription "$SUBSCRIPTION" --only-show-errors -o none 2>/dev/null; then break; fi
    [ "$attempt" -lt 6 ] || { echo "role assignment failed" >&2; exit 1; }
    sleep 10  # the new identity may not have replicated yet
  done
fi

RUNBOOK=horizon-spike-deadline-reaper
say "runbook $RUNBOOK (PowerShell 7.2)"
azr --method put --url "$BASE/runbooks/$RUNBOOK?api-version=$API" --body "$(jq -cn --arg loc "$REGION" '{location:$loc,properties:{runbookType:"PowerShell72",logProgress:false,logVerbose:false,description:"Deallocate horizon-azure-vm-spike VMs whose deadline tag has passed."}}')" >/dev/null
SCRIPT=$(cat "$(dirname "$0")/reaper-runbook.ps1")
azr --method put --url "$BASE/runbooks/$RUNBOOK/draft/content?api-version=$API" --headers "Content-Type=text/powershell" --body "$SCRIPT" >/dev/null
azr --method post --url "$BASE/runbooks/$RUNBOOK/publish?api-version=$API" >/dev/null
STATE=""
for _ in $(seq 1 24); do
  STATE=$(azr --method get --url "$BASE/runbooks/$RUNBOOK?api-version=$API" --query properties.state -o tsv)
  [ "$STATE" = Published ] && break
  sleep 5
done
[ "$STATE" = Published ] || { echo "runbook did not reach Published (state: $STATE)" >&2; exit 1; }
PUBLISHED=$(azr --method get --url "$BASE/runbooks/$RUNBOOK/content?api-version=$API" 2>/dev/null || true)
[ "$(printf '%s' "$PUBLISHED" | tr -d '\r')" = "$(printf '%s' "$SCRIPT")" ] || { echo "published runbook content does not match this script" >&2; exit 1; }

SCHEDULE=every-15-minutes
say "schedule $SCHEDULE and job link"
START=$(python3 -c 'import datetime as d; t=d.datetime.now(d.timezone.utc)+d.timedelta(minutes=7); print(t.replace(second=0,microsecond=0).strftime("%Y-%m-%dT%H:%M:%S+00:00"))')
azr --method put --url "$BASE/schedules/$SCHEDULE?api-version=$API" --body "$(jq -cn --arg start "$START" '{name:"every-15-minutes",properties:{startTime:$start,frequency:"Minute",interval:15,timeZone:"UTC"}}')" >/dev/null 2>&1 \
  || say "schedule already exists (kept)"
# The job link pins the subscription the runbook scans. The list endpoint omits parameters, so each
# link is read individually; a link with a different or missing subscription is replaced.
pinned_link_count() {
  local count=0 id
  for id in $(azr --method get --url "$BASE/jobSchedules?api-version=$API" --query "value[?properties.runbook.name=='$RUNBOOK' && properties.schedule.name=='$SCHEDULE'].properties.jobScheduleId" -o tsv 2>/dev/null); do
    # Azure returns parameter keys with its own casing (SubscriptionID), so compare case-insensitively.
    if [ "$(azr --method get --url "$BASE/jobSchedules/$id?api-version=$API" 2>/dev/null | jq -r '(.properties.parameters // {}) | to_entries | map(select(.key | ascii_downcase == "subscriptionid")) | .[0].value // empty | ascii_downcase')" = "$SUBSCRIPTION" ]; then
      count=$((count + 1))
    else
      say "replacing job link $id (subscription not pinned to this one)" >&2
      azr --method delete --url "$BASE/jobSchedules/$id?api-version=$API" >/dev/null 2>&1 || true
    fi
  done
  echo "$count"
}
if [ "$(pinned_link_count)" = 0 ]; then
  sleep 5
  azr --method put --url "$BASE/jobSchedules/$(python3 -c 'import uuid; print(uuid.uuid4())')?api-version=$API" --body "$(jq -cn --arg rb "$RUNBOOK" --arg sc "$SCHEDULE" --arg sub "$SUBSCRIPTION" '{properties:{runbook:{name:$rb},schedule:{name:$sc},parameters:{SubscriptionId:$sub}}}')" >/dev/null
else
  say "job schedule already linked with this subscription (kept)"
fi

say "verifying"
LINKED=$(pinned_link_count)
SCHED=$(azr --method get --url "$BASE/schedules/$SCHEDULE?api-version=$API" --query "{enabled:properties.isEnabled,next:properties.nextRun,interval:properties.interval,frequency:properties.frequency}")
printf '%s\n' "$SCHED" | jq -c .
NEXT=$(date -u -d "$(jq -r '.next // empty' <<<"$SCHED")" +%s 2>/dev/null || echo 0)
if [ "$LINKED" != 1 ] || [ "$(jq -r 'if .enabled == true and .frequency == "Minute" and .interval == 15 then "ok" else "bad" end' <<<"$SCHED")" != ok ] || [ "$NEXT" -le "$(date +%s)" ]; then
  echo "reaper is NOT ready: runbook link count $LINKED, schedule $SCHED" >&2; exit 1
fi
say "reaper ready: VMs tagged purpose=horizon-azure-vm-spike with a past deadline tag are deallocated within about 15 minutes"
