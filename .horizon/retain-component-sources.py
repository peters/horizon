"""Retain pinned native component source archives alongside the image binaries."""
from pathlib import Path
import hashlib
import json
import shutil
import urllib.request

manifest = Path(__file__).with_name('component-sources.json')
output = Path('/usr/local/share/sources/agent-components')
output.mkdir(parents=True, exist_ok=True)
for component in json.loads(manifest.read_text()):
    filename = component['filename']
    if Path(filename).name != filename or not component['url'].startswith('https://'):
        raise ValueError('Source archive must have a plain filename and HTTPS origin')
    destination = output / filename
    checksum = hashlib.sha256()
    with urllib.request.urlopen(component['url'], timeout=120) as response:
        with destination.open('wb') as archive:
            while chunk := response.read(1024 * 1024):
                checksum.update(chunk)
                archive.write(chunk)
    if checksum.hexdigest() != component['sha256']:
        destination.unlink()
        raise ValueError(f'Source checksum mismatch: {filename}')
shutil.copy2(manifest, output / 'manifest.json')
