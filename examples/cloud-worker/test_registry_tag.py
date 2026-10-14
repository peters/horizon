"""The registry steps that keep published worker image tags immutable."""
import hashlib
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('registry-tag.sh')


def run(command, ref, answers):
    """Runs the script with a docker stand-in that gives `answers` in turn.

    Each answer is (exit status, stdout bytes, stderr text); the last one repeats.
    The stand-in records how often it was called."""
    with tempfile.TemporaryDirectory() as root:
        root = Path(root)
        for index, (status, stdout, stderr) in enumerate(answers):
            (root / f'answer{index}').write_bytes(stdout)
            (root / f'answer{index}.err').write_text(stderr)
            (root / f'answer{index}.status').write_text(str(status))
        docker = root / 'docker'
        docker.write_text(f"""#!/bin/sh
count=$(cat {root}/calls 2>/dev/null || echo 0)
echo $((count + 1)) > {root}/calls
index=$count
[ "$index" -ge {len(answers)} ] && index={len(answers) - 1}
cat {root}/answer$index
cat {root}/answer$index.err >&2
exit $(cat {root}/answer$index.status)
""")
        docker.chmod(0o755)
        sleep = root / 'sleep'
        sleep.write_text('#!/bin/sh\n')
        sleep.chmod(0o755)
        env = {**os.environ, 'PATH': f'{root}:{os.environ["PATH"]}'}
        result = subprocess.run(['bash', str(SCRIPT), command, ref], capture_output=True, text=True, env=env,
                                timeout=60, check=False)
        calls = int((root / 'calls').read_text()) if (root / 'calls').exists() else 0
        return result, calls


class RequireAbsentTests(unittest.TestCase):
    def test_a_missing_tag_is_absent(self):
        for message in ['ERROR: ghcr.io/owner/image:sha-1: not found', 'manifest unknown: manifest unknown']:
            result, _ = run('require-absent', 'ghcr.io/owner/image:sha-1', [(1, b'', message)])
            self.assertEqual(result.returncode, 0, message)

    def test_an_existing_tag_is_refused(self):
        result, _ = run('require-absent', 'ghcr.io/owner/image:sha-1', [(0, b'{"schemaVersion":2}', '')])
        self.assertEqual(result.returncode, 1)
        self.assertIn('already exists', result.stderr)

    def test_any_other_error_is_refused(self):
        for message in ['ERROR: unauthorized: authentication required', 'ERROR: dial tcp: i/o timeout', '']:
            result, _ = run('require-absent', 'ghcr.io/owner/image:sha-1', [(1, b'', message)])
            self.assertEqual(result.returncode, 1, message)
            self.assertIn('Cannot tell whether', result.stderr)


class DigestTests(unittest.TestCase):
    def test_the_digest_is_the_hash_of_the_raw_bytes_with_a_final_newline(self):
        raw = b'{"schemaVersion":2,"manifests":[]}\n'
        result, calls = run('digest', 'ghcr.io/owner/image:sha-1', [(0, raw, '')])
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, 'sha256:' + hashlib.sha256(raw).hexdigest() + '\n')
        self.assertEqual(calls, 1)

    def test_a_tag_that_appears_late_is_read_after_retries(self):
        raw = b'{"schemaVersion":2}'
        result, calls = run('digest', 'ghcr.io/owner/image:sha-1', [(1, b'', 'not found'), (1, b'', 'not found'),
                                                                    (0, raw, '')])
        self.assertEqual(result.stdout, 'sha256:' + hashlib.sha256(raw).hexdigest() + '\n')
        self.assertEqual(calls, 3)

    def test_a_tag_that_never_appears_fails_after_twelve_tries(self):
        result, calls = run('digest', 'ghcr.io/owner/image:sha-1', [(1, b'', 'not found')])
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, '')
        self.assertEqual(calls, 12)


if __name__ == '__main__':
    unittest.main()
