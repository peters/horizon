#!/usr/bin/python3 -I
"""Explicit worker-owned versioned byte captures; not atomic snapshots or backups."""

import fcntl
import importlib.util
import os
import resource
import selectors
import subprocess
import sys
import time

spec = importlib.util.spec_from_file_location('capture_store', os.path.join(os.path.dirname(__file__), 'byte_capture_store.py'))
store = importlib.util.module_from_spec(spec)
spec.loader.exec_module(store)
BASE = '/workspace/.horizon-worker'
HELPER = '/usr/local/bin/horizon-repository'
ENV = {'PATH': '/usr/bin:/bin', 'LC_ALL': 'C'}
INTERVAL = 10
ATTEMPT_SECONDS = 15
STALE_SECONDS = 30
LABEL = 'versioned byte capture; not an atomic snapshot or independent backup'
REASONS = ('invalid', 'unsupported', 'identity', 'capture', 'capacity', 'storage', 'changed')


def now():
    return time.time_ns() // 1_000_000


def child_limits():
    resource.setrlimit(resource.RLIMIT_AS, (2 * 1024**3, 2 * 1024**3))
    resource.setrlimit(resource.RLIMIT_FSIZE, (160 * 1024**2, 160 * 1024**2))
    resource.setrlimit(resource.RLIMIT_CPU, (15, 15))


def invoke(enrollment, plan, available, cancelled=lambda: False):
    """Bound child pipes/time. An uninterruptible filesystem can delay OS teardown."""
    request = store.encode({'enrollment': enrollment, 'available_bytes': available})
    store.require(len(request) <= store.LIMIT)
    child = subprocess.Popen([HELPER, 'capture-plan' if plan else 'capture-once'],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        cwd='/', env=ENV, close_fds=True, start_new_session=True, umask=0o077, preexec_fn=child_limits)
    output = bytearray()
    try:
        os.set_blocking(child.stdin.fileno(), False)
        os.set_blocking(child.stdout.fileno(), False)
        view = memoryview(request)
        deadline = time.monotonic() + ATTEMPT_SECONDS
        with selectors.DefaultSelector() as ready:
            ready.register(child.stdin, selectors.EVENT_WRITE)
            ready.register(child.stdout, selectors.EVENT_READ)
            while ready.get_map():
                store.require(time.monotonic() < deadline and not cancelled())
                for key, _ in ready.select(0.1):
                    if key.fileobj is child.stdin:
                        count = os.write(child.stdin.fileno(), view)
                        store.require(count > 0)
                        view = view[count:]
                        if not view:
                            ready.unregister(child.stdin)
                            child.stdin.close()
                    else:
                        data = os.read(child.stdout.fileno(), 1025 - len(output))
                        output.extend(data)
                        store.require(len(output) <= 1024)
                        if not data:
                            ready.unregister(child.stdout)
                if child.poll() is not None and view:
                    raise store.CaptureError('capture request handoff failed')
        code = child.wait(timeout=max(0.01, deadline - time.monotonic()))
        value = store.decode(output)
        if code in (1, 2) and isinstance(value, dict) and value.get('reason') in REASONS:
            raise store.CaptureError(value['reason'])
        store.require(code == 0 and set(value) == {'version', 'binding', 'manifest', 'record_sha256', 'record_bytes', 'reason'}
                      and value['version'] == 1 and value['reason'] is None and store.digest(value['binding']))
        if plan:
            store.require(all(value[field] is None for field in ('manifest', 'record_sha256', 'record_bytes')))
        return value
    finally:
        if child.poll() is None:
            child.kill()  # Only the exact child created above, never a saved PID.
            try:
                child.wait(timeout=1)
            except subprocess.TimeoutExpired:
                pass  # Do not promise a hard kill/reap bound for uninterruptible I/O.
        child.stdin.close()
        child.stdout.close()


def cancelled(slot):
    value = slot.read('cancel.json')
    if value is None:
        return False
    store.require(store.decode(value) == {'cancel': True})
    return True


def service(binding):
    slot = store.open_slot(BASE, binding)
    lock = os.open('lock', os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=slot.fd)
    bundles = receipts = None
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        enrollment = store.decode(slot.read('enrollment.json'))
        bundles = slot.child('bundles')
        receipts = slot.child('receipts')
        state = {'version': 1, 'label': LABEL, 'binding': binding, 'state': 'running',
                 'interval_seconds': INTERVAL, 'stale_after_seconds': STALE_SECONDS,
                 'started_at_millis': now(), 'attempt_started_at_millis': None,
                 'attempt_finished_at_millis': None, 'last_success': None, 'generation': 0, 'last_error': None}
        while not cancelled(slot):
            started = time.monotonic()
            state['attempt_started_at_millis'] = now()
            state['attempt_finished_at_millis'] = None
            slot.write('status.json', state, replace=True)
            try:
                available = store.CAPACITY - bundles.usage()
                observation = invoke(enrollment, False, available, lambda: cancelled(slot))
                store.require(observation['binding'] == binding and not cancelled(slot))
                bundles.verified_record(observation)
                bundles.usage()
                finished = now()
                store.require(finished >= state['attempt_started_at_millis'])
                previous_time = state['last_success']['verified_at_millis'] if state['last_success'] else None
                store.require(previous_time is None or finished >= previous_time)
                success = dict(observation, generation=state['generation'] + 1,
                    capture_started_at_millis=state['attempt_started_at_millis'], verified_at_millis=finished,
                    previous_verified_at_millis=previous_time)
                # One immutable receipt per distinct content version; repeated bytes
                # refresh the observed interval without accumulating new disk records.
                name = observation['manifest'] + '.json'
                previous = receipts.read(name)
                if previous is None:
                    receipts.write(name, success)
                else:
                    previous = store.decode(previous)
                    store.require(all(previous[field] == observation[field] for field in observation))
                store.require(not cancelled(slot))
                state['generation'] = success['generation']
                state['last_success'] = success
                state['state'], state['last_error'] = 'running', None
            except (OSError, ValueError, KeyError, TypeError, store.CaptureError, subprocess.SubprocessError) as error:
                state['state'] = 'cancelled' if cancelled(slot) else 'error'
                state['last_error'] = str(error) if isinstance(error, store.CaptureError) and str(error) in REASONS else 'unavailable'
                if state['state'] != 'cancelled' and state['last_error'] == 'changed':
                    state['state'] = 'degraded'  # Capture detected edits before publication; safe to sample again.
            state['attempt_finished_at_millis'] = now()
            # Success advances only after bundle publication AND verified readback.
            slot.write('status.json', state, replace=True)
            if state['state'] not in ('running', 'degraded'):
                return
            while time.monotonic() - started < INTERVAL and not cancelled(slot):
                time.sleep(0.1)
        state['state'] = 'cancelled'
        slot.write('status.json', state, replace=True)
    finally:
        if bundles is not None:
            bundles.close()
        if receipts is not None:
            receipts.close()
        os.close(lock)
        slot.close()


def start(enrollment):
    binding = invoke(enrollment, True, store.CAPACITY)['binding']
    try:
        slot = store.open_slot(BASE, binding, True)
    except FileExistsError:
        return status(binding)  # Existing enrollment is never automatically relaunched.
    try:
        slot.write('enrollment.json', enrollment)
        slot.child('bundles', True).close()
        slot.child('receipts', True).close()
        # All I/O inherited from the controller is severed; no client heartbeat.
        child = subprocess.Popen(['/usr/bin/python3', '-I', os.path.abspath(__file__), '_run', binding],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            cwd='/', env=ENV, close_fds=True, start_new_session=True, umask=0o077)
        child.poll()
        return {'version': 1, 'binding': binding, 'state': 'submitted', 'label': LABEL}
    finally:
        slot.close()


def status(binding):
    try:
        slot = store.open_slot(BASE, binding)
    except FileNotFoundError:
        return {'version': 1, 'binding': binding, 'state': 'absent', 'label': LABEL}
    try:
        raw = slot.read('status.json')
        if raw is None:
            return {'version': 1, 'binding': binding, 'state': 'claimed_unknown', 'label': LABEL}
        value = store.decode(raw)
        store.require(set(value) == {'version', 'label', 'binding', 'state', 'interval_seconds',
            'stale_after_seconds', 'started_at_millis', 'attempt_started_at_millis',
            'attempt_finished_at_millis', 'last_success', 'generation', 'last_error'}
            and value['version'] == 1 and value['binding'] == binding and value['label'] == LABEL
            and value['state'] in ('running', 'degraded', 'cancelled', 'error')
            and value['last_error'] in (None, 'unavailable') + REASONS
            and value['interval_seconds'] == INTERVAL and value['stale_after_seconds'] == STALE_SECONDS
            and type(value['generation']) is int and value['generation'] >= 0)
        for field in ('started_at_millis', 'attempt_started_at_millis', 'attempt_finished_at_millis'):
            store.require(value[field] is None or type(value[field]) is int and value[field] >= 0)
        success = value['last_success']
        if success is not None:
            store.require(set(success) == {'version', 'binding', 'manifest', 'record_sha256', 'record_bytes', 'reason',
                'generation', 'capture_started_at_millis', 'verified_at_millis', 'previous_verified_at_millis'}
                and success['version'] == 1 and success['binding'] == binding and success['reason'] is None
                and success['generation'] == value['generation']
                and type(success['verified_at_millis']) is int and type(success['capture_started_at_millis']) is int
                and 0 <= success['capture_started_at_millis'] <= success['verified_at_millis'])
            store.require(success['previous_verified_at_millis'] is None or
                type(success['previous_verified_at_millis']) is int and
                0 <= success['previous_verified_at_millis'] <= success['verified_at_millis'])
            bundles = slot.child('bundles')
            try:
                bundles.verified_record(success)
            finally:
                bundles.close()
        observed = now()
        value['observed_at_millis'] = observed
        value['stale'] = success is None or observed < success['verified_at_millis'] or observed - success['verified_at_millis'] > STALE_SECONDS * 1000
        # The stored word "running" is not a fresh process-liveness observation.
        value['recorded_state'] = value['state']
        if value['stale'] and value['state'] in ('running', 'degraded'):
            value['state'] = 'stale'
        return value
    finally:
        slot.close()


def main(arguments):
    if len(arguments) == 1 and arguments[0] == 'start':
        raw = sys.stdin.buffer.read(store.LIMIT + 1)
        store.require(len(raw) <= store.LIMIT)
        return start(store.decode(raw))
    store.require(len(arguments) == 2 and arguments[0] in ('status', 'cancel', '_run') and store.digest(arguments[1]))
    command, binding = arguments
    if command == '_run':
        service(binding)
        return None
    if command == 'cancel':
        slot = store.open_slot(BASE, binding)
        try:
            if not cancelled(slot):
                slot.write('cancel.json', {'cancel': True})
        finally:
            slot.close()
    return status(binding)


if __name__ == '__main__':
    try:
        result = main(sys.argv[1:])
        if result is not None:
            sys.stdout.buffer.write(store.encode(result))
            sys.stdout.buffer.flush()
    except (OSError, ValueError, KeyError, TypeError, store.CaptureError, subprocess.SubprocessError):
        sys.stderr.write('Byte capture unavailable; retain all data and inspect status.\n')
        sys.exit(1)
