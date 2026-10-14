"""The worker recipes build the same worker for linux/amd64 and linux/arm64."""
from pathlib import Path
import re
import unittest

HERE = Path(__file__).parent
ARCHITECTURES = ('amd64', 'arm64')


def run_instructions(recipe):
    """Each RUN instruction of a Dockerfile, with its continuation lines joined."""
    joined = re.sub(r'\\\n', ' ', recipe)
    return [line for line in joined.splitlines() if line.startswith('RUN ')]


class RecipeArchitectureTests(unittest.TestCase):
    def test_every_pinned_download_has_a_checksum_for_each_architecture(self):
        recipe = (HERE / 'Dockerfile').read_text()
        pinned = [run for run in run_instructions(recipe) if 'sha256sum -c' in run]
        # gh, geckodriver and BrowserStackLocal.
        self.assertEqual(len(pinned), 3)
        for run in pinned:
            for arch in ARCHITECTURES:
                self.assertRegex(run, rf'\b{arch}\) [^;]*\b[0-9a-f]{{64}}\b')
            self.assertRegex(run, r'\*\) echo "no pinned [^"]*" >&2; exit 1;;')

    def test_chromium_has_a_source_for_each_architecture(self):
        recipe = (HERE / 'Dockerfile').read_text()
        [chromium] = [run for run in run_instructions(recipe) if '*,chromium,*' in run]
        self.assertIn("amd64) curl -fsSL https://dl.google.com/linux/linux_signing_key.pub", chromium)
        self.assertIn('install -y --no-install-recommends google-chrome-stable', chromium)
        self.assertIn('arm64) apt-get update', chromium)
        self.assertIn('install -y --no-install-recommends chromium;;', chromium)
        # Only chromium and the libraries Ubuntu lacks may come from Debian.
        self.assertIn("Package: *\\nPin: origin deb.debian.org\\nPin-Priority: -1", chromium)
        self.assertIn("Package: chromium chromium-common libdav1d6 libjpeg62-turbo\\nPin: origin deb.debian.org\\nPin-Priority: 500",
                      chromium)
        self.assertIn('*) echo "no Chromium source for $arch" >&2; exit 1;;', chromium)

    def test_no_recipe_or_build_script_fixes_one_architecture(self):
        for name in ('Dockerfile', 'Dockerfile.helpers', 'build-base-image.sh', 'build-helpers-image.sh'):
            self.assertNotIn('linux/amd64', (HERE / name).read_text(), name)
        for name in ('build-base-image.sh', 'build-helpers-image.sh'):
            script = (HERE / name).read_text()
            self.assertIn("platform=${PLATFORM:-$(docker version --format '{{.Server.Os}}/{{.Server.Arch}}')}", script)
            self.assertNotRegex(script, r'docker (?:build|create|run) (?!.*--platform "\$platform")')


if __name__ == '__main__':
    unittest.main()
