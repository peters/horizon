"""Pure host producer regressions; no Docker, preflight, SSH or startup execution."""
import importlib.util
from pathlib import Path
import stat
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('ingress', Path(__file__).with_name('ingress.py'))
ingress = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ingress)


def ports():
    return {'22/tcp': [dict(HostIp='0.0.0.0', HostPort='22')],
            **{f'{600 + i}/tcp': [dict(HostIp='0.0.0.0', HostPort=str(20000 + i))]
               for i in range(32)}}


class Producer(unittest.TestCase):
    def owner(self, directory):
        owner = ingress.Original('registry.example/worker@sha256:' + 'a' * 64, 'allocation-original', 2)
        owner.directory = Path(directory) / owner.generation
        owner.directory.mkdir()
        return owner

    def test_actual_publication_needs_complete_families_and_never_backends(self):
        observed = ports()
        self.assertEqual(len(ingress.publications(observed)), 33)
        for bindings in observed.values():
            bindings.append(dict(HostIp='::', HostPort=bindings[0]['HostPort']))
        self.assertEqual(len(ingress.publications(observed)), 66)
        observed['5900/tcp'] = None  # EXPOSE is not publication.
        self.assertEqual(len(ingress.publications(observed)), 66)
        for bad in [('632/tcp', '20000'), ('22/udp', '22'), ('5900/tcp', '5900')]:
            candidate = ports()
            candidate[bad[0]] = [dict(HostIp='0.0.0.0', HostPort=bad[1])]
            with self.assertRaises(ingress.Refused):
                ingress.publications(candidate)
        observed['600/tcp'].pop()
        with self.assertRaises(ingress.Refused):
            ingress.publications(observed)

    def test_nonwildcard_duplicate_and_incorrect_port_maps_are_refused(self):
        for change in ['address', 'duplicate', 'target', 'missing']:
            observed = ports()
            if change == 'address':
                observed['600/tcp'][0]['HostIp'] = '127.0.0.1'
            elif change == 'duplicate':
                observed['600/tcp'].append(observed['600/tcp'][0].copy())
            elif change == 'target':
                observed['600/tcp'][0]['HostPort'] = '20001'
            else:
                observed.pop('631/tcp')
            with self.assertRaises(ingress.Refused):
                ingress.publications(observed)

    def test_atomic_generation_cannot_be_republished(self):
        with tempfile.TemporaryDirectory() as temporary:
            owner = self.owner(temporary)
            owner.publish(b'original\n')
            receipt = owner.directory / 'receipt.json'
            self.assertEqual(receipt.read_bytes(), b'original\n')
            self.assertEqual(stat.S_IMODE(receipt.stat().st_mode), 0o400)
            self.assertEqual(receipt.stat().st_nlink, 1)
            with self.assertRaises(FileExistsError):
                owner.publish(b'successor\n')
            self.assertEqual(receipt.read_bytes(), b'original\n')

    def test_known_original_cleanup_is_not_blocked_by_failed_kernel_readback(self):
        with tempfile.TemporaryDirectory() as temporary:
            owner = self.owner(temporary)
            owner.cid = 'b' * 64
            owner.creation_attempted = True
            owner.publish(b'original\n')
            calls = []

            def command(argv, _deadline, _limit):
                self.assertFalse((owner.directory / 'receipt.json').exists())
                calls.append(argv)
                return (owner.cid + '\n').encode() if 'rm' in argv else b''

            with patch.object(ingress, 'bounded', side_effect=command), \
                    patch.object(owner, 'inspect', side_effect=ingress.Refused):
                self.assertTrue(owner.close())
            self.assertIn(['rm', '--force', owner.cid], [c[len(ingress.DOCKER):] for c in calls])
            self.assertFalse(owner.directory.exists())

    def test_lost_create_answer_never_uses_an_unverified_successor_for_cleanup(self):
        with tempfile.TemporaryDirectory() as temporary:
            owner = self.owner(temporary)
            owner.creation_attempted = True
            calls = []

            def command(argv, _deadline, _limit):
                calls.append(argv)
                return ('c' * 64 + '\n').encode()

            with patch.object(ingress, 'bounded', side_effect=command), \
                    patch.object(owner, 'inspect', side_effect=ingress.Refused):
                self.assertFalse(owner.close())
            self.assertIsNone(owner.cid)
            self.assertFalse(any('rm' in argv for argv in calls))
            self.assertTrue(owner.directory.exists())

    def test_actual_child_pipe_bounds_kill_and_reap_the_same_child(self):
        original = subprocess.Popen
        children = []

        def launch(*args, **kwargs):
            child = original(*args, **kwargs)
            children.append(child)
            return child

        for program in ["import os;os.write(1,b'x'*65)", 'import time;time.sleep(30)']:
            with patch.object(ingress.subprocess, 'Popen', side_effect=launch):
                with self.assertRaises(ingress.Refused):
                    ingress.bounded(['/usr/bin/python3', '-I', '-c', program], time.monotonic() + 0.1, 64)
            self.assertIsNotNone(children[-1].returncode)

        with patch.object(ingress.subprocess, 'Popen', side_effect=launch):
            self.assertEqual(ingress.bounded(['/usr/bin/python3', '-I', '-c',
                                             "import os;os.write(1,b'x'*64)"],
                                            time.monotonic() + 2, 64), b'x' * 64)
        self.assertEqual(children[-1].returncode, 0)

    def test_narrow_docker_format_cannot_dump_environment_or_credentials(self):
        self.assertNotIn('.Config.Env', ingress.FORMAT)
        self.assertNotIn('{{json .}}', ingress.FORMAT)
        for field in ['.Id', '.Image', '.State.Pid', '.NetworkSettings.Ports', '.Mounts']:
            self.assertIn(field, ingress.FORMAT)

    def test_exited_leader_cannot_leave_a_descendant_holding_the_pipe(self):
        # The leader exits before timeout. Its child deliberately keeps stdout.
        with tempfile.TemporaryDirectory() as temporary:
            pidfile = Path(temporary) / 'descendant.pid'
            program = '''import os,sys,time
pid=os.fork()
if pid:
    with open(sys.argv[1], 'w') as output: output.write(str(pid))
    os._exit(0)
time.sleep(30)
'''
            original = subprocess.Popen
            leaders = []

            def launch(*args, **kwargs):
                child = original(*args, **kwargs)
                leaders.append(child)
                return child

            with patch.object(ingress.subprocess, 'Popen', side_effect=launch):
                with self.assertRaises(ingress.Refused):
                    ingress.bounded(['/usr/bin/python3', '-I', '-c', program, str(pidfile)],
                                    time.monotonic() + 1, 64)
            self.assertEqual(leaders[0].returncode, 0)
            descendant = int(pidfile.read_text())
            deadline = time.monotonic() + 2
            while True:
                try:
                    state = Path(f'/proc/{descendant}/stat').read_text().rsplit(') ', 1)[1].split()[0]
                except FileNotFoundError:
                    break
                if state in ('Z', 'X'):
                    break  # Physically dead; original host init/tini reaps the orphan.
                self.assertLess(time.monotonic(), deadline, 'original descendant is still running')
                time.sleep(0.01)


if __name__ == '__main__':
    unittest.main()
