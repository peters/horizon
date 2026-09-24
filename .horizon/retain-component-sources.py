"""Retain the corresponding sources and notices of the Codex release in the image.

A requested release's sources are derived from its own tagged tree: the voice
library list, the libcap release its sandbox links statically, and the ripgrep
and zsh revisions it bundles. When that tree no longer has this layout, the last
reviewed release is retained instead, so Codex never ships without its matching
sources. Checksum mismatches and download failures stop the build. Prints the
Codex version whose sources were retained; progress goes to standard error.
"""
from dataclasses import dataclass
from pathlib import Path
import hashlib
import json
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

REPOSITORY = 'https://github.com/openai/codex'
SOURCES = Path('/usr/local/share/sources/agent-components')
NOTICES = Path('/usr/local/share/licenses/agent-client')
CORRESPONDING_SOURCE = 'agent-corresponding-source'

# The last release whose layout, sources and notices were reviewed by hand. Builds
# that request no release, or whose release cannot be derived, retain this one.
PINNED_VERSION = '0.155.1'
PINNED_COMMIT = 'be2951ea34f0d295ed0becf97079f92fa5f6950e'
PINNED_RIPGREP = '15.2.0'
PINNED_ZSH = '77045ef899e53b9598bebc5a41db93a548a40ca6'
PINNED_MANIFEST = Path(__file__).with_name('component-sources.json')
# Reviewed checksums of notices fetched outside the source archive.
NOTICE_SHA256 = {
    'https://raw.githubusercontent.com/BurntSushi/ripgrep/15.2.0/COPYING':
        '01c266bced4a434da0051174d6bee16a4c82cf634e2679b6155d40d75012390f',
    'https://raw.githubusercontent.com/BurntSushi/ripgrep/15.2.0/LICENSE-MIT':
        '0f96a83840e146e43c0ec96a22ec1f392e0680e6c1226e6f3ba87e0740af850f',
    'https://raw.githubusercontent.com/BurntSushi/ripgrep/15.2.0/UNLICENSE':
        '7e12e5df4bae12cb21581ba157ced20e1986a0508dd10d0e8a4ab9a4cf94e85c',
    'https://raw.githubusercontent.com/zsh-users/zsh/77045ef899e53b9598bebc5a41db93a548a40ca6/LICENCE':
        'd06fdf3ef9b1ec69d6b9e170b0a9516fbad3523261ff1668bde3bfea6e0ef5f5',
}

TREE_NOTICES = {
    'LICENSE': 'LICENSE',
    'NOTICE': 'NOTICE',
    'codex-rs/vendor/bubblewrap/COPYING': 'bundled/agent-bubblewrap-COPYING',
    'third_party/wezterm/LICENSE': 'bundled/agent-wezterm-LICENSE',
}
VOICE_SOURCES = 'third_party/voice/sources.json'
LIBCAP_SCRIPT = '.github/scripts/install-musl-build-tools.sh'
RIPGREP_MANIFEST = 'scripts/codex_package/rg'
ZSH_WORKFLOW = '.github/workflows/rust-release-zsh.yml'
LAYOUT = (VOICE_SOURCES, LIBCAP_SCRIPT, RIPGREP_MANIFEST, ZSH_WORKFLOW, *TREE_NOTICES)

# Semantic Versioning 2.0.0, as Horizon's release lookup accepts it: no leading zeros
# in numeric core or pre-release identifiers, no empty identifiers.
VERSION = re.compile(
    r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)'
    r'(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(\.(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?'
    r'(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?')
COMMIT = re.compile(r'[0-9a-f]{40}')
SHA256 = re.compile(r'[0-9a-f]{64}')


class LayoutChanged(Exception):
    """The release tree lacks a file or value that the derivation reads."""


@dataclass(frozen=True)
class Release:
    version: str
    commit: str
    ripgrep: str
    zsh: str
    components: list
    manifest: bytes
    archive: Path | None = None  # Corresponding source already fetched while deriving.


def log(message):
    print(message, file=sys.stderr, flush=True)


def download(url, destination, expected=None):
    """Stream url to destination and return its sha256, removing it on a mismatch."""
    checksum = hashlib.sha256()
    with urllib.request.urlopen(url, timeout=120) as response, destination.open('wb') as output:
        while chunk := response.read(1024 * 1024):
            checksum.update(chunk)
            output.write(chunk)
    if expected is not None and checksum.hexdigest() != expected:
        destination.unlink()
        raise ValueError(f'Checksum mismatch for {url}: expected {expected}, got {checksum.hexdigest()}')
    return checksum.hexdigest()


def tag_commit(version):
    tag = f'refs/tags/rust-v{version}'
    result = subprocess.run(['git', 'ls-remote', '--exit-code', '--tags', REPOSITORY, tag, tag + '^{}'],
                            stdout=subprocess.PIPE, text=True, timeout=120)
    if result.returncode == 2:
        raise LayoutChanged(f'tag rust-v{version} not found')
    result.check_returncode()
    refs = {ref: commit for commit, ref in (line.split('\t') for line in result.stdout.splitlines())}
    # An annotated tag names a tag object; its peeled entry names the commit.
    commit = refs.get(tag + '^{}', refs.get(tag, ''))
    if not COMMIT.fullmatch(commit):
        raise LayoutChanged(f'tag rust-v{version} does not name a commit')
    return commit


def tree_files(archive, paths):
    """Regular files at the given tree paths, below the archive's top-level directory."""
    found = {}
    with tarfile.open(archive, 'r:gz') as tar:
        for member in tar:
            path = member.name.partition('/')[2]
            if path in paths and member.isfile():
                found[path] = tar.extractfile(member).read()
    return found


def text(files, path):
    try:
        return files[path].decode('utf-8')
    except UnicodeDecodeError as error:
        raise LayoutChanged(f'{path} is not UTF-8') from error


def voice_components(files):
    try:
        voice = json.loads(text(files, VOICE_SOURCES))
    except ValueError as error:
        raise LayoutChanged(f'{VOICE_SOURCES} is not JSON') from error
    if not isinstance(voice, dict) or voice.get('schema_version') != 1:
        raise LayoutChanged(f'{VOICE_SOURCES} schema_version is not 1')
    try:
        return [{'name': source['name'], 'version': source['version'], 'role': source['role'],
                 'url': source['url'], 'filename': source['archive'], 'sha256': source['sha256']}
                for source in voice['sources']]
    except (KeyError, TypeError) as error:
        raise LayoutChanged(f'{VOICE_SOURCES} sources lack a field') from error


def libcap_component(files):
    values = dict(re.findall(r'^libcap_(version|sha256)="([0-9A-Za-z.]+)"$', text(files, LIBCAP_SCRIPT), re.M))
    if set(values) != {'version', 'sha256'}:
        raise LayoutChanged(f'{LIBCAP_SCRIPT} lacks libcap_version or libcap_sha256')
    version = values['version']
    return {'name': 'libcap', 'version': version, 'role': 'bubblewrap-statically-linked-native-library',
            'url': f'https://mirrors.edge.kernel.org/pub/linux/libs/security/linux-privs/libcap2/libcap-{version}.tar.xz',
            'filename': f'libcap-{version}.tar.xz', 'sha256': values['sha256']}


def single(values, description):
    if len(set(values)) != 1:
        raise LayoutChanged(f'no single {description}')
    return values[0]


def check(components):
    """Every archive needs a plain filename, an HTTPS origin and a sha256 to verify."""
    for component in components:
        if (not all(isinstance(value, str) for value in component.values())
                or Path(component['filename']).name != component['filename'] or component['filename'] in {'', '.', '..', 'manifest.json'}
                or not component['url'].startswith('https://') or not SHA256.fullmatch(component['sha256'])):
            raise LayoutChanged(f'unusable source entry {component["name"]!r}')
    if len({component['filename'] for component in components}) != len(components):
        raise LayoutChanged('source archives share a filename')


def derive(version, commit, archive, checksum):
    """The release's source manifest and bundled revisions, read from its source archive."""
    files = tree_files(archive, LAYOUT)
    missing = [path for path in LAYOUT if path not in files]
    if missing:
        raise LayoutChanged('missing ' + ', '.join(missing))
    components = voice_components(files)
    components.append({'name': CORRESPONDING_SOURCE, 'version': commit,
                       'role': 'native-build-scripts-and-bubblewrap-source',
                       'url': f'https://codeload.github.com/openai/codex/tar.gz/{commit}',
                       'filename': f'agent-source-{commit}.tar.gz', 'sha256': checksum})
    components.append(libcap_component(files))
    check(components)
    ripgrep = single(re.findall(r'/BurntSushi/ripgrep/releases/download/([0-9][0-9A-Za-z.-]*)/',
                                text(files, RIPGREP_MANIFEST)), 'ripgrep release in ' + RIPGREP_MANIFEST)
    zsh = single(re.findall(r'^\s*ZSH_COMMIT:\s*["\']?([0-9a-f]{40})["\']?\s*$', text(files, ZSH_WORKFLOW), re.M),
                 'ZSH_COMMIT in ' + ZSH_WORKFLOW)
    manifest = (json.dumps(components, indent=2) + '\n').encode()
    return Release(version, commit, ripgrep, zsh, components, manifest, archive)


def requested_release(version, scratch):
    commit = tag_commit(version)
    archive = scratch / 'agent-source.tar.gz'
    checksum = download(f'https://codeload.github.com/openai/codex/tar.gz/{commit}', archive)
    return derive(version, commit, archive, checksum)


def pinned_release():
    manifest = PINNED_MANIFEST.read_bytes()
    components = json.loads(manifest)
    check(components)
    return Release(PINNED_VERSION, PINNED_COMMIT, PINNED_RIPGREP, PINNED_ZSH, components, manifest)


def retain(release):
    SOURCES.mkdir(parents=True, exist_ok=True)
    for component in release.components:
        destination = SOURCES / component['filename']
        if component['name'] == CORRESPONDING_SOURCE:
            source_archive = destination
            if release.archive is not None:
                shutil.move(release.archive, destination)
                continue
        download(component['url'], destination, component['sha256'])
    (SOURCES / 'manifest.json').write_bytes(release.manifest)
    (NOTICES / 'bundled').mkdir(parents=True, exist_ok=True)
    notices = tree_files(source_archive, TREE_NOTICES)
    for path, name in TREE_NOTICES.items():
        (NOTICES / name).write_bytes(notices[path])
    (NOTICES / 'SOURCE').write_text(f'{REPOSITORY}/tree/{release.commit}\n')
    upstream = [(f'https://raw.githubusercontent.com/BurntSushi/ripgrep/{release.ripgrep}/{name}', f'rg-{name}')
                for name in ('COPYING', 'LICENSE-MIT', 'UNLICENSE')]
    upstream.append((f'https://raw.githubusercontent.com/zsh-users/zsh/{release.zsh}/LICENCE', 'zsh-LICENCE'))
    for url, name in upstream:
        download(url, NOTICES / 'bundled' / name, NOTICE_SHA256.get(url))


def main(argv):
    requested = argv[1] if len(argv) > 1 else ''
    if requested and not VERSION.fullmatch(requested):
        raise SystemExit(f'Invalid Codex version {requested!r}')
    with tempfile.TemporaryDirectory() as scratch:
        release = None
        if not requested:
            log(f'No Codex version requested; retaining the reviewed Codex {PINNED_VERSION}.')
        else:
            try:
                release = requested_release(requested, Path(scratch))
            except LayoutChanged as error:
                log(f'WARNING: the Codex {requested} source layout changed ({error}). Installing the last '
                    f'verified Codex {PINNED_VERSION} instead; review the pin in .horizon/retain-component-sources.py.')
        release = release or pinned_release()
        log(f'Retaining Codex {release.version} sources from {REPOSITORY}/tree/{release.commit}')
        retain(release)
    print(release.version)


if __name__ == '__main__':
    main(sys.argv)
