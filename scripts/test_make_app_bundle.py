#!/usr/bin/env python3
"""The macOS app bundle declares why it uses the microphone.

Speech input starts a recorder as a child of Horizon. macOS asks the person for
the microphone on behalf of the responsible process, and it ends that process
when the bundle holds no `NSMicrophoneUsageDescription`. The check writes the
`Info.plist` of the bundle in the mode of `make_app_bundle.sh` that needs no
binary, no `otool` and no `sips`, so a Linux computer runs it too.
"""

import os
from pathlib import Path
import plistlib
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "packaging/macos/make_app_bundle.sh"
MICROPHONE_KEY = "NSMicrophoneUsageDescription"


def write_info_plist(minimum_macos_version="14.2", version="9.9.9"):
    """The `Info.plist` that the script writes, as a dictionary."""
    temporary = tempfile.TemporaryDirectory()
    destination = Path(temporary.name) / "Horizon.app/Contents/Info.plist"
    environment = os.environ.copy()
    environment.update(
        {
            "HORIZON_APP_BUNDLE_PLIST_ONLY": "1",
            "HORIZON_APP_BUNDLE_PLIST": str(destination),
            "HORIZON_APP_BUNDLE_MINIMUM_MACOS_VERSION": minimum_macos_version,
            "HORIZON_APP_BUNDLE_VERSION": version,
        }
    )
    result = subprocess.run(
        ["bash", str(SCRIPT)],
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        temporary.cleanup()
        raise AssertionError(f"make_app_bundle.sh failed: {result.stderr.strip()}")
    try:
        return plistlib.loads(destination.read_bytes())
    finally:
        temporary.cleanup()


@unittest.skipUnless(
    sys.platform.startswith(("linux", "darwin")),
    "the app bundle of macOS is assembled on a Unix system",
)
class InfoPlistTests(unittest.TestCase):
    def test_the_bundle_states_why_it_uses_the_microphone(self):
        plist = write_info_plist()
        self.assertIn(
            MICROPHONE_KEY,
            plist,
            "macOS ends Horizon on the first take without this key",
        )
        description = plist[MICROPHONE_KEY]
        self.assertIsInstance(description, str)
        self.assertIn("microphone", description.lower())
        self.assertGreater(
            len(description.strip()),
            20,
            "the sentence must say what the permission is for",
        )

    def test_the_bundle_keeps_the_identity_that_it_had(self):
        plist = write_info_plist(minimum_macos_version="14.2", version="9.9.9")
        self.assertEqual(plist["CFBundleName"], "Horizon")
        self.assertEqual(plist["CFBundleIdentifier"], "com.github.peters.horizon")
        self.assertEqual(plist["CFBundleExecutable"], "horizon")
        self.assertEqual(plist["CFBundlePackageType"], "APPL")
        self.assertEqual(plist["LSMinimumSystemVersion"], "14.2")
        self.assertEqual(plist["CFBundleVersion"], "9.9.9")
        self.assertEqual(plist["CFBundleShortVersionString"], "9.9.9")


if __name__ == "__main__":
    unittest.main()
