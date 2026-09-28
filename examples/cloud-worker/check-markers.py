#!/usr/bin/env python3
"""Check that a worker image reports every contract marker of this revision's checker."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys


CHECKER = Path(__file__).resolve().with_name('horizon-worker-check')
MARKER = re.compile(r'horizon-[a-z]+(?:-[a-z]+)*-contract=[0-9]+')
# Reported only for a selected remote browser account, or on a running worker (--ready).
CONDITIONAL = frozenset({'horizon-browserstack-contract=1', 'horizon-self-stop-contract=1'})
# An explicit empty selection checks the contract itself, not a profile's agents or browsers.
SELECTION = json.dumps({'agents': [], 'browsers': [], 'desktop': False}, separators=(',', ':'))


def expected_markers(checker=CHECKER):
    return sorted(set(MARKER.findall(checker.read_text())) - CONDITIONAL)


def missing_markers(output, expected):
    reported = set(output.splitlines())
    return [marker for marker in expected if marker not in reported]


# Docker pulls a missing image first, and a CUDA base can take many minutes to pull.
def check_image(image, docker='docker', timeout=1800):
    if not image or image.startswith('-') or any(character.isspace() for character in image):
        raise ValueError('Invalid image reference')
    command = [docker, 'run', '--rm', '--network=none',
               '--entrypoint', '/usr/local/bin/horizon-worker-check',
               '--env', 'HORIZON_WORKER_CAPABILITIES=' + SELECTION, image, '--git-auth']
    result = subprocess.run(command, capture_output=True, text=True, timeout=timeout, check=False)
    if result.returncode != 0:
        raise ValueError('The image checker failed: ' + (result.stderr.strip() or 'no output'))
    return missing_markers(result.stdout, expected_markers())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('image', help='Local or pullable image reference, such as a recipe base pinned by digest')
    parser.add_argument('--docker', default='docker')
    args = parser.parse_args()
    try:
        missing = check_image(args.image, args.docker)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(error, file=sys.stderr)
        return 1
    if missing:
        print('The image lacks current worker contract markers: ' + ', '.join(missing), file=sys.stderr)
        print('Copy the current helpers into it; see examples/cloud-worker/README.md.', file=sys.stderr)
        return 1
    print(f'The image reports all {len(expected_markers())} current worker contract markers.')
    return 0


if __name__ == '__main__':
    sys.exit(main())
