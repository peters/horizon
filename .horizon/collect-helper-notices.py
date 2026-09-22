from pathlib import Path
import json, shutil, tomllib, urllib.request
root = Path('/tmp/helper-cargo')
output = Path('/output/share/licenses/horizon-dependencies')
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
    supplements = {
        'profiling': ('aclysma/profiling', '8271551172eb6fa4cba47369aedd93790c623df9', ['LICENSE-APACHE', 'LICENSE-MIT']),
        'profiling-procmacros': ('aclysma/profiling', '8271551172eb6fa4cba47369aedd93790c623df9', ['LICENSE-APACHE', 'LICENSE-MIT']),
        'rmcp': ('modelcontextprotocol/rust-sdk', 'fd7811fdaa9fefa1c8034534b4d7a31c97204f89', ['LICENSE']),
        'rmcp-macros': ('modelcontextprotocol/rust-sdk', 'fd7811fdaa9fefa1c8034534b4d7a31c97204f89', ['LICENSE']),
    }
    provenance = {'package': data, 'source_revision': None}
    vcs = source / '.cargo_vcs_info.json'
    if vcs.is_file():
        provenance['source_revision'] = json.loads(vcs.read_text())
    destination.mkdir(parents=True, exist_ok=True)
    notices = sorted({str(p.relative_to(source)) for p in files})
    if data.get('name') in supplements:
        repository, revision, filenames = supplements[data['name']]
        assert provenance['source_revision']['git']['sha1'] == revision
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
