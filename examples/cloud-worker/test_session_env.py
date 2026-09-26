"""Session environment files from image layers, parsed by the real helper."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).parent


class SessionEnvironmentTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.directory = self.root / 'session-env.d'
        self.helper = self.root / 'horizon-worker-session-env'
        source = (SCRIPTS / 'horizon-worker-session-env').read_text()
        self.helper.write_text(source.replace('/etc/horizon-worker/session-env.d', str(self.directory)))

    def layer(self, name, content):
        self.directory.mkdir(exist_ok=True)
        path = self.directory / name
        path.write_bytes(content.encode() if isinstance(content, str) else content)
        return path

    def run_helper(self, mode, path='/usr/local/bin:/usr/bin:/bin'):
        return subprocess.run([sys.executable, str(self.helper), mode], env={'PATH': path},
                              capture_output=True, timeout=10)

    def applied(self, path='/usr/local/bin:/usr/bin:/bin'):
        result = self.run_helper('apply', path)
        self.assertEqual(result.returncode, 0, result.stderr)
        pairs = [item.split('=', 1) for item in result.stdout.decode().split('\0') if item]
        return dict(pairs)

    def refusal(self):
        for mode in ['check', 'apply']:
            result = self.run_helper(mode)
            self.assertEqual(result.returncode, 1, (mode, result.stdout))
            self.assertEqual(result.stdout, b'')
        return result.stderr.decode()

    def test_absent_directory_and_empty_layers_change_nothing(self):
        self.assertEqual(self.applied(), {})
        self.assertEqual(self.run_helper('check').returncode, 0)
        self.layer('20-empty.env', '# nothing to add\n\n')
        self.assertEqual(self.applied(), {})

    def test_two_layers_compose_path_around_the_base(self):
        self.layer('20-gpu-library.env', 'PATH_PREPEND=/usr/local/cuda/bin\nPATH_APPEND=/opt/library/tools\n')
        self.layer('30-gpu-consumer.env', 'PATH_PREPEND=/opt/consumer/bin\nPATH_APPEND=/opt/consumer/tools\n')
        self.assertEqual(self.applied()['PATH'], ':'.join([
            '/opt/consumer/bin', '/usr/local/cuda/bin', '/usr/local/bin', '/usr/bin', '/bin',
            '/opt/library/tools', '/opt/consumer/tools']))

    def test_a_directory_already_on_path_stays_where_it_is(self):
        self.layer('20-a.env', 'PATH_APPEND=/usr/bin\nPATH_PREPEND=/opt/tool/bin\n')
        self.layer('30-b.env', 'PATH_APPEND=/opt/tool/bin\nPATH_APPEND=/opt/x\n')
        self.layer('40-c.env', 'PATH_PREPEND=/opt/x\nPATH_PREPEND=/bin\n')
        self.assertEqual(self.applied('/usr/bin:/bin:/usr/bin')['PATH'], '/opt/tool/bin:/usr/bin:/bin:/opt/x')

    def test_later_files_override_in_lexical_order(self):
        self.layer('20-library.env', 'TOOLKIT=library\nONLY_LIBRARY=1\n')
        self.layer('30-consumer.env', 'TOOLKIT=consumer\nEMPTY=\nSPACED=a b=c  \n')
        # Lexical, not numeric: 100 sorts before 20.
        self.layer('100-first.env', 'TOOLKIT=first\n')
        self.assertEqual(self.applied(), {'TOOLKIT': 'consumer', 'ONLY_LIBRARY': '1',
                                          'EMPTY': '', 'SPACED': 'a b=c  '})

    def test_malformed_files_are_refused_with_their_file_and_line(self):
        cases = [
            ('20-x.env', 'export TOOLKIT=1\n', '20-x.env:1: invalid variable name'),
            ('20-x.env', 'TOOLKIT\n', '20-x.env:1: expected KEY=VALUE'),
            ('20-x.env', '# ok\nTOOLKIT=$(id)\n BAD=1\n', '20-x.env:3: invalid variable name'),
            ('20-x.env', 'PATH=/opt/bin\n', '20-x.env:1: PATH cannot be set by an image layer'),
            ('20-x.env', 'HOME=/root\n', 'HOME cannot be set'),
            ('20-x.env', 'HORIZON_SESSION_DIR=/tmp\n', 'HORIZON_SESSION_DIR cannot be set'),
            ('20-x.env', 'UID=0\n', 'UID cannot be set'),
            ('20-x.env', 'BASHOPTS=x\n', 'BASHOPTS cannot be set'),
            ('20-x.env', 'K' * 129 + '=1\n', 'at most 128'),
            ('20-x.env', 'PATH_PREPEND=opt/bin\n', 'PATH_PREPEND needs one absolute directory'),
            ('20-x.env', 'PATH_APPEND=/a:/b\n', 'PATH_APPEND needs one absolute directory'),
            ('20-x.env', 'TOOLKIT=a\r\n', 'control character'),
            ('20-x.env', b'TOOLKIT=a\0b\n', 'control character'),
            ('20-x.env', 'TOOLKIT=a\tb\n', '20-x.env:1: control character'),
            ('20-x.env', 'TOOLKIT=a\x0bb\n', 'control character'),
            ('20-x.env', 'TOOLKIT=a\x7fb\n', 'control character'),
            ('20-x.env', 'TOOLKIT=a\x85b\n', 'control character'),
            ('20-x.env', 'VALID=1\n# comment\x1b[31m\n', '20-x.env:2: control character'),
            ('20-x.env', b'TOOLKIT=\xff\n', 'not UTF-8'),
            ('20-x.env', 'TOOLKIT=' + 'a' * 4097 + '\n', 'value longer than 4096'),
            ('20-x.env', ('# padding\n' * 2000), 'larger than 16384 bytes'),
            ('20-x.conf', 'TOOLKIT=1\n', 'name must be'),
            ('.hidden.env', 'TOOLKIT=1\n', 'name must be'),
            ('2' * 97 + '.env', 'TOOLKIT=1\n', 'name must be at most 100'),
        ]
        for name, content, message in cases:
            with self.subTest(content=content[:40], name=name):
                self.layer('10-valid.env', 'VALID=1\n')
                path = self.layer(name, content)
                refused = self.refusal()
                self.assertIn('Session environment refused:', refused)
                self.assertIn(message, refused)
                path.unlink()

    def test_names_and_keys_at_their_limits_are_accepted(self):
        self.layer('2' * 96 + '.env', 'K' * 128 + '=1\n')
        self.assertEqual(self.applied(), {'K' * 128: '1'})

    def test_entries_other_than_regular_files_are_refused(self):
        self.layer('10-real.env', 'VALID=1\n')
        (self.directory / '20-link.env').symlink_to(self.directory / '10-real.env')
        self.assertIn('20-link.env: not a regular file', self.refusal())
        (self.directory / '20-link.env').unlink()
        (self.directory / '20-dir.env').mkdir()
        self.assertIn('20-dir.env: not a regular file', self.refusal())
        (self.directory / '20-dir.env').rmdir()
        for index in range(64):
            self.layer(f'{index:02d}-x.env', '')
        self.assertIn('more than 64 files', self.refusal())

    def test_the_directory_itself_must_be_a_directory(self):
        self.directory.write_text('VALID=1\n')
        self.assertIn('not a directory', self.refusal())

    def test_usage(self):
        result = self.run_helper('show')
        self.assertEqual(result.returncode, 2)


if __name__ == '__main__':
    unittest.main()
