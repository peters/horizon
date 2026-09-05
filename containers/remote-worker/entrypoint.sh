#!/bin/sh
set -eu

readonly EX_USAGE=64
readonly GITHUB_TOKEN_MOUNT=/run/secrets/github-token
readonly MAX_LEASE_SECONDS=2592000
readonly MAX_PUBLIC_KEY_BYTES=16384
readonly MAX_TOKEN_BYTES=16384

fail_usage() {
    printf '%s\n' "$1" >&2
    exit "${EX_USAGE}"
}

umask 077

deadline_epoch=
lease_seconds=
if [ "${HORIZON_TERMINATE_AFTER+x}" = x ]; then
    terminate_after=${HORIZON_TERMINATE_AFTER}
    if [ -z "${terminate_after}" ]; then
        fail_usage "HORIZON_TERMINATE_AFTER must contain a future RFC 3339 timestamp when supplied"
    fi
    case "${terminate_after}" in
        *'
'*)
            fail_usage "HORIZON_TERMINATE_AFTER must contain exactly one timestamp"
            ;;
    esac
    if ! printf '%s\n' "${terminate_after}" |
        grep -Eq '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}([.][0-9]+)?(Z|[+-][0-9]{2}:[0-9]{2})$'; then
        fail_usage "HORIZON_TERMINATE_AFTER must contain a valid RFC 3339 timestamp"
    fi
    if ! deadline_epoch=$(date --date="${terminate_after}" +%s 2>/dev/null); then
        fail_usage "HORIZON_TERMINATE_AFTER must contain a valid RFC 3339 timestamp"
    fi
    now_epoch=$(date +%s)
    lease_seconds=$((deadline_epoch - now_epoch))
    if [ "${lease_seconds}" -le 0 ]; then
        fail_usage "HORIZON_TERMINATE_AFTER must be in the future"
    fi
    if [ "${lease_seconds}" -gt "${MAX_LEASE_SECONDS}" ]; then
        fail_usage "HORIZON_TERMINATE_AFTER may be at most 30 days in the future"
    fi
fi

ssh_public_key=${HORIZON_SSH_PUBLIC_KEY:-}
if [ -z "${ssh_public_key}" ]; then
    fail_usage "HORIZON_SSH_PUBLIC_KEY must contain one OpenSSH public key"
fi
case "${ssh_public_key}" in
    *'
'*)
        fail_usage "HORIZON_SSH_PUBLIC_KEY must contain exactly one OpenSSH public key"
        ;;
esac
public_key_bytes=$(printf '%s' "${ssh_public_key}" | wc -c | tr -d '[:space:]')
if [ "${public_key_bytes}" -gt "${MAX_PUBLIC_KEY_BYTES}" ]; then
    fail_usage "HORIZON_SSH_PUBLIC_KEY exceeds the 16384-byte limit"
fi
public_key_kind=${ssh_public_key%%[[:space:]]*}
case "${public_key_kind}" in
    ssh-ed25519) ;;
    *) fail_usage "HORIZON_SSH_PUBLIC_KEY must use an ssh-ed25519 key" ;;
esac

if [ -n "${HORIZON_GITHUB_TOKEN:-}" ] || [ -n "${GITHUB_TOKEN:-}" ] || [ -n "${GH_TOKEN:-}" ]; then
    fail_usage "GitHub tokens are not accepted through environment variables; mount a secret file and set HORIZON_GITHUB_TOKEN_FILE"
fi

mkdir -p /run/horizon /run/sshd /root/.ssh
chmod 0700 /run/horizon /root/.ssh

authorized_key_candidate=/run/horizon/authorized-key.candidate
printf '%s\n' "${ssh_public_key}" > "${authorized_key_candidate}"
if ! ssh-keygen -l -f "${authorized_key_candidate}" >/dev/null 2>&1; then
    rm -f "${authorized_key_candidate}"
    fail_usage "HORIZON_SSH_PUBLIC_KEY is not a valid OpenSSH public key"
fi
install -m 0600 "${authorized_key_candidate}" /root/.ssh/authorized_keys
rm -f "${authorized_key_candidate}"

rm -f /run/horizon/github-token
github_token_file=${HORIZON_GITHUB_TOKEN_FILE:-}
token_bytes=0
if [ -n "${github_token_file}" ]; then
    if [ "${github_token_file}" != "${GITHUB_TOKEN_MOUNT}" ]; then
        fail_usage "HORIZON_GITHUB_TOKEN_FILE must be ${GITHUB_TOKEN_MOUNT}"
    fi
    if [ -L "${github_token_file}" ] || [ ! -f "${github_token_file}" ] || [ ! -r "${github_token_file}" ] || [ ! -s "${github_token_file}" ]; then
        fail_usage "HORIZON_GITHUB_TOKEN_FILE must name a readable non-empty regular file"
    fi
    if ! awk -v target="${GITHUB_TOKEN_MOUNT}" '
        $5 == target {
            option_count = split($6, options, ",")
            for (i = 1; i <= option_count; i++) {
                if (options[i] == "ro") {
                    read_only = 1
                }
            }
        }
        END { exit read_only ? 0 : 1 }
    ' /proc/self/mountinfo; then
        fail_usage "${GITHUB_TOKEN_MOUNT} must be mounted as a read-only file"
    fi
    token_bytes=$(wc -c < "${github_token_file}" | tr -d '[:space:]')
    if [ "${token_bytes}" -gt "${MAX_TOKEN_BYTES}" ]; then
        fail_usage "HORIZON_GITHUB_TOKEN_FILE exceeds the 16384-byte limit"
    fi
    install -m 0600 "${github_token_file}" /run/horizon/github-token

    GITHUB_TOKEN=$(cat /run/horizon/github-token)
    export GITHUB_TOKEN
    GH_TOKEN=${GITHUB_TOKEN}
    export GH_TOKEN
    gh auth setup-git
    git config --global --replace-all url.https://github.com/.insteadOf git@github.com:
    git config --global --add url.https://github.com/.insteadOf ssh://git@github.com/
fi

ssh-keygen -A >/dev/null
if [ ! -s /etc/ssh/ssh_host_ed25519_key ]; then
    printf '%s\n' "failed to generate the runtime SSH host key" >&2
    exit 1
fi

if [ -n "${deadline_epoch}" ]; then
    now_epoch=$(date +%s)
    lease_seconds=$((deadline_epoch - now_epoch))
    if [ "${lease_seconds}" -le 0 ]; then
        printf '%s\n' "horizon-worker: lease deadline reached; terminating worker" >&2
        exit 0
    fi
fi

unset \
    authorized_key_candidate \
    deadline_epoch \
    github_token_file \
    GH_TOKEN \
    HORIZON_GITHUB_TOKEN \
    HORIZON_GITHUB_TOKEN_FILE \
    HORIZON_SSH_PUBLIC_KEY \
    HORIZON_TERMINATE_AFTER \
    GITHUB_TOKEN \
    now_epoch \
    public_key_bytes \
    public_key_kind \
    ssh_public_key \
    terminate_after \
    token_bytes

if [ -n "${lease_seconds}" ]; then
    entrypoint_pid=$$
    (
        sleep "${lease_seconds}"
        printf '%s\n' "horizon-worker: lease deadline reached; terminating worker" >&2
        kill -TERM "${entrypoint_pid}" 2>/dev/null || exit 0
        sleep 10
        kill -KILL "${entrypoint_pid}" 2>/dev/null || true
    ) &
else
    printf '%s\n' "horizon-worker: no expiry supplied; persistent worker" >&2
fi
unset entrypoint_pid lease_seconds

exec /usr/sbin/sshd -D -e
