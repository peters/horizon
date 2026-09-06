#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 2 || $1 != --image || -z $2 ]]; then
  printf '%s\n' 'usage: run-remote-worker-host-identity-smoke.sh --image <local-image>' >&2
  exit 64
fi
image=$2
for utility in docker ssh ssh-keygen python3; do
  command -v "$utility" >/dev/null
done
image_id=$(docker image inspect --format '{{.Id}}' "$image")
[[ $image_id =~ ^sha256:[a-f0-9]{64}$ ]] || exit 1
fixture=$(mktemp -d /tmp/horizon-host-key-smoke.XXXXXX)
smoke_id="horizon-host-key-${fixture##*.}-$$"
volume="${smoke_id}-workspace"
owned_volume=false
containers=()

fail() { printf 'Host identity smoke failed: %s\n' "$1" >&2; exit 1; }
owned_worker() {
  local observed
  observed=$(docker container inspect --format '{{.Id}}|{{.Image}}|{{index .Config.Labels "horizon.host-key-smoke"}}' "$1") || return 1
  [[ $observed == "$1|$image_id|$smoke_id" ]]
}
remove_worker() {
  owned_worker "$1" || return 1
  docker container rm --force "$1" >/dev/null
}
stop_worker() {
  owned_worker "$1" || fail 'worker ownership changed before Stop'
  docker stop --time 10 "$1" >/dev/null
}
cleanup() {
  local result=$? container
  trap - EXIT
  for container in "${containers[@]}"; do
    if docker container inspect "$container" >/dev/null 2>&1; then
      remove_worker "$container" || result=1
    fi
  done
  if [[ $owned_volume == true ]]; then
    if [[ $(docker volume inspect --format '{{index .Labels "horizon.host-key-smoke"}}' "$volume") == "$smoke_id" ]]; then
      docker volume rm "$volume" >/dev/null || result=1
    else
      result=1
    fi
  fi
  case "$fixture" in
    /tmp/horizon-host-key-smoke.*) rm -r -- "$fixture" || result=1 ;;
    *) result=1 ;;
  esac
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

if docker volume inspect "$volume" >/dev/null 2>&1; then
  fail 'task volume already exists'
fi
docker volume create --label "horizon.host-key-smoke=$smoke_id" "$volume" >/dev/null
[[ $(docker volume inspect --format '{{index .Labels "horizon.host-key-smoke"}}' "$volume") == "$smoke_id" ]] ||
  fail 'volume ownership mismatch'
owned_volume=true
ssh-keygen -q -t ed25519 -N '' -C '' -f "$fixture/client"
ssh-keygen -q -t ed25519 -N '' -C '' -f "$fixture/other-client"
client_public=$(<"$fixture/client.pub")
other_public=$(<"$fixture/other-client.pub")
expected_key=

create_worker() {
  local access=$1
  created_id=$(docker create --pull=never --label "horizon.host-key-smoke=$smoke_id" \
    --publish 127.0.0.1::22 --mount "type=volume,src=$volume,dst=/workspace" \
    --env "HORIZON_SSH_PUBLIC_KEY=$access" "$image_id")
  [[ $created_id =~ ^[a-f0-9]{64}$ ]] || fail 'invalid created container identity'
  containers+=("$created_id")
  owned_worker "$created_id" || fail 'created worker ownership mismatch'
  docker start "$created_id" >/dev/null
}

wait_ready() {
  local container=$1 attempt
  port=$(docker port "$container" 22/tcp | sed -n 's/^127\.0\.0\.1://p')
  [[ $port =~ ^[0-9]+$ ]] || fail 'missing loopback SSH port'
  for attempt in {1..40}; do
    if observed_key=$(docker exec "$container" cat /etc/ssh/ssh_host_ed25519_key.pub 2>/dev/null); then
      [[ -z $expected_key || $observed_key == "$expected_key" ]] || fail 'retained host identity changed'
      printf '[127.0.0.1]:%s %s\n' "$port" "${expected_key:-$observed_key}" >"$fixture/known-hosts"
      if connect true 2>/dev/null; then return 0; fi
    fi
    [[ $(docker inspect --format '{{.State.Running}}' "$container") == true ]] || fail 'worker exited before SSH'
    sleep 0.25
  done
  fail 'SSH startup timed out'
}

connect() {
  ssh -F /dev/null -i "$fixture/client" -o BatchMode=yes -o IdentitiesOnly=yes \
    -o StrictHostKeyChecking=yes -o "UserKnownHostsFile=$fixture/known-hosts" \
    -o GlobalKnownHostsFile=/dev/null -o ConnectTimeout=2 -p "$port" root@127.0.0.1 "$@"
}

runtime=$(python3 -c 'import uuid; print(uuid.uuid4())')
panel_request() {
  python3 - "$runtime" "$1" "$2" <<'PY' | connect horizon-panel-session request
import json
import sys
runtime, operation, panel = sys.argv[1:]
command = "from pathlib import Path; import time; Path('panel-" + panel + "-runs').open('a').write('once\\n'); "
if panel == "completed":
    command += "raise SystemExit(42)"
else:
    command += "\nwhile True:\n with Path('panel-ticks').open('a') as stream: stream.write('tick\\n')\n time.sleep(0.05)"
request = {"version": 1, "operation": operation, "runtime": runtime, "panel": panel}
if operation != "status":
    request.update(directory=".", argv=["/usr/bin/python3", "-c", command])
print(json.dumps(request))
PY
}
panel_state() {
  panel_request "$1" "$2" | python3 -c 'import json,sys; print(json.load(sys.stdin)["state"])'
}
panel_markers() {
  connect "sha256sum /workspace/.horizon-worker/panels/$runtime/completed.json /workspace/.horizon-worker/panels/$runtime/progress.json"
}
panel_bytes() {
  connect 'sha256sum /workspace/horizon/panel-completed-runs /workspace/horizon/panel-progress-runs /workspace/horizon/panel-ticks'
}
assert_no_replay() {
  local panel
  for panel in completed progress; do
    [[ $(panel_state status "$panel") == unavailable ]] || fail 'lost task reported a live or completed process'
    [[ $(panel_state start "$panel") == unavailable ]] || fail 'start replayed a retained task'
    [[ $(panel_state verify "$panel") == unavailable ]] || fail 'verification replayed a retained task'
    if connect horizon-panel-session attach "$runtime" "$panel" >/dev/null 2>&1; then
      fail 'attachment accepted a lost task'
    fi
  done
  [[ $(panel_markers) == "$retained_markers" ]] || fail 'task identity records changed'
  [[ $(panel_bytes) == "$retained_bytes" ]] || fail 'task bytes changed without a new task'
  if connect "tmux -N -S /run/horizon/panels/$runtime.sock list-sessions" >/dev/null 2>&1; then
    fail 'a new task server was created'
  fi
}

create_worker "$client_public"
first_id=$created_id
wait_ready "$first_id"
expected_key=$observed_key
connect 'printf "retained-workspace-data\n" > /workspace/horizon/retained.txt'
panel_request start completed >/dev/null
[[ $(panel_state start progress) == running ]] || fail 'progress task did not start'
for attempt in {1..40}; do
  [[ $(panel_state status completed) == exited ]] && break
  sleep 0.1
done
[[ $(panel_state status completed) == exited ]] || fail 'completed task did not exit'
before_ticks=$(connect 'wc -l < /workspace/horizon/panel-ticks')
for attempt in {1..40}; do
  after_ticks=$(connect 'wc -l < /workspace/horizon/panel-ticks')
  (( after_ticks > before_ticks )) && break
  sleep 0.1
done
(( after_ticks > before_ticks )) || fail 'task did not progress between disconnected clients'
retained_markers=$(panel_markers)
stop_worker "$first_id"
remove_worker "$first_id"

create_worker "$client_public"
second_id=$created_id
[[ $second_id != "$first_id" ]] || fail 'runtime filesystem was not replaced'
wait_ready "$second_id"
[[ $observed_key == "$expected_key" ]] || fail 'host identity changed after runtime filesystem loss'
[[ $(connect 'cat /workspace/horizon/retained.txt') == retained-workspace-data ]] || fail 'workspace data was lost'
for panel in completed progress; do
  [[ $(connect "cat /workspace/horizon/panel-$panel-runs") == once ]] || fail 'a task ran more than once'
done
(( $(connect 'wc -l < /workspace/horizon/panel-ticks') >= after_ticks )) || fail 'task progress was lost'
retained_bytes=$(panel_bytes)
assert_no_replay
stop_worker "$second_id"
remove_worker "$second_id"

create_worker "$other_public"
rejected_id=$created_id
for attempt in {1..40}; do
  [[ $(docker inspect --format '{{.State.Running}}' "$rejected_id") == true ]] || break
  sleep 0.25
done
[[ $(docker inspect --format '{{.State.Running}}:{{.State.ExitCode}}' "$rejected_id") == false:64 ]] ||
  fail 'retained volume accepted a different access identity'
remove_worker "$rejected_id"

create_worker "$client_public"
wait_ready "$created_id"
[[ $observed_key == "$expected_key" ]] || fail 'rejected access changed the original host key'
[[ $(connect 'cat /workspace/horizon/retained.txt') == retained-workspace-data ]] || fail 'rejected access changed data'
assert_no_replay
printf '%s\n' 'PASS retained host key, workspace bytes and task claims after runtime filesystem replacement; no task replay; wrong access key rejected'
