#!/usr/bin/env python3
"""Create a source/credential-free worker context containing only stripped helpers."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess


WORKER_SCRIPTS = (
    'horizon-worker-start', 'horizon-worker-check', 'horizon-worker-configure', 'horizon-worker-supervise',
    'horizon-worker-idle', 'horizon-worker-stop',
    'horizon-worker-run', 'horizon-worker-session', 'horizon-worker-source',
    'horizon-worker-import', 'horizon-worker-siblings', 'horizon-worker-git-auth', 'horizon-worker-browserstack',
)
CONTEXT_FILES = ('Dockerfile', '.dockerignore', *WORKER_SCRIPTS)
HELPERS = ('horizon-cloud-worker', 'horizon-browser', 'horizon-device')


def require_regular(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError('Worker context input must be a regular file: ' + path.name)


def copy_regular(source, destination):
    require_regular(source)
    shutil.copy2(source, destination, follow_symlinks=False)
    require_regular(destination)


def gpu_recipe(recipe, base):
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9./:_-]*@sha256:[0-9a-f]{64}', base) or len(base) > 512:
        raise ValueError('GPU base must be a credential-free image pinned by SHA-256 digest')
    default = 'ARG BASE_IMAGE=ubuntu:24.04'
    if recipe.count(default) != 1:
        raise ValueError('Shared worker recipe has no unique base-image declaration')
    return (recipe.replace(default, 'ARG BASE_IMAGE=' + base, 1)
            + '\n# The selected base must supply the workload GPU libraries; no CPU fallback.\n'
            + 'ENV NVIDIA_VISIBLE_DEVICES=all NVIDIA_DRIVER_CAPABILITIES=compute,utility\n')


def prepare_context(bin_dir, output, gpu_base=None):
    source = Path(__file__).resolve().parent
    for path in [*(source / name for name in CONTEXT_FILES), *(bin_dir / name for name in HELPERS)]:
        require_regular(path)
    gpu = gpu_recipe((source / 'Dockerfile').read_text(), gpu_base) if gpu_base is not None else None
    if output.exists():
        raise ValueError('output must be a new directory to exclude historical or credential files')
    output.mkdir(parents=True)
    for name in CONTEXT_FILES:
        copy_regular(source / name, output / name)
    if gpu is not None:
        (output / 'Dockerfile.gpu').write_text(gpu)
    (output / 'bin').mkdir()
    manifest = {}
    for name in HELPERS:
        target = output / 'bin' / name
        copy_regular(bin_dir / name, target)
        subprocess.run(['strip', '--strip-unneeded', str(target)], check=True)
        manifest[name] = {'bytes': target.stat().st_size, 'sha256': hashlib.sha256(target.read_bytes()).hexdigest()}
    (output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bin-dir', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--gpu-base', help='Pinned upstream CUDA/project runtime; generates Dockerfile.gpu')
    args = parser.parse_args()
    try:
        manifest = prepare_context(args.bin_dir, args.output, args.gpu_base)
    except ValueError as error:
        parser.error(str(error))
    print(json.dumps(manifest, indent=2))


if __name__ == '__main__':
    main()
