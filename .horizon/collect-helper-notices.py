from pathlib import Path
import json, shutil, tomllib, urllib.request

SUPPLEMENTS = {
    ('profiling', '1.0.18'): ('aclysma/profiling', '8271551172eb6fa4cba47369aedd93790c623df9', ['LICENSE-APACHE', 'LICENSE-MIT']),
    ('profiling-procmacros', '1.0.18'): ('aclysma/profiling', '8271551172eb6fa4cba47369aedd93790c623df9', ['LICENSE-APACHE', 'LICENSE-MIT']),
    ('rmcp', '3.4.1'): ('modelcontextprotocol/rust-sdk', '9427a929959e665e0d12e9395f674026baf4bd48', ['LICENSE']),
    ('rmcp', '3.5.0'): ('modelcontextprotocol/rust-sdk', '0cde3c5cf3e6aff0cc852ce6045f107e95991f48', ['LICENSE']),
    ('rmcp-macros', '3.5.0'): ('modelcontextprotocol/rust-sdk', '0cde3c5cf3e6aff0cc852ce6045f107e95991f48', ['LICENSE']),
    ('rmcp', '3.5.1'): ('modelcontextprotocol/rust-sdk', '79437f291b2c44053d00dcd5db969fd0cca7c887', ['LICENSE']),
    ('rmcp-macros', '3.5.1'): ('modelcontextprotocol/rust-sdk', '79437f291b2c44053d00dcd5db969fd0cca7c887', ['LICENSE']),
}
SUPPLEMENTED_NAMES = {name for name, version in SUPPLEMENTS}


def collect_notices(root, output):
    output.mkdir(parents=True)
    packages = []
    for source in sorted((root / 'registry/src').glob('*/*')) + sorted((root / 'git/checkouts').glob('*/*')):
        manifest = source / 'Cargo.toml'
        if not manifest.is_file():
            continue
        data = tomllib.loads(manifest.read_text()).get('package', {})
        destination = output / source.name
        names = ('license', 'copying', 'notice', 'copyright', 'authors')
        files = [p for p in source.rglob('*') if p.is_file() and p.name.lower().startswith(names)]
        declared = data.get('license-file')
        if isinstance(declared, str) and (source / declared).is_file():
            files.append(source / declared)
        for file in files:
            target = destination / file.relative_to(source)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(file, target)
        provenance = {'package': data, 'source_revision': None}
        vcs = source / '.cargo_vcs_info.json'
        if vcs.is_file():
            provenance['source_revision'] = json.loads(vcs.read_text())
        destination.mkdir(parents=True, exist_ok=True)
        notices = sorted({str(p.relative_to(source)) for p in files})
        if data.get('name') in SUPPLEMENTED_NAMES:
            identity = data['name'], data.get('version')
            supplement = SUPPLEMENTS.get(identity)
            if supplement is None:
                raise RuntimeError(f'No reviewed license supplement for {identity[0]} {identity[1]}')
            repository, revision, filenames = supplement
            source_revision = provenance['source_revision'] or {}
            actual_revision = source_revision.get('git', {}).get('sha1')
            if actual_revision != revision:
                raise RuntimeError(f'License provenance mismatch for {identity[0]} {identity[1]}: '
                                   f'expected {revision}, got {actual_revision}')
            for filename in filenames:
                url = f'https://raw.githubusercontent.com/{repository}/{revision}/{filename}'
                with urllib.request.urlopen(url, timeout=60) as response:
                    (destination / filename).write_bytes(response.read())
                notices.append(filename)
        (destination / 'provenance.json').write_text(json.dumps(provenance, indent=2))
        packages.append({'name': data.get('name', source.name), 'version': data.get('version'),
                         'license': data.get('license'), 'repository': data.get('repository'),
                         'notices': sorted(set(notices))})
    (output / 'packages.json').write_text(json.dumps(packages, indent=2))


if __name__ == '__main__':
    collect_notices(Path('/tmp/helper-cargo'), Path('/output/share/licenses/horizon-dependencies'))
