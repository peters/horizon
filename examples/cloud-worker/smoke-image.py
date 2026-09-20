#!/usr/bin/env python3
"""Run the real worker contract and service startup without allocating compute.

Use DOCKER_HOST/DOCKER_CONFIG for the caller's Docker connection. Only a temporary
public SSH key enters the container; source and account credentials are excluded.
The exact task-created container is removed even when a check fails.
"""
import argparse
import json
from pathlib import Path
import signal
import subprocess
import tempfile
import time
import uuid


def run(*args, **kwargs):
    kwargs.setdefault('timeout', 45)
    return subprocess.run(args, check=True, capture_output=True, text=True, **kwargs)


def remove(names):
    failures = []
    for name in names:
        try:
            subprocess.run(['docker', 'rm', '--force', '--volumes', name],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                           check=False, timeout=45)
            remaining = run('docker', 'container', 'ls', '--all', '--quiet',
                            '--filter', 'name=^/' + name + '$')
            if remaining.stdout.strip():
                failures.append(name)
        except (subprocess.SubprocessError, OSError):
            failures.append(name)
    if failures:
        raise RuntimeError('Task container cleanup was not confirmed: ' + ', '.join(failures))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('image', help='Prefer an immutable image digest')
    parser.add_argument('--capabilities', required=True, help='Requested capability JSON')
    parser.add_argument('--expect-absent', action='append', default=[])
    args = parser.parse_args()
    caps = json.loads(args.capabilities)
    selected = json.dumps(caps, separators=(',', ':'))
    name = 'horizon-capability-smoke-' + uuid.uuid4().hex
    contract_name = name + '-contract'
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix='horizon-capability-key-') as directory:
        key = Path(directory) / 'key'
        run('ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', str(key))
        public = key.with_suffix('.pub').read_text().strip()
        try:
            contract = run('docker', 'run', '--name', contract_name, '--network', 'none',
                           '--entrypoint', 'horizon-worker-check', args.image, '--git-auth',
                           '--capabilities-json', selected)
            assert 'horizon-capabilities-contract=1' in contract.stdout
            run('docker', 'run', '-d', '--name', name, '--network', 'none',
                '--env', 'PUBLIC_KEY=' + public,
                '--env', 'HORIZON_WORKER_CAPABILITIES=' + selected, args.image)
            deadline = time.monotonic() + 90
            while True:
                ready = subprocess.run(
                    ['docker', 'exec', name, 'horizon-worker-check', '--ready',
                     '--capabilities-json', selected], capture_output=True, text=True, timeout=40)
                if ready.returncode == 0:
                    break
                if time.monotonic() > deadline:
                    raise RuntimeError('Worker service readiness deadline exceeded: ' + ready.stderr)
                time.sleep(0.2)
            for executable in args.expect_absent:
                check = run('docker', 'exec', name, 'python3', '-c',
                            'import shutil,sys; print(bool(shutil.which(sys.argv[1])))', executable)
                assert check.stdout.strip() == 'False', 'Unexpected installed executable: ' + executable
            config = json.loads(run('docker', 'exec', name, 'cat', '/workspace/agent-mcp.json').stdout)
            expected = set()
            if caps.get('browsers'):
                expected.add('horizon-browser')
            if caps.get('desktop'):
                expected.add('horizon-device')
            assert set(config['mcpServers']) == expected, 'Agent tool advertisement differs from profile'
            for agent in {'codex', 'claude', 'grok'} - set(caps.get('agents', [])):
                check = subprocess.run(['docker', 'exec', name, 'horizon-worker-check', '--agent', agent],
                                       capture_output=True, text=True, timeout=45)
                assert check.returncode != 0, 'Disabled agent accepted: ' + agent
            # An active worker may not reconnect under a different capability selection.
            changed = dict(caps, desktop=not caps.get('desktop', False))
            mismatch = subprocess.run(['docker', 'exec', name, 'horizon-worker-check', '--ready',
                                       '--capabilities-json', json.dumps(changed)], capture_output=True, timeout=45)
            assert mismatch.returncode != 0, 'Runtime capability drift accepted'
        finally:
            remove([contract_name, name])
    print(json.dumps({'image': args.image, 'capabilities': caps, 'contract': 'pass',
                      'services': 'pass', 'managed_tools': sorted(expected),
                      'absent_executables': args.expect_absent, 'cleanup': 'confirmed',
                      'seconds': round(time.monotonic() - started, 3)}))


def interrupted(_signal, _frame):
    raise KeyboardInterrupt


if __name__ == '__main__':
    signal.signal(signal.SIGTERM, interrupted)
    main()
