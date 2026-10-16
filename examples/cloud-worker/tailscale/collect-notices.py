"""Preserve notices from the exact Go modules consumed by the two shipped binaries."""
import json
from pathlib import Path
import shutil
import sys


def collect(packages, destination):
    destination.mkdir(parents=True)
    decoder = json.JSONDecoder()
    text = packages.read_text()
    offset = 0
    modules = {}
    while offset < len(text):
        while offset < len(text) and text[offset].isspace():
            offset += 1
        if offset == len(text):
            break
        package, offset = decoder.raw_decode(text, offset)
        module = package.get('Module')
        if module:
            actual = module.get('Replace', module)
            modules[(module['Path'], module.get('Version', ''))] = actual['Dir']
    manifest = []
    for index, ((name, version), directory) in enumerate(sorted(modules.items())):
        root = Path(directory)
        target = destination / 'dependencies' / str(index)
        notices = []
        for path in sorted(root.rglob('*')):
            if (not path.is_symlink() and path.is_file()
                    and path.name.lower().startswith(('license', 'copying', 'notice', 'copyright', 'patents'))):
                relative = path.relative_to(root)
                output = target / relative
                output.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(path, output)
                notices.append(str(relative))
        if not notices:
            raise ValueError('No license notice for consumed module ' + name)
        manifest.append({'module': name, 'version': version, 'directory': str(index), 'notices': notices})
    if not modules:
        raise ValueError('No consumed Go modules')
    # Tailscale's own license and maintained dependency index accompany full notices.
    shutil.copyfile('LICENSE', destination / 'LICENSE')
    shutil.copyfile('licenses/tailscale.md', destination / 'DEPENDENCIES.md')
    for name in ('LICENSE', 'PATENTS'):
        shutil.copyfile(Path('/usr/local/go') / name, destination / ('Go-' + name))
    (destination / 'modules.json').write_text(json.dumps(manifest, indent=2) + '\n')


if __name__ == '__main__':
    collect(Path(sys.argv[1]), Path(sys.argv[2]))
