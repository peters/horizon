#!/usr/bin/env python3
"""Create a source/credential-free worker context containing only stripped helpers."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--bin-dir', required=True, type=Path)
parser.add_argument('--output', required=True, type=Path)
args = parser.parse_args()
if args.output.exists():
    parser.error('output must be a new directory to exclude historical or credential files')
args.output.mkdir(parents=True)
source = Path(__file__).resolve().parent
for path in source.iterdir():
    if path.name == 'Dockerfile' or path.name.startswith('horizon-worker-'):
        shutil.copy2(path, args.output / path.name)
(args.output / 'bin').mkdir()
manifest = {}
for name in ['horizon-cloud-worker', 'horizon-browser', 'horizon-device']:
    target = args.output / 'bin' / name
    shutil.copy2(args.bin_dir / name, target)
    subprocess.run(['strip', '--strip-unneeded', str(target)], check=True)
    manifest[name] = {'bytes': target.stat().st_size, 'sha256': hashlib.sha256(target.read_bytes()).hexdigest()}
(args.output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(json.dumps(manifest, indent=2))
