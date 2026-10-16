#!/usr/bin/python3
"""Opt-in host START only: original Docker readback, never a receipt decoder."""
import json
import os
from pathlib import Path
import re
import select
import signal
import stat
import subprocess
import sys
import time
import uuid

DOCKER = ['/usr/bin/docker', '--host', 'unix:///var/run/docker.sock', '--config', '/root/.docker']
SYSTEM_ENV = {'PATH': '/usr/bin:/bin', 'HOME': '/root', 'LANG': 'C.UTF-8'}
ROOT = Path('/run/horizon-worker-ingress')
DESTINATION = '/run/horizon-host-ingress'
WORKSPACE = '/mnt/horizon-volume/workspace'
IMAGE = '/etc/horizon-worker/image'
ENVIRONMENT = '/etc/horizon-worker/worker.env'
GENERATION_LABEL = 'io.horizon.ingress.generation'
ALLOCATION_LABEL = 'io.horizon.ingress.allocation'
EXTERNAL_START, LISTENER_START, COUNT = 20000, 600, 32
MAX_OUTPUT = 32 * 1024
FORMAT = ('{"id":{{json .Id}},"image":{{json .Config.Image}},'
          '"image_id":{{json .Image}},"pid":{{json .State.Pid}},'
          '"running":{{json .State.Running}},"ports":{{json .NetworkSettings.Ports}},'
          '"sysctls":{{json .HostConfig.Sysctls}},"mounts":{{json .Mounts}},'
          '"labels":{{json .Config.Labels}}}')


class Refused(Exception):
    pass


class Stopped(Exception):
    pass


def stop(signum, _frame):
    raise Stopped(128 + signum)


def require(condition):
    if not condition:
        raise Refused()


def bounded(argv, deadline, limit=MAX_OUTPUT):
    """No shell, inherited credentials, child stderr or unbounded pipe capture."""
    require(time.monotonic() < deadline)
    child = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.DEVNULL, env=SYSTEM_ENV, start_new_session=True)
    output = bytearray()
    completed = False
    try:
        while True:
            remaining = deadline - time.monotonic()
            require(remaining > 0)
            readable, _, _ = select.select([child.stdout], [], [], remaining)
            require(readable)
            part = os.read(child.stdout.fileno(), min(4096, limit + 1 - len(output)))
            if not part:
                break
            output.extend(part)
            require(len(output) <= limit)
        # Keep the original leader unreaped until the group-cleanup decision.
        # Its descendants can outlive it and still own this command's pipe.
        while True:
            require(time.monotonic() < deadline)
            status = os.waitid(os.P_PID, child.pid, os.WEXITED | os.WNOWAIT | os.WNOHANG)
            if status is not None:
                require(status.si_code == os.CLD_EXITED and status.si_status == 0)
                break
            time.sleep(min(0.01, deadline - time.monotonic()))
        require(child.wait(timeout=max(0.001, deadline - time.monotonic())) == 0)
        completed = True
        return bytes(output)
    finally:
        if not completed:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait(timeout=3)
        child.stdout.close()


def trusted_path(path):
    current = Path(os.path.realpath(path))
    for ancestor in (current, *current.parents):
        info = ancestor.stat()
        require(info.st_uid == 0 and not info.st_mode & 0o022)
    return current


def root_file(path, limit):
    trusted_path(path)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        info = os.fstat(fd)
        require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and info.st_uid == 0)
        data = os.read(fd, limit + 1)
        require(len(data) <= limit and info.st_size == len(data))
        after = os.fstat(fd)
        require((after.st_dev, after.st_ino, after.st_mode, after.st_uid, after.st_nlink,
                 after.st_size, after.st_mtime_ns, after.st_ctime_ns)
                == (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_nlink,
                    info.st_size, info.st_mtime_ns, info.st_ctime_ns))
        return data.decode('utf-8')
    finally:
        os.close(fd)


def preflight():
    require(os.getuid() == os.geteuid() == 0 and sys.platform == 'linux')
    require(all(hasattr(os, name) for name in ['pidfd_open', 'waitid', 'WNOWAIT']))
    for tool in ['/usr/bin/python3', '/usr/bin/docker', '/usr/bin/nsenter', '/usr/bin/cat']:
        require(os.access(trusted_path(tool), os.X_OK))
    image = root_file(IMAGE, 512).strip()
    require(re.fullmatch(r'[A-Za-z0-9/._:@-]+@sha256:[0-9a-f]{64}', image) is not None)
    operations = [line.partition('=')[2] for line in root_file(ENVIRONMENT, MAX_OUTPUT).splitlines()
                  if line.startswith('HORIZON_CLOUD_OPERATION=')]
    require(len(operations) == 1 and re.fullmatch(r'[A-Za-z0-9_-]{1,100}', operations[0]) is not None)
    return image, operations[0]


def publications(ports):
    """Compare actual Docker publication, not a caller's serialized receipt."""
    actual = []
    for key, bindings in ports.items():
        if not bindings:
            continue  # An image EXPOSE declaration with no host publication.
        require(re.fullmatch(r'[0-9]+/tcp', key) is not None)
        internal = int(key.split('/')[0])
        for binding in bindings:
            require(set(binding) == {'HostIp', 'HostPort'})
            require(binding['HostIp'] in ('0.0.0.0', '::'))
            require(re.fullmatch(r'[0-9]+', binding['HostPort']) is not None)
            actual.append((binding['HostIp'], int(binding['HostPort']), internal))
    require(len(actual) in (33, 66) and len(set(actual)) == len(actual))
    expected = set()
    for family in {entry[0] for entry in actual}:
        expected.add((family, 22, 22))
        expected.update((family, EXTERNAL_START + i, LISTENER_START + i) for i in range(COUNT))
    require(set(actual) == expected)
    return [dict(protocol='tcp', host_ip=ip, host_port=external, container_port=internal)
            for ip, external, internal in sorted(actual)]


def proc_text(pid, leaf):
    with open(f'/proc/{pid}/{leaf}', encoding='utf-8') as source:
        text = source.read(MAX_OUTPUT + 1)
    require(len(text) <= MAX_OUTPUT)
    return text


def process(pid):
    raw = proc_text(pid, 'stat')
    start = int(raw.rsplit(') ', 1)[1].split()[19])
    require(start > 0)
    fields = dict(line.split(':', 1) for line in proc_text(pid, 'status').splitlines()
                  if ':' in line)
    require(all(int(uid) == 0 for uid in fields['Uid'].split()))
    require(int(fields['CapEff'], 16) & (1 << 10))  # Effective CAP_NET_BIND_SERVICE.
    namespace = os.stat(f'/proc/{pid}/ns/net')
    require(namespace.st_ino != os.stat('/proc/self/ns/net').st_ino)
    return start, namespace.st_dev, namespace.st_ino


class Original:
    def __init__(self, image, allocation, shm):
        self.image, self.allocation, self.shm = image, allocation, shm
        self.generation = str(uuid.uuid4())
        self.directory = ROOT / self.generation
        self.cid = None
        self.creation_attempted = False
        self.attach = None

    def create(self):
        ROOT.mkdir(mode=0o700, exist_ok=True)
        require(not ROOT.is_symlink() and stat.S_IMODE(ROOT.stat().st_mode) == 0o700)
        trusted_path(ROOT)
        self.directory.mkdir(mode=0o700)
        argv = DOCKER + ['create', '--name', 'horizon-worker', '--pull', 'never',
                        '--cidfile', str(self.directory / 'container.cid'),
                        '--label', GENERATION_LABEL + '=' + self.generation,
                        '--label', ALLOCATION_LABEL + '=' + self.allocation,
                        '--cap-add', 'NET_BIND_SERVICE',
                        '--sysctl', 'net.ipv4.ip_unprivileged_port_start=1024',
                        '-p', '22:22', '-p', '20000-20031:600-631/tcp',
                        '--shm-size', str(self.shm) + 'g', '--log-driver', 'local',
                        '--log-opt', 'max-size=10m', '--log-opt', 'max-file=5',
                        '-v', WORKSPACE + ':/workspace', '--env-file', ENVIRONMENT,
                        '--mount', f'type=bind,src={self.directory},dst={DESTINATION},readonly', self.image]
        self.creation_attempted = True
        cid = bounded(argv, time.monotonic() + 30, 128).decode().strip()
        require(re.fullmatch(r'[0-9a-f]{64}', cid) is not None)
        self.cid = cid

    def inspect(self, deadline, bindings=True, cid=None):
        expected = self.cid if cid is None else cid
        observed = json.loads(bounded(DOCKER + ['inspect', '--format', FORMAT, expected], deadline))
        require(observed['id'] == expected and observed['image'] == self.image)
        require(observed['labels'].get(GENERATION_LABEL) == self.generation)
        require(observed['labels'].get(ALLOCATION_LABEL) == self.allocation)
        require(re.fullmatch(r'sha256:[0-9a-f]{64}', observed['image_id']) is not None)
        if bindings:
            require(observed['sysctls'].get('net.ipv4.ip_unprivileged_port_start') == '1024')
            mounts = [m for m in observed['mounts'] if m['Destination'] == DESTINATION]
            require(len(mounts) == 1 and mounts[0]['Type'] == 'bind' and not mounts[0]['RW'])
            require(mounts[0]['Source'] == str(self.directory))
        return observed

    def receipt(self):
        deadline = time.monotonic() + 3
        while True:
            before = self.inspect(deadline)
            if before['running'] and type(before['pid']) is int and before['pid'] > 0:
                break
            require(self.attach.poll() is None and time.monotonic() < deadline)
            time.sleep(0.01)
        pid = before['pid']
        pidfd = os.pidfd_open(pid)
        netfd = None
        try:
            netfd = os.open(f'/proc/{pid}/ns/net', os.O_RDONLY | os.O_CLOEXEC)
            first = process(pid)
            namespace = os.fstat(netfd)
            require(first[1:] == (namespace.st_dev, namespace.st_ino))
            command = ['/usr/bin/nsenter', f'--net=/proc/{os.getpid()}/fd/{netfd}', '--',
                       '/usr/bin/cat', '/proc/sys/net/ipv4/ip_unprivileged_port_start']
            boundary = int(bounded(command, deadline, 32))
            require(1024 <= boundary <= 65535)
            published = publications(before['ports'])
            require(self.inspect(deadline) == before and process(pid) == first)
            require(int(bounded(command, deadline, 32)) == boundary)
            require(not select.select([pidfd], [], [], 0)[0])
            self.publish(self.encode_receipt(before, first, boundary, published))
        finally:
            if netfd is not None:
                os.close(netfd)
            os.close(pidfd)

    def encode_receipt(self, observed, identity, boundary, published):
        facts = dict(version=1, allocation=self.allocation, generation=self.generation,
                     container=dict(id=self.cid, requested_image=self.image, image_id=observed['image_id'],
                                    init_start_ticks=identity[0], netns=dict(device=identity[1], inode=identity[2])),
                     unprivileged_port_start=boundary, published=published)
        data = json.dumps(facts, separators=(',', ':')).encode() + b'\n'
        require(len(data) <= 16 * 1024)
        return data

    def publish(self, data):
        directory = os.open(self.directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            fd = os.open('receipt.tmp', os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                         0o400, dir_fd=directory)
            with os.fdopen(fd, 'wb') as output:
                output.write(data)
                output.flush()
                os.fsync(output.fileno())
            # Atomic creation without replacement: the generation is never refreshed.
            os.link('receipt.tmp', 'receipt.json', src_dir_fd=directory, dst_dir_fd=directory,
                    follow_symlinks=False)
            os.unlink('receipt.tmp', dir_fd=directory)
            os.fsync(directory)
        finally:
            os.close(directory)

    def close(self):
        # Deny new receipt reads before waiting on any Docker child or cleanup.
        revoked = True
        try:
            (self.directory / 'receipt.json').unlink(missing_ok=True)
        except OSError:
            revoked = False
        clean = not self.creation_attempted
        try:
            if self.creation_attempted:
                if self.cid is None:
                    ids = bounded(DOCKER + ['ps', '--all', '--quiet', '--no-trunc', '--filter',
                                           'label=' + GENERATION_LABEL + '=' + self.generation],
                                  time.monotonic() + 3, 128).decode().splitlines()
                    require(len(ids) == 1 and re.fullmatch(r'[0-9a-f]{64}', ids[0]) is not None)
                    self.inspect(time.monotonic() + 3, bindings=False, cid=ids[0])
                    self.cid = ids[0]  # Original nonce only; cleanup, never an admission.
                bounded(DOCKER + ['rm', '--force', self.cid], time.monotonic() + 15, 128)
                require(not bounded(DOCKER + ['ps', '--all', '--quiet', '--no-trunc', '--filter',
                                              'id=' + self.cid], time.monotonic() + 3, 128).strip())
                clean = True
        except (Refused, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
            clean = False
        finally:
            if self.attach is not None:
                try:
                    if self.attach.poll() is None:
                        try:
                            os.killpg(self.attach.pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                    self.attach.wait(timeout=3)
                except (OSError, subprocess.SubprocessError):
                    clean = False
            if clean and revoked:
                try:
                    for name in ['receipt.tmp', 'container.cid']:
                        (self.directory / name).unlink(missing_ok=True)
                    if self.directory.exists():
                        self.directory.rmdir()
                except OSError:
                    clean = False
        return clean and revoked


def pull(image):
    for attempt in range(10):
        try:
            bounded(DOCKER + ['image', 'inspect', '--format', '{{.Id}}', image], time.monotonic() + 3, 128)
            return
        except (Refused, subprocess.SubprocessError):
            try:
                bounded(DOCKER + ['pull', '--quiet', image], time.monotonic() + 120)
                return
            except (Refused, subprocess.SubprocessError):
                require(attempt < 9)
                time.sleep(15)


def main():
    os.umask(0o077)
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    owner = None
    result = 1
    try:
        require(len(sys.argv) == 2 and sys.argv[1].isdigit() and 1 <= int(sys.argv[1]) <= 64)
        image, allocation = preflight()
        pull(image)
        owner = Original(image, allocation, int(sys.argv[1]))
        owner.create()
        owner.attach = subprocess.Popen(DOCKER + ['start', '--attach', owner.cid],
                                        stdin=subprocess.DEVNULL, env=SYSTEM_ENV, start_new_session=True)
        owner.receipt()
        result = owner.attach.wait()  # Original attached logs and actual container exit status.
    except Stopped as error:
        result = error.args[0]
    except (Refused, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
        print('horizon-worker: protected ingress startup or cleanup unresolved', file=sys.stderr)
    finally:
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        if owner is not None and not owner.close():
            print('horizon-worker: original container cleanup unresolved', file=sys.stderr)
            result = 1
    return result if 0 <= result <= 255 else 1


if __name__ == '__main__':
    sys.exit(main())
