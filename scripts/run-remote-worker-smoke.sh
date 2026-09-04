#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Run the local security smoke test for the Horizon remote-worker image.

Usage:
  run-remote-worker-smoke.sh [--image <reference>] [--keep-image]

Options:
  --image <reference>  Test an existing local image instead of building one.
  --keep-image         Keep an image built by this script after the smoke.
  -h, --help           Show this help.
EOF
}

fail() {
  printf 'remote-worker smoke failed: %s\n' "$1" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

image=
keep_image=false
while (($# > 0)); do
  case "$1" in
    --image)
      (($# >= 2)) || fail "--image requires a value"
      image=$2
      shift 2
      ;;
    --keep-image)
      keep_image=true
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
done

for command_name in docker ssh ssh-keygen ssh-keyscan; do
  require_command "${command_name}"
done

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
smoke_id="horizon-worker-smoke-$(date -u +%Y%m%d%H%M%S)-$$"
temp_dir=$(mktemp -d /tmp/horizon-remote-worker-smoke.XXXXXX)
case "${temp_dir}" in
  /tmp/horizon-remote-worker-smoke.*) ;;
  *) fail "refusing unexpected temporary directory: ${temp_dir}" ;;
esac

declare -a containers=()
built_image=false

cleanup() {
  local exit_code=$?
  local cleanup_status=0
  local container_name
  for container_name in "${containers[@]}"; do
    docker rm --force "${container_name}" >/dev/null 2>&1 || cleanup_status=1
  done
  if [[ "${built_image}" == true && "${keep_image}" != true ]]; then
    docker image rm --force "${image}" >/dev/null 2>&1 || cleanup_status=1
  fi
  case "${temp_dir}" in
    /tmp/horizon-remote-worker-smoke.*)
      rm -rf -- "${temp_dir}" || cleanup_status=1
      ;;
  esac
  if ((exit_code == 0 && cleanup_status != 0)); then
    printf '%s\n' "remote-worker smoke cleanup failed" >&2
    exit "${cleanup_status}"
  fi
  return "${exit_code}"
}
trap cleanup EXIT

deadline_after() {
  docker run --rm --entrypoint /bin/date "${image}" \
    -u --date="+$1 seconds" +%Y-%m-%dT%H:%M:%SZ
}

record_container() {
  containers+=("$1")
}

expect_usage_failure() {
  local case_name=$1
  shift
  local container_name="${smoke_id}-${case_name}"
  local output
  local status
  record_container "${container_name}"

  set +e
  output=$(docker run --name "${container_name}" "$@" "${image}" 2>&1)
  status=$?
  set -e

  [[ "${status}" -eq 64 ]] ||
    fail "${case_name} returned ${status}, expected configuration exit 64"
  if [[ "${output}" == *"${fake_token}"* ]]; then
    fail "${case_name} exposed the test token in container logs"
  fi
}

wait_for_host_key() {
  local container_name=$1
  local known_hosts_path=$2
  local port
  local attempt

  port=$(docker port "${container_name}" 22/tcp | sed -n 's/.*://p' | tail -n 1)
  [[ "${port}" =~ ^[0-9]+$ ]] ||
    fail "could not resolve the SSH port for ${container_name}"

  for attempt in {1..30}; do
    if ssh-keyscan -T 1 -p "${port}" -t ed25519 127.0.0.1 \
      >"${known_hosts_path}" 2>/dev/null &&
      [[ -s "${known_hosts_path}" ]]; then
      printf '%s\n' "${port}"
      return 0
    fi
    if [[ "$(docker inspect --format '{{.State.Running}}' "${container_name}")" != true ]]; then
      docker logs "${container_name}" >&2 || true
      fail "${container_name} exited before SSH became ready"
    fi
    sleep 1
  done

  docker logs "${container_name}" >&2 || true
  fail "SSH did not become ready for ${container_name}"
}

ssh_worker() {
  local identity_file=$1
  local known_hosts_path=$2
  local port=$3
  local remote_command=$4

  ssh \
    -F /dev/null \
    -i "${identity_file}" \
    -o BatchMode=yes \
    -o IdentitiesOnly=yes \
    -o KbdInteractiveAuthentication=no \
    -o PasswordAuthentication=no \
    -o StrictHostKeyChecking=yes \
    -o "UserKnownHostsFile=${known_hosts_path}" \
    -p "${port}" \
    root@127.0.0.1 \
    "${remote_command}"
}

if [[ -z "${image}" ]]; then
  image="${smoke_id}:local"
  worker_image_version="smoke-$(date -u +%Y%m%d%H%M%S)"
  printf 'Building %s\n' "${image}"
  docker build \
    --file "${repo_root}/containers/remote-worker/Dockerfile" \
    --build-arg "WORKER_IMAGE_VERSION=${worker_image_version}" \
    --tag "${image}" \
    "${repo_root}"
  built_image=true
  set +e
  empty_version_output=$(docker build \
    --file "${repo_root}/containers/remote-worker/Dockerfile" \
    --build-arg WORKER_IMAGE_VERSION= "${repo_root}" 2>&1)
  empty_version_status=$?
  set -e
  [[ "${empty_version_status}" -ne 0 ]] || fail "worker image build accepted an empty version"
  grep -qF 'WORKER_IMAGE_VERSION must not be empty' <<<"${empty_version_output}" ||
    fail "empty-version build did not reach the expected validation"
else
  worker_image_version=$(
    docker image inspect \
      --format '{{ index .Config.Labels "org.opencontainers.image.version" }}' \
      "${image}"
  )
  [[ -n "${worker_image_version}" && "${worker_image_version}" != "<no value>" ]] ||
    fail "${image} has no worker image version label"
fi

actual_image_version=$(
  docker image inspect \
    --format '{{ index .Config.Labels "org.opencontainers.image.version" }}' \
    "${image}"
)
[[ "${actual_image_version}" == "${worker_image_version}" ]] ||
  fail "worker image version label does not match the requested build version"

image_environment=$(docker image inspect --format '{{json .Config.Env}}' "${image}")
if grep -Eq '"(GH_TOKEN|GITHUB_TOKEN|HORIZON_GITHUB_TOKEN|HORIZON_SSH_PUBLIC_KEY)=' \
  <<<"${image_environment}"; then
  fail "image configuration contains runtime authentication material"
fi

image_history=$(docker history --no-trunc --format '{{.CreatedBy}}' "${image}")
if grep -Eq '(github_pat_|ghp_|rpa_)' <<<"${image_history}"; then
  fail "image history resembles embedded authentication material"
fi

docker run --rm --entrypoint /bin/sh "${image}" -eu -c '
  test -z "$(find /workspace/horizon -mindepth 1 -print -quit)"
  test ! -e /opt/horizon-dependency-cache
  test -z "$(find /etc/ssh -maxdepth 1 -name "ssh_host_*" -print -quit)"
  test ! -e /root/.ssh
  test ! -e /root/.config
  test ! -e /root/.docker
  test ! -e /root/.gitconfig
  test ! -e /root/.local
  test ! -e /root/.netrc
  test ! -e /root/.npm
  test ! -e /root/.npmrc
  test -s /etc/ssl/certs/ca-certificates.crt
  for tool in \
    cargo rustc git git-lfs gh rsync tar tmux ssh sshd \
    codex claude gemini opencode kilo pi grok
  do
    command -v "${tool}" >/dev/null
  done
'

ssh-keygen -q -t ed25519 -N '' -f "${temp_dir}/client"
ssh-keygen -q -t ed25519 -N '' -f "${temp_dir}/wrong-client"
ssh-keygen -q -t rsa -b 2048 -N '' -f "${temp_dir}/unsupported-client"
client_public_key=$(<"${temp_dir}/client.pub")
unsupported_public_key=$(<"${temp_dir}/unsupported-client.pub")
fake_token="ghp_horizon_worker_smoke_${smoke_id}_not_real"
printf '%s' "${fake_token}" >"${temp_dir}/github-token"
chmod 0600 "${temp_dir}/github-token"
head -c 16385 /dev/zero | tr '\0' x >"${temp_dir}/oversized-token"

normal_deadline=$(deadline_after 600)
oversized_deadline=$(deadline_after 2678400)

expect_usage_failure \
  missing-key \
  --env "HORIZON_TERMINATE_AFTER=${normal_deadline}"
expect_usage_failure \
  malformed-key \
  --env HORIZON_SSH_PUBLIC_KEY=not-a-public-key \
  --env "HORIZON_TERMINATE_AFTER=${normal_deadline}"
expect_usage_failure \
  unsupported-key \
  --env "HORIZON_SSH_PUBLIC_KEY=${unsupported_public_key}" \
  --env "HORIZON_TERMINATE_AFTER=${normal_deadline}"
expect_usage_failure \
  missing-lease \
  --env "HORIZON_SSH_PUBLIC_KEY=${client_public_key}"
expect_usage_failure \
  oversized-lease \
  --env "HORIZON_SSH_PUBLIC_KEY=${client_public_key}" \
  --env "HORIZON_TERMINATE_AFTER=${oversized_deadline}"
expect_usage_failure \
  direct-token \
  --env "HORIZON_SSH_PUBLIC_KEY=${client_public_key}" \
  --env "HORIZON_TERMINATE_AFTER=${normal_deadline}" \
  --env "HORIZON_GITHUB_TOKEN=${fake_token}"
expect_usage_failure \
  github-token-env \
  --env "HORIZON_SSH_PUBLIC_KEY=${client_public_key}" \
  --env "HORIZON_TERMINATE_AFTER=${normal_deadline}" \
  --env "GITHUB_TOKEN=${fake_token}"
expect_usage_failure \
  gh-token-env \
  --env "HORIZON_SSH_PUBLIC_KEY=${client_public_key}" \
  --env "HORIZON_TERMINATE_AFTER=${normal_deadline}" \
  --env "GH_TOKEN=${fake_token}"
expect_usage_failure \
  oversized-token \
  --env "HORIZON_SSH_PUBLIC_KEY=${client_public_key}" \
  --env "HORIZON_TERMINATE_AFTER=${normal_deadline}" \
  --mount "type=bind,src=${temp_dir}/oversized-token,dst=/run/secrets/github-token,readonly" \
  --env HORIZON_GITHUB_TOKEN_FILE=/run/secrets/github-token
expect_usage_failure \
  misplaced-token \
  --env "HORIZON_SSH_PUBLIC_KEY=${client_public_key}" \
  --env "HORIZON_TERMINATE_AFTER=${normal_deadline}" \
  --mount "type=bind,src=${temp_dir}/github-token,dst=/run/secrets/other-token,readonly" \
  --env HORIZON_GITHUB_TOKEN_FILE=/run/secrets/other-token
expect_usage_failure \
  writable-token \
  --env "HORIZON_SSH_PUBLIC_KEY=${client_public_key}" \
  --env "HORIZON_TERMINATE_AFTER=${normal_deadline}" \
  --mount "type=bind,src=${temp_dir}/github-token,dst=/run/secrets/github-token" \
  --env HORIZON_GITHUB_TOKEN_FILE=/run/secrets/github-token

printf '%s\n' '#!/bin/sh' 'sleep 7' 'exit 0' >"${temp_dir}/slow-gh"
chmod 0755 "${temp_dir}/slow-gh"
startup_expiry_worker="${smoke_id}-startup-expiry"
record_container "${startup_expiry_worker}"
docker run --detach \
  --name "${startup_expiry_worker}" \
  --label horizon.remote-worker-smoke=true \
  --env "HORIZON_SSH_PUBLIC_KEY=${client_public_key}" \
  --env "HORIZON_TERMINATE_AFTER=$(deadline_after 5)" \
  --mount "type=bind,src=${temp_dir}/github-token,dst=/run/secrets/github-token,readonly" \
  --env HORIZON_GITHUB_TOKEN_FILE=/run/secrets/github-token \
  --mount "type=bind,src=${temp_dir}/slow-gh,dst=/usr/bin/gh,readonly" \
  "${image}" >/dev/null
startup_expired=false
for _ in {1..20}; do
  if [[ "$(docker inspect --format '{{.State.Running}}' "${startup_expiry_worker}")" != true ]]; then
    startup_expired=true
    break
  fi
  sleep 1
done
[[ "${startup_expired}" == true ]] ||
  fail "worker did not enforce a deadline that expired during initialization"
startup_expiry_logs=$(docker logs "${startup_expiry_worker}" 2>&1)
grep -qF 'horizon-worker: lease deadline reached; terminating worker' \
  <<<"${startup_expiry_logs}" ||
  fail "startup-expiry worker did not log the watchdog marker"
if grep -qF 'Server listening on' <<<"${startup_expiry_logs}"; then
  fail "worker accepted SSH after its deadline expired during initialization"
fi

worker_a="${smoke_id}-worker-a"
worker_b="${smoke_id}-worker-b"
record_container "${worker_a}"
record_container "${worker_b}"

docker run --detach \
  --name "${worker_a}" \
  --label horizon.remote-worker-smoke=true \
  --publish 127.0.0.1::22 \
  --env "HORIZON_SSH_PUBLIC_KEY=${client_public_key}" \
  --env "HORIZON_TERMINATE_AFTER=${normal_deadline}" \
  --mount "type=bind,src=${temp_dir}/github-token,dst=/run/secrets/github-token,readonly" \
  --env HORIZON_GITHUB_TOKEN_FILE=/run/secrets/github-token \
  "${image}" >/dev/null

docker run --detach \
  --name "${worker_b}" \
  --label horizon.remote-worker-smoke=true \
  --publish 127.0.0.1::22 \
  --env "HORIZON_SSH_PUBLIC_KEY=${client_public_key}" \
  --env "HORIZON_TERMINATE_AFTER=${normal_deadline}" \
  "${image}" >/dev/null

worker_a_port=$(wait_for_host_key "${worker_a}" "${temp_dir}/worker-a-known-hosts")
worker_b_port=$(wait_for_host_key "${worker_b}" "${temp_dir}/worker-b-known-hosts")
worker_a_fingerprint=$(
  ssh-keygen -lf "${temp_dir}/worker-a-known-hosts" | awk '{print $2}'
)
worker_b_fingerprint=$(
  ssh-keygen -lf "${temp_dir}/worker-b-known-hosts" | awk '{print $2}'
)
[[ "${worker_a_fingerprint}" != "${worker_b_fingerprint}" ]] ||
  fail "the two workers reused the same runtime SSH host key"

ssh_worker \
  "${temp_dir}/client" \
  "${temp_dir}/worker-b-known-hosts" \
  "${worker_b_port}" \
  'horizon-agent-session env' |
  grep -qx 'HORIZON=1' ||
  fail "horizon-agent-session did not mark the remote environment"

git_config_before=$(docker exec "${worker_a}" sha256sum /root/.gitconfig | awk '{print $1}')
ssh_worker \
  "${temp_dir}/client" \
  "${temp_dir}/worker-a-known-hosts" \
  "${worker_a_port}" \
  'horizon-agent-session sleep 2' &
session_one_pid=$!
ssh_worker \
  "${temp_dir}/client" \
  "${temp_dir}/worker-a-known-hosts" \
  "${worker_a_port}" \
  'horizon-agent-session sleep 2' &
session_two_pid=$!
set +e
wait "${session_one_pid}"
session_one_status=$?
wait "${session_two_pid}"
session_two_status=$?
set -e
[[ "${session_one_status}" -eq 0 && "${session_two_status}" -eq 0 ]] ||
  fail "concurrent token-backed agent sessions failed"
git_config_after=$(docker exec "${worker_a}" sha256sum /root/.gitconfig | awk '{print $1}')
[[ "${git_config_after}" == "${git_config_before}" ]] ||
  fail "token-backed agent sessions changed shared Git configuration"
git_rewrites=$(
  docker exec "${worker_a}" \
    git config --global --get-all url.https://github.com/.insteadOf
)
expected_git_rewrites=$'git@github.com:\nssh://git@github.com/'
[[ "${git_rewrites}" == "${expected_git_rewrites}" ]] ||
  fail "repeated token-backed sessions left unexpected Git URL rewrites"

set +e
ssh_worker \
  "${temp_dir}/wrong-client" \
  "${temp_dir}/worker-a-known-hosts" \
  "${worker_a_port}" \
  'true' >/dev/null 2>&1
wrong_key_status=$?
set -e
[[ "${wrong_key_status}" -ne 0 ]] ||
  fail "worker accepted an SSH identity that was not authorized"

authorized_key=$(docker exec "${worker_a}" cat /root/.ssh/authorized_keys)
[[ "${authorized_key}" == "${client_public_key}" ]] ||
  fail "worker authorized_keys differs from the supplied public key"
[[ "$(docker exec "${worker_a}" stat -c %a /root/.ssh/authorized_keys)" == 600 ]] ||
  fail "worker authorized_keys is not mode 0600"

copied_token=$(docker exec "${worker_a}" cat /run/horizon/github-token)
[[ "${copied_token}" == "${fake_token}" ]] ||
  fail "worker runtime token copy differs from the mounted secret"
[[ "$(docker exec "${worker_a}" stat -c %a /run/horizon/github-token)" == 600 ]] ||
  fail "worker runtime token copy is not mode 0600"

worker_environment=$(docker inspect --format '{{json .Config.Env}}' "${worker_a}")
if [[ "${worker_environment}" == *"${fake_token}"* ]]; then
  fail "mounted token entered the worker container environment"
fi
pid_one_environment=$(
  docker exec "${worker_a}" /bin/sh -c "tr '\\0' '\\n' </proc/1/environ"
)
if [[ "${pid_one_environment}" == *"${fake_token}"* ]] ||
  grep -Eq '^(GH_TOKEN|GITHUB_TOKEN|HORIZON_(GITHUB_TOKEN|GITHUB_TOKEN_FILE|SSH_PUBLIC_KEY|TERMINATE_AFTER))=' \
    <<<"${pid_one_environment}"; then
  fail "runtime credentials remained in the SSH daemon environment"
fi
if grep -qF "${fake_token}" <<<"${image_history}"; then
  fail "mounted token entered image history"
fi

effective_sshd_config=$(docker exec "${worker_a}" sshd -T)
for required_setting in \
  'authenticationmethods publickey' \
  'passwordauthentication no' \
  'kbdinteractiveauthentication no' \
  'permitemptypasswords no' \
  'allowagentforwarding no' \
  'x11forwarding no' \
  'permituserenvironment no'
do
  grep -qx "${required_setting}" <<<"${effective_sshd_config}" ||
    fail "effective sshd configuration is missing: ${required_setting}"
done
grep -Eq '^permitrootlogin (prohibit-password|without-password)$' \
  <<<"${effective_sshd_config}" ||
  fail "root login is not restricted to public-key authentication"

lease_worker="${smoke_id}-lease"
record_container "${lease_worker}"
docker run --detach \
  --name "${lease_worker}" \
  --label horizon.remote-worker-smoke=true \
  --env "HORIZON_SSH_PUBLIC_KEY=${client_public_key}" \
  --env "HORIZON_TERMINATE_AFTER=$(deadline_after 5)" \
  "${image}" >/dev/null

lease_stopped=false
for _ in {1..20}; do
  if [[ "$(docker inspect --format '{{.State.Running}}' "${lease_worker}")" != true ]]; then
    lease_stopped=true
    break
  fi
  sleep 1
done
[[ "${lease_stopped}" == true ]] ||
  fail "lease watchdog did not stop the worker within 20 seconds"
lease_logs=$(docker logs "${lease_worker}" 2>&1)
grep -qF 'horizon-worker: lease deadline reached; terminating worker' \
  <<<"${lease_logs}" ||
  fail "lease watchdog marker was not written"

image_id=$(docker image inspect --format '{{.Id}}' "${image}")
printf 'Remote-worker smoke passed.\n'
printf 'Image: %s (%s)\n' "${image_id}" "${actual_image_version}"
printf 'Worker A host key: %s\n' "${worker_a_fingerprint}"
printf 'Worker B host key: %s\n' "${worker_b_fingerprint}"
