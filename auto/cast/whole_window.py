"""Staged whole-window v1 evidence through real UI consent and public casting MCP."""
import argparse
import hashlib
import json
import os
import platform
from pathlib import Path
import select
import shutil
import signal
import socket
import subprocess
import sys
import time
import uuid

from bench import fingerprint
from receiver import Receiver
from window_fixture import Desktop, verify_candidate, root_region, native_environment
from window_decode import decode
from window_provenance import decoder_identities, executable_sha256, verified_decoders, verify_file

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
CONTRACT = 'whole-window-v1-exploratory'


def write(path, value):
    temporary = path.with_suffix('.new')
    temporary.write_text(json.dumps(value, indent=2))
    temporary.replace(path)


def wait(predicate, seconds=30):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(0.1)
    raise TimeoutError('whole-window condition did not settle')


def request(root, role, operation, **arguments):
    key = uuid.uuid4().hex
    directory = root / 'commands' / role
    write(directory / (key + '.json'), {'operation': operation, **arguments})
    answer = root / 'responses' / role / (key + '.json')
    wait(answer.exists, 40)
    result = json.loads(answer.read_text())
    answer.unlink()
    return result


def read_json_line(stream, buffer, deadline):
    while True:
        at = buffer.find(b'\n')
        if at >= 0:
            line = bytes(buffer[:at]); del buffer[:at + 1]
            if line:
                return json.loads(line)
            continue
        if len(buffer) >= 4 * 1024 * 1024:
            raise RuntimeError('public MCP response exceeds bound')
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([stream], [], [], remaining)[0]:
            raise TimeoutError('public MCP response deadline')
        block = os.read(stream.fileno(), min(65536, 4 * 1024 * 1024 - len(buffer)))
        if not block:
            raise RuntimeError('public MCP process ended before complete response')
        buffer.extend(block)


class Mcp:
    def __init__(self, binary, log):
        self.process = subprocess.Popen([str(binary), '--browser-mcp'], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=log, bufsize=0)
        self.sequence, self.buffer = 0, bytearray()
        try:
            self.call('initialize', {'protocolVersion': '2025-11-25', 'capabilities': {},
                      'clientInfo': {'name': 'whole-window-benchmark', 'version': '1'}})
            self.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
        except BaseException:
            self.close()
            raise

    def send(self, value):
        self.process.stdin.write((json.dumps(value) + '\n').encode())

    def call(self, method, arguments):
        self.sequence += 1
        key = self.sequence
        self.send({'jsonrpc': '2.0', 'id': key, 'method': method, 'params': arguments})
        deadline = time.monotonic() + 25
        while time.monotonic() < deadline:
            response = read_json_line(self.process.stdout, self.buffer, deadline)
            if response.get('id') == key:
                if 'error' in response:
                    raise RuntimeError('public MCP request failed')
                return response['result']
        raise TimeoutError('public MCP request')

    def cast(self, arguments):
        return self.call('tools/call', {'name': 'cast', 'arguments': arguments})

    def close(self):
        try:
            self.process.stdin.close()
        except BrokenPipeError:
            pass
        for action, timeout in [(None, 3), (self.process.terminate, 2), (self.process.kill, 2)]:
            if action and self.process.poll() is None:
                action()
            try:
                self.process.wait(timeout=timeout)
                self.process.stdout.close()
                return
            except subprocess.TimeoutExpired:
                pass
        raise RuntimeError('public MCP process did not terminate')


def agent(root, role):
    if not os.environ.get('HORIZON_BROWSER_ACTOR', '').startswith('horizon:'):
        raise RuntimeError('agent client requires a real Horizon agent panel identity')
    manifest = json.loads((root / 'prepare.json').read_text())
    receiver = manifest['receiver_id']
    with (root / (role + '-mcp-errors.log')).open('w') as log:
        client = Mcp(root / 'bin/horizon', log)
        write(root / (role + '-ready.json'), {'ready': True})
        pin_requests = 0
        try:
            while not (root / 'finish').exists():
                for path in sorted((root / 'commands' / role).glob('*.json')):
                    operation = json.loads(path.read_text())
                    path.unlink()
                    if operation.get('receiver_id', receiver) != receiver:
                        raise RuntimeError('refusing a receiver outside this owned fixture')
                    result = client.cast(operation)
                    outcome = result.get('structuredContent', {})
                    if result.get('isError'):
                        outcome['error'] = '\n'.join(row.get('text', '') for row in result.get('content', []))
                    # Physical discovery results never enter durable benchmark evidence.
                    for field in ['receivers', 'paired_receivers', 'sessions']:
                        outcome[field] = [row for row in outcome.get(field, [])
                                          if row.get('id', row.get('receiver_id')) == receiver]
                    if operation['operation'] == 'status' and role == 'owner':
                        pending = [row for row in outcome.get('sessions', []) if row['state'] == 'pin_required']
                        if pending:
                            if pin_requests:
                                raise RuntimeError('remembered pairing unexpectedly requested another PIN')
                            pin_requests += 1
                            # This code belongs only to our synthetic receiver, never a human TV.
                            paired = client.cast({'operation': 'pair', 'receiver_id': receiver, 'pin': '1234'})
                            if paired.get('isError'):
                                raise RuntimeError('synthetic pairing failed')
                    write(root / 'responses' / role / path.name,
                          {'isError': result.get('isError', False), 'outcome': outcome,
                           'pin_requests': pin_requests})
                time.sleep(0.05)
        finally:
            client.close()


def require_ok(response):
    if response['isError'] or response['outcome'].get('error'):
        raise RuntimeError('public casting operation failed: ' + str(response['outcome'].get('error')))
    return response['outcome']


def session(root):
    outcome = require_ok(request(root, 'owner', 'status'))
    if len(outcome['sessions']) != 1:
        raise RuntimeError('exactly one owned casting session is required')
    result = outcome['sessions'][0]
    if result['state'] == 'failed':
        raise RuntimeError('casting worker failed: ' + str(result.get('error')))
    return result


def validate_selection(value, backend, scaler):
    expected = 'h264_nvenc' if backend == 'gpu' else 'libx264'
    if value.get('encoder') != expected:
        raise RuntimeError('requested encoder unavailable; fallback cannot qualify requested lane')
    actual = value.get('scaler')
    if actual not in [None, 'cpu', 'cuda'] or (scaler is not None and actual != scaler):
        raise RuntimeError('requested scaler unavailable or invalid')
    if backend == 'cpu' and actual == 'cuda':
        raise RuntimeError('software encoder cannot report CUDA scaling')


def source_digest():
    patterns = ['Cargo*.toml', 'Cargo.lock', '.cargo/**/*', 'crates/**/*',
                'assets/**/*', 'packaging/**/*']
    paths = {path for pattern in patterns for path in REPO.glob(pattern)
             if path.is_file() and not {'target', '__pycache__', '.git'}.intersection(path.relative_to(REPO).parts)}
    return fingerprint(paths)


def benchmark_digest():
    return fingerprint([*HERE.glob('*.py'), HERE / 'bench.sh', HERE / 'requirements.txt',
                        *REPO.glob('scripts/device-smoke/*.py')])


def prepare(arguments):
    root = arguments.output.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    binaries = root / 'bin'
    binaries.mkdir()
    for name, original in [('horizon', arguments.horizon), ('horizon-device', arguments.horizon_device)]:
        shutil.copy2(original.resolve(strict=True), binaries / name)
        (binaries / name).chmod(0o700)
    for role in ['owner', 'outsider']:
        for category in ['commands', 'responses']:
            (root / category / role).mkdir(mode=0o700, parents=True)
    receiver_root = root / 'receiver'
    receiver_root.mkdir(mode=0o700)
    receiver = Receiver(receiver_root, ip='127.0.0.8', label='Window-' + uuid.uuid4().hex[:12], split_streams=True)
    desktop = advertisement = None
    try:
        from zeroconf import ServiceInfo, Zeroconf
        advertisement = Zeroconf()
        service = ServiceInfo('_airplay._tcp.local.', receiver.label + '._airplay._tcp.local.',
                    addresses=[socket.inet_aton(receiver.ip)], port=receiver.port,
                    properties={'deviceid': receiver.label, 'model': 'AppleTV14,1', 'features': '0x0,0x200'},
                    server=receiver.label.lower() + '.local.')
        advertisement.register_service(service)
        manifest = {'contract': CONTRACT, 'receiver_id': receiver.label,
                    'source_sha256': source_digest(), 'benchmark_sha256': benchmark_digest(),
                    'machine': {'kernel': platform.release(), 'architecture': platform.machine(),
                                'python': sys.version},
                    'commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip(),
                    'binary_sha256': hashlib.sha256((binaries / 'horizon').read_bytes()).hexdigest(),
                    'device_sha256': executable_sha256(binaries / 'horizon-device'),
                    'decoder_tools': decoder_identities()}
        write(root / 'prepare.json', manifest)
        desktop = Desktop(root, binaries, arguments.tools.resolve() if arguments.tools else None,
                          device_sha256=manifest['device_sha256'])
        wait(lambda: (root / 'owner-ready.json').exists() and (root / 'outsider-ready.json').exists(), 60)
        manifest['candidate'] = verify_candidate(desktop.application.pid, binaries / 'horizon', root / 'data/horizon.yaml')
        require_ok(request(root, 'owner', 'discover'))
        wait(lambda: any(row['id'] == receiver.label for row in require_ok(request(root, 'owner', 'status'))['receivers']), 40)
        denied = request(root, 'owner', 'start', receiver_id=receiver.label, source={'kind': 'application'})
        if not denied['isError'] or 'user approval' not in (denied['outcome'].get('error') or ''):
            raise RuntimeError('application capture must be denied before actual UI consent')
        manifest['preapproval_denied'] = True
        write(root / 'prepare.json', manifest)
        print(json.dumps({'status': 'PREPARED', 'root': str(root), **desktop.manifest,
              'next': 'Verify the owned native viewer publicly; grant Entire Horizon in its real Cast UI, then run.'}), flush=True)
        while not (root / 'finish').exists():
            if any(child.poll() is not None for child in desktop.children):
                raise RuntimeError('fixture child exited before completion')
            write(root / 'receiver-live.json', {'sessions': receiver.sessions, 'errors': receiver.errors})
            time.sleep(0.25)
        desktop.close_normally(binaries / 'horizon-device')
        write(root / 'normal-close.json', {'normal_close': True})
    finally:
        failures = []
        operations = [receiver.close]
        if advertisement:
            operations += [advertisement.unregister_all_services, advertisement.close]
        if desktop:
            operations += [desktop.close]
        for operation in operations:
            try:
                operation()
            except Exception as error:
                failures.append(str(error))
        write(root / 'receiver-final.json', {'sessions': receiver.sessions, 'errors': receiver.errors})
        write(root / 'closed.json', {'closed': not failures, 'errors': failures})
        if failures:
            raise RuntimeError('fixture cleanup failed: ' + '; '.join(failures))


def capture_reference(root, lab, candidate, device_sha256):
    device = verify_file(root / 'bin/horizon-device', device_sha256)
    env = native_environment(lab)
    windows = subprocess.check_output(['xdotool', 'search', '--onlyvisible', '--pid',
              str(candidate['pid'])], env=env, text=True, timeout=10).split()
    if len(windows) != 1:
        raise RuntimeError('exactly one owned root client window is required')
    geometry = subprocess.check_output(['xwininfo', '-id', windows[0]],
                                       env={**env, 'LC_ALL': 'C'}, text=True, timeout=10)
    region = root_region(geometry)
    image = root / 'root-reference.png'
    verify_file(device, device_sha256)
    receipt = json.loads(subprocess.check_output([device, '--target',
              str(root / 'target.json'), 'screenshot', str(image), '--options',
              json.dumps({'region': region})], env=env, text=True, timeout=15))
    if not receipt['ok'] or receipt['result']['source_region'] != region:
        raise RuntimeError('independent root-client reference capture failed')
    reference = {'image': str(image), 'region': region,
                 'sha256': hashlib.sha256(image.read_bytes()).hexdigest()}
    write(root / 'root-reference.json', reference)
    return reference


def validate_viewer(receipt, endpoint):
    observations = json.loads(Path(receipt).read_text())
    panels = []
    for observation in observations:
        response = observation.get('response', observation)
        data = response.get('structuredContent', response)
        if 'panels' in data:
            matches = [row for row in data['panels'] if row.get('endpoint') == endpoint]
            if len(matches) != 1:
                raise RuntimeError('viewer observation must identify this fixture endpoint')
            panels.append(matches[0])
    if len(panels) < 3:
        raise RuntimeError('three public native viewer observations are required')
    identities = {(row['panel_id'], row.get('diagnostics', {}).get('connection_generation')) for row in panels}
    def presented(row):
        diagnostics = row.get('diagnostics', {})
        host = diagnostics.get('host', {})
        presentation = diagnostics.get('presentation')
        if presentation == 'hidden':
            return False
        outside = presentation == 'not_rendered' and host.get('exclusion') == 'outside_canvas'
        clipped = presentation == 'clipped' and host.get('view_changed_since_reveal') is True
        return row.get('image_displayed') or ((outside or clipped)
                    and diagnostics.get('last_displayed_age_millis') is not None)
    if len(identities) != 1 or not all(row.get('owned_by_caller') and row['connection'] == 'connected'
             and row.get('image_received') and presented(row) for row in panels):
        raise RuntimeError('native viewer ownership/live presentation unverified')
    if panels[-1]['frame_sequence'] <= panels[0]['frame_sequence']:
        raise RuntimeError('native viewer motion unverified')
    observed = [row.get('diagnostics', {}).get('observed_at_millis', 0) for row in panels]
    if not all(observed) or observed[-1] - observed[0] < 2000 or abs(time.time() * 1000 - observed[-1]) > 60000:
        raise RuntimeError('native viewer observations are stale or insufficiently spaced')
    return {'panel_id': panels[-1]['panel_id'], 'observations': len(panels)}


def run(arguments):
    root = arguments.prepared.resolve(strict=True)
    sampler = receiver = None
    try:
        from window_resources import Sampler
        manifest = json.loads((root / 'prepare.json').read_text())
        lab = json.loads((root / 'lab.json').read_text())
        if manifest['contract'] != CONTRACT or not manifest.get('preapproval_denied'):
            raise RuntimeError('unqualified preparation manifest')
        receiver = manifest['receiver_id']
        if not receiver.startswith('Window-'):
            receiver = None
            raise RuntimeError('preparation receiver is not owned synthetic fixture')
        viewer = validate_viewer(arguments.viewer_evidence, lab['vnc_address'])
        candidate = verify_candidate(lab['launcher_pid'], root / 'bin/horizon', root / 'data/horizon.yaml')
        if candidate != manifest['candidate'] or source_digest() != manifest['source_sha256'] or benchmark_digest() != manifest['benchmark_sha256']:
            raise RuntimeError('candidate or benchmark changed since preparation')
        verify_file(root / 'bin/horizon-device', manifest['device_sha256'])
        verified_decoders(manifest['decoder_tools'])
        source = next(row for row in require_ok(request(root, 'owner', 'sources'))['sources'] if row['source']['kind'] == 'application')
        if source['requires_user_approval'] or not source['available']:
            raise RuntimeError('grant Entire Horizon through the actual UI before running')
        start = {'receiver_id': receiver, 'source': {'kind': 'application'},
                 'resolution': arguments.resolution, 'orientation': arguments.orientation}
        denied = request(root, 'outsider', 'start', **start)
        if not denied['isError'] or 'user approval' not in (denied['outcome'].get('error') or ''):
            raise RuntimeError('other workspace gained application permission')
        require_ok(request(root, 'owner', 'start', **start))
        def streaming():
            value = session(root)
            return value if value['state'] == 'streaming' and value['frames'] >= 3 else None
        wait(streaming)
        duplicate = request(root, 'owner', 'start', **start)
        if not duplicate['isError'] or 'already has' not in (duplicate['outcome'].get('error') or ''):
            raise RuntimeError('duplicate start did not preserve receiver ownership')
        time.sleep(2)
        reference = capture_reference(root, lab, candidate, manifest['device_sha256'])
        sampler = Sampler(candidate['pid'], gpu=True)
        first = session(root)
        validate_selection(first, arguments.backend, arguments.scaler)
        first_counter_at = time.monotonic()
        sampler.start()
        time.sleep(arguments.seconds)
        sampler.stop()
        last = session(root)
        last_counter_at = time.monotonic()
        resources = sampler.finish()
        sampler = None
        validate_selection(last, arguments.backend, arguments.scaler)
        if (first['encoder'], first.get('scaler')) != (last['encoder'], last.get('scaler')):
            raise RuntimeError('encoding/scaling selection changed during measurement')
        if last['state'] != 'streaming' or last['frames'] <= first['frames']:
            raise RuntimeError('no sender advancement during fixed measurement window')
        write(root / 'measurement.json', {'seconds': resources['seconds'],
              'began_monotonic': resources['began_monotonic'],
              'ended_monotonic': resources['ended_monotonic'], 'first_frame': first['frames'],
              'last_frame': last['frames'], 'frames': last['frames'] - first['frames'],
              'sender_counter_seconds': last_counter_at - first_counter_at,
              'sender_fps': (last['frames'] - first['frames']) / (last_counter_at - first_counter_at), 'encoder': last['encoder'],
              'scaler': last.get('scaler'), 'resources': resources})
        write(root / 'revoke-required.json', {'status': 'REVOKE_REQUIRED'})
        print('Measurement complete. Revoke the workspace grant through the Cast UI.', flush=True)
        wait(lambda: next(row for row in require_ok(request(root, 'owner', 'sources'))['sources']
                          if row['source']['kind'] == 'application')['requires_user_approval'], 180)
        wait(lambda: session(root)['state'] == 'stopped')
        final = session(root)
        again = request(root, 'owner', 'start', **start)
        if not again['isError'] or 'user approval' not in (again['outcome'].get('error') or ''):
            raise RuntimeError('revoked workspace can restart application capture')
        write(root / 'finish', {'finish': True})
        wait(lambda: (root / 'closed.json').exists(), 40)
        if not json.loads((root / 'closed.json').read_text())['closed']:
            raise RuntimeError('owned fixture cleanup failed')
        received = json.loads((root / 'receiver-final.json').read_text())
        if received['errors'] or len(received['sessions']) != 1:
            raise RuntimeError('independent receiver failed or unexpected session count')
        state = received['sessions'][0]
        if (not state['teardown'] or not state['events'] or state['frames'] != final['frames']
                or state.get('/pair-verify') != 2 or state.get('pin_requests') != 1):
            raise RuntimeError('receiver authentication/accounting/TEARDOWN gate failed')
        if not json.loads((root / 'normal-close.json').read_text())['normal_close']:
            raise RuntimeError('native application did not close normally')
        dimensions = {'720p': (1280, 720), '1080p': (1920, 1080), '4k': (3840, 2160)}[arguments.resolution]
        if arguments.orientation == 'portrait':
            dimensions = dimensions[::-1]
        quality = decode(root / 'receiver', state, dimensions, reference,
                         json.loads((root / 'measurement.json').read_text()),
                         tools=verified_decoders(manifest['decoder_tools']))
        verified_decoders(manifest['decoder_tools'])
        if source_digest() != manifest['source_sha256'] or benchmark_digest() != manifest['benchmark_sha256']:
            raise RuntimeError('source or benchmark changed during experiment')
        summary = {'status': 'PASS', 'contract': CONTRACT, 'score': None, 'exploratory': True,
                   'viewer': viewer, 'quality': quality, 'measurement': json.loads((root / 'measurement.json').read_text()),
                   'scope': 'Actual isolated Horizon capture/crop/encoder/encrypted transport; no displayed TV FPS or latency.'}
        write(root / 'summary.json', summary)
        print(json.dumps(summary), flush=True)
    except (Exception, KeyboardInterrupt) as error:
        write(root / 'run-failure.json', {'status': 'FAIL', 'error': str(error), 'score': None})
        raise
    finally:
        if sampler:
            try:
                write(root / 'partial-resources.json', sampler.finish())
            except Exception as error:
                write(root / 'resource-failure.json', {'error': str(error)})
        if not (root / 'finish').exists():
            try:
                if receiver:
                    request(root, 'owner', 'stop', receiver_id=receiver)
            except Exception as error:
                write(root / 'stop-failure.json', {'error': str(error)})
            finally:
                write(root / 'finish', {'finish': True})
                try:
                    wait(lambda: (root / 'closed.json').exists(), 40)
                    if not json.loads((root / 'closed.json').read_text())['closed']:
                        raise RuntimeError('owned fixture cleanup failed')
                except Exception as error:
                    write(root / 'cleanup-failure.json', {'error': str(error)})


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='phase', required=True)
    prepare_parser = sub.add_parser('prepare')
    for name in ['horizon', 'horizon-device', 'output']:
        prepare_parser.add_argument('--' + name, type=Path, required=True)
    prepare_parser.add_argument('--tools', type=Path)
    run_parser = sub.add_parser('run')
    run_parser.add_argument('--prepared', type=Path, required=True)
    run_parser.add_argument('--viewer-evidence', type=Path, required=True)
    run_parser.add_argument('--seconds', type=int, choices=range(1, 121), default=10)
    run_parser.add_argument('--resolution', choices=['720p', '1080p', '4k'], default='1080p')
    run_parser.add_argument('--orientation', choices=['landscape', 'portrait'], default='landscape')
    run_parser.add_argument('--backend', choices=['cpu', 'gpu'], default='cpu')
    run_parser.add_argument('--scaler', choices=['cpu', 'cuda'])
    agent_parser = sub.add_parser('agent')
    agent_parser.add_argument('--prepared', type=Path, required=True)
    agent_parser.add_argument('--role', choices=['owner', 'outsider'], required=True)
    arguments = parser.parse_args(argv)
    if sys.platform != 'linux' or not __debug__:
        parser.error('Linux and normal Python assertion checks are required')
    try:
        if arguments.phase in ['prepare', 'run']:
            def stop(_signal, _frame):
                raise KeyboardInterrupt
            signal.signal(signal.SIGTERM, stop)
        if arguments.phase == 'prepare':
            prepare(arguments)
        elif arguments.phase == 'run':
            run(arguments)
        else:
            agent(arguments.prepared, arguments.role)
    except (Exception, KeyboardInterrupt) as error:
        root = getattr(arguments, 'prepared', getattr(arguments, 'output', None))
        if root and root.exists():
            write(root / (arguments.phase + '-failure.json'), {'status': 'FAIL', 'error': str(error), 'score': None})
        raise


if __name__ == '__main__':
    main()
