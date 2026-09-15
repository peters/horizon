#!/bin/bash
set -euo pipefail
timeout --kill-after=5s 15 dockerd --version | python3 -c 'import sys; p=sys.stdin.read().split(); assert len(p)>=3 and p[:2]==["Docker","version"] and p[2].rstrip(",")=="29.1.3", "workspace sandbox v1 requires qualified Docker 29.1.3"'
[ "$(cat /sys/module/apparmor/parameters/enabled)" = Y ]
grep -qw errno /proc/sys/kernel/seccomp/actions_avail
printf '%s\n' '__APPARMOR_SHA__  /etc/apparmor.d/horizon-workspace-sandbox-v1' '__SECCOMP_SHA__  /etc/horizon/worker-seccomp-v1.json' | sha256sum --check --status
timeout --kill-after=5s 30 apparmor_parser --replace /etc/apparmor.d/horizon-workspace-sandbox-v1
grep -Fxq 'horizon-workspace-sandbox-v1 (enforce)' /sys/kernel/security/apparmor/profiles
