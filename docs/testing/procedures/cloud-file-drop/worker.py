#!/usr/bin/env python3
"""Stand-in cloud worker for the cloud file drop procedure.

Runs an unprivileged OpenSSH server on 127.0.0.1 in a bubblewrap namespace whose
`/workspace` is a private folder, so the real drop commands run unchanged. nss_wrapper
maps the worker's SSH user `root` to the user that runs this script; nothing gets root.
The stand-in `horizon-worker-session` records the session and runs a plain shell.

Usage: worker.py --tools <unpacked-debs> --state <new-directory> --seed <tools-of-the-candidate>
Stop it with Ctrl-C or SIGTERM. It writes <seed>/worker.json for seed.py.
"""
import argparse
import json
import os
import signal
import socket
import subprocess
import sys
from pathlib import Path

SESSION = r"""#!/bin/bash
# Stand-in: records the session as the worker does, then runs a shell in it.
set -eu
if [ "${1:-}" = --shared ]; then shift; fi
id=$1
agent=$2
mkdir -p "/workspace/sessions/$id"
printf '%s\n' "$agent" > "/workspace/sessions/$id/agent"
cd /workspace
export HOME=/workspace/home PS1='worker:\w\$ '
exec bash --norc --noprofile -i
"""

CHECK = """#!/bin/sh
printf '%s\\n' horizon-worker-contract=1 horizon-shared-checkout-contract=1
"""


def free_port():
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', 0))
        return probe.getsockname()[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tools', type=Path, required=True)
    parser.add_argument('--state', type=Path, required=True)
    parser.add_argument('--seed', type=Path, required=True)
    args = parser.parse_args()
    tools = args.tools.resolve(strict=True)
    args.state.mkdir(mode=0o700, parents=True, exist_ok=False)
    state = args.state.resolve()
    ssh = state / 'ssh'
    ssh.mkdir(mode=0o700)
    for name in ('host_ed25519', 'client_ed25519'):
        subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-C', 'stand-in', '-f', str(ssh / name)],
                       check=True)
    (ssh / 'authorized_keys').write_bytes((ssh / 'client_ed25519.pub').read_bytes())
    workspace = state / 'workspace'
    (workspace / 'home').mkdir(parents=True)
    (workspace / 'sessions').mkdir()
    bin_dir = state / 'bin'
    bin_dir.mkdir()
    for name, body in (('horizon-worker-session', SESSION), ('horizon-worker-check', CHECK)):
        (bin_dir / name).write_text(body)
        (bin_dir / name).chmod(0o755)
    uid, gid = os.getuid(), os.getgid()
    (state / 'passwd').write_text(f'root:x:{uid}:{gid}:Stand-in worker:/workspace/home:/bin/bash\n')
    (state / 'group').write_text(f'root:x:{gid}:\n')
    port = free_port()
    (state / 'sshd_config').write_text('\n'.join([
        f'Port {port}', 'ListenAddress 127.0.0.1', f'HostKey {ssh / "host_ed25519"}',
        f'AuthorizedKeysFile {ssh / "authorized_keys"}', f'PidFile {state / "sshd.pid"}',
        'UsePAM no', 'PasswordAuthentication no', 'KbdInteractiveAuthentication no',
        'StrictModes no', 'PermitRootLogin yes', 'SetEnv PATH=/usr/local/bin:/usr/bin:/bin', '']))
    wrapper = tools / 'usr/lib/x86_64-linux-gnu/libnss_wrapper.so'
    namespace = [
        'bwrap', '--die-with-parent', '--tmpfs', '/',
        '--ro-bind', '/usr', '/usr', '--symlink', 'usr/bin', '/bin', '--symlink', 'usr/sbin', '/sbin',
        '--symlink', 'usr/lib', '/lib', '--symlink', 'usr/lib64', '/lib64',
        '--ro-bind', '/etc', '/etc', '--dev', '/dev', '--proc', '/proc', '--tmpfs', '/run', '--tmpfs', '/tmp',
        '--bind', str(workspace), '/workspace', '--ro-bind', str(bin_dir), '/usr/local/bin',
        '--ro-bind', str(tools / 'usr/lib/openssh'), '/usr/lib/openssh',
        '--bind', str(state), str(state), '--ro-bind', str(tools), str(tools), '--',
        'env', f'LD_PRELOAD={wrapper}', f'NSS_WRAPPER_PASSWD={state / "passwd"}',
        f'NSS_WRAPPER_GROUP={state / "group"}',
        str(tools / 'usr/sbin/sshd'), '-D', '-e', '-f', str(state / 'sshd_config'),
    ]
    log = (state / 'sshd.log').open('wb')
    server = subprocess.Popen(namespace, stdout=log, stderr=log, start_new_session=True)
    seed = args.seed.resolve(strict=True)
    (seed / 'worker.json').write_text(json.dumps({
        'port': port, 'identity': str(ssh / 'client_ed25519'), 'workspace': str(workspace)}))
    print(json.dumps({'port': port, 'workspace': str(workspace), 'pid': server.pid}), flush=True)

    def stop(_signal, _frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, stop)
    try:
        server.wait()
        sys.exit(f'sshd exited: {server.returncode}; see {state / "sshd.log"}')
    except KeyboardInterrupt:
        pass
    finally:
        try:
            os.killpg(server.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        server.wait()
        log.close()


if __name__ == '__main__':
    main()
