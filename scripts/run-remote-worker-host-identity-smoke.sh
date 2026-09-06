#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 2 || $1 != --image || -z $2 ]]; then
  printf '%s\n' 'usage: run-remote-worker-host-identity-smoke.sh --image <local-image>' >&2
  exit 64
fi
image=$2
for utility in docker ssh ssh-keygen; do
  command -v "$utility" >/dev/null
done
docker image inspect "$image" >/dev/null
fixture=$(mktemp -d /tmp/horizon-host-key-smoke.XXXXXX)
smoke_id="horizon-host-key-${fixture##*.}-$$"
volume="${smoke_id}-workspace"
owned_volume=false
containers=()

fail() { printf 'Host identity smoke failed: %s\n' "$1" >&2; exit 1; }
cleanup() {
  local result=$? container
  trap - EXIT
  for container in "${containers[@]}"; do
    if docker container inspect "$container" >/dev/null 2>&1; then
      docker container rm --force "$container" >/dev/null || result=1
    fi
  done
  if [[ $owned_volume == true ]]; then
    docker volume rm "$volume" >/dev/null || result=1
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
    --env "HORIZON_SSH_PUBLIC_KEY=$access" "$image")
  [[ $created_id =~ ^[a-f0-9]{64}$ ]] || fail 'invalid created container identity'
  containers+=("$created_id")
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

create_worker "$client_public"
first_id=$created_id
wait_ready "$first_id"
expected_key=$observed_key
connect 'printf "retained-workspace-data\n" > /workspace/horizon/retained.txt'
docker stop --time 10 "$first_id" >/dev/null
docker rm "$first_id" >/dev/null

create_worker "$client_public"
second_id=$created_id
[[ $second_id != "$first_id" ]] || fail 'runtime filesystem was not replaced'
wait_ready "$second_id"
[[ $observed_key == "$expected_key" ]] || fail 'host identity changed after runtime filesystem loss'
[[ $(connect 'cat /workspace/horizon/retained.txt') == retained-workspace-data ]] || fail 'workspace data was lost'
docker stop --time 10 "$second_id" >/dev/null
docker rm "$second_id" >/dev/null

create_worker "$other_public"
rejected_id=$created_id
for attempt in {1..40}; do
  [[ $(docker inspect --format '{{.State.Running}}' "$rejected_id") == true ]] || break
  sleep 0.25
done
[[ $(docker inspect --format '{{.State.Running}}:{{.State.ExitCode}}' "$rejected_id") == false:64 ]] ||
  fail 'retained volume accepted a different access identity'
docker rm "$rejected_id" >/dev/null

create_worker "$client_public"
wait_ready "$created_id"
[[ $observed_key == "$expected_key" ]] || fail 'rejected access changed the original host key'
[[ $(connect 'cat /workspace/horizon/retained.txt') == retained-workspace-data ]] || fail 'rejected access changed data'
printf '%s\n' 'PASS retained host key and workspace data after runtime filesystem replacement; wrong access key rejected'
