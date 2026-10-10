#!/usr/bin/env python3
"""Check shipped MCP tool coverage, skill parity, references, and embedded assets."""
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
SOURCES = {
    'crates/horizon-browser-mcp/src/server.rs': 'horizon-browser',
    'crates/horizon-device/src/cli/mcp.rs': 'horizon-device',
    'crates/horizon-app-host/src/mcp/mod.rs': 'horizon-app-testing',
    'crates/horizon-cloud-worker/src/companion_tools/mcp.rs': 'horizon-cloud',
    'crates/horizon-cloud-worker/src/local_network/mcp.rs': 'horizon-cloud',
}
TOOL = re.compile(r'#\[tool\(\s*name\s*=\s*"([^"]+)"')
OPERATIONS = [
    ('crates/horizon-browser-control/src/manifest/cast.rs', 'CastOperation', 'horizon-cast', set(), 'cast'),
    ('crates/horizon-browser-control/src/manifest/device.rs', 'Operation', 'horizon-device', {'BrowserScreenshot'}, 'device_panel'),
    ('crates/horizon-browser-control/src/manifest/device.rs', 'VideoAction', 'horizon-device', set(), 'device_panel video'),
    ('crates/horizon-browser-mcp/src/model.rs', 'ActKind', 'horizon-browser', set(), 'browser_act'),
    ('crates/horizon-browser-mcp/src/model/network.rs', 'NetworkOperation', 'horizon-browser', set(), 'browser_network'),
    ('crates/horizon-browser-mcp/src/model/video.rs', 'VideoOperation', 'horizon-browser', set(), 'browser_video'),
    ('crates/horizon-browser-mcp/src/model/http_auth.rs', 'HttpAuthOperation', 'horizon-browser', set(), 'browser_http_auth'),
    ('crates/horizon-browser-mcp/src/model/recovery.rs', 'RecoveryOperation', 'horizon-browser', set(), 'browser_remote_allocations'),
    ('crates/horizon-app-host/src/mcp/model.rs', 'VideoOperation', 'horizon-app-testing', set(), 'app_video'),
    ('crates/horizon-app-testing/src/recipe.rs', 'Action', 'horizon-app-testing', set(), 'native recipe'),
    ('crates/horizon-device/src/model.rs', 'Action', 'horizon-device', set(), 'device_act'),
]


def enum_variants(source, name):
    start = re.search(r'\benum\s+' + re.escape(name) + r'\s*\{', source)
    if start is None:
        raise ValueError(f'enum {name} not found')
    # Each variant starts with an upper-case name at the outer body depth.
    body = source[start.end():]
    depth = 1
    variants = []
    for line in body.splitlines():
        stripped = line.strip()
        if depth == 1:
            variant = re.match(r'([A-Z]\w*)\s*(?:[,({]|$)', stripped)
            if variant:
                variants.append(variant.group(1))
        # Enum fields in these declarations do not contain braces in strings.
        if not stripped.startswith('//'):
            depth += line.count('{') - line.count('}')
        if depth == 0:
            break
    return variants


def snake_case(name):
    return re.sub(r'(?<!^)(?=[A-Z])', '_', name).lower()


def check():
    errors = []
    codex = ROOT / 'assets/plugins/codex/skills'
    claude = ROOT / 'assets/plugins/claude-code/skills'
    build = (ROOT / 'crates/horizon-ui/build.rs').read_text()
    installed = (ROOT / 'crates/horizon-ui/src/plugin_install/mod.rs').read_text() + (ROOT / 'crates/horizon-ui/src/plugin_install/mcp_skills.rs').read_text()
    documents = {}
    primary_skills = {p.name for p in codex.iterdir() if p.is_dir()}
    other_skills = {p.name for p in claude.iterdir() if p.is_dir()}
    if primary_skills != other_skills:
        errors.append('bundle skill sets differ: ' + ', '.join(sorted(primary_skills ^ other_skills)))
    for skill in sorted(codex.iterdir()):
        if not skill.is_dir():
            continue
        counterpart = claude / skill.name
        files = {f.relative_to(skill) for f in skill.rglob('*') if f.is_file()}
        other_files = {f.relative_to(counterpart) for f in counterpart.rglob('*') if f.is_file()}
        if files != other_files:
            errors.append(f'{skill.name}: bundle file sets differ')
        documents[skill.name] = '\n'.join((skill / f).read_text() for f in sorted(files) if f.suffix == '.md')
        for relative in files:
            source = skill / relative
            other = counterpart / relative
            if not other.exists() or source.read_bytes() != other.read_bytes():
                errors.append(f'{skill.name}/{relative}: bundle contents differ')
            for provider in ('codex', 'claude-code'):
                asset = f'plugins/{provider}/skills/{skill.name}/{relative.as_posix()}'
                if f'"{asset}"' not in build:
                    errors.append(f'{asset}: missing build asset')
            if f'/assets/plugins/codex/skills/{skill.name}/{relative.as_posix()}' not in installed:
                errors.append(f'{skill.name}/{relative}: missing installer embedding')
        reachable = {Path('SKILL.md')}
        pending = list(reachable)
        while pending:
            relative = pending.pop()
            for link in re.findall(r'\]\(([^)]+)\)', (skill / relative).read_text()):
                if ':' in link or link.startswith('#'):
                    continue
                target = (skill / relative.parent / link.split('#')[0]).resolve()
                if not target.is_relative_to(skill.resolve()) or not target.is_file():
                    errors.append(f'{skill.name}/{relative}: broken reference {link}')
                elif target.suffix == '.md':
                    next_relative = target.relative_to(skill.resolve())
                    if next_relative not in reachable:
                        reachable.add(next_relative)
                        pending.append(next_relative)
        for relative in files:
            if relative.suffix == '.md' and relative not in reachable:
                errors.append(f'{skill.name}/{relative}: unreachable reference')
    count = 0
    discovered_sources = set()
    for source in (ROOT / 'crates').rglob('*.rs'):
        if 'examples' in source.parts:
            continue
        names = TOOL.findall(source.read_text())
        if not names:
            continue
        relative = source.relative_to(ROOT).as_posix()
        discovered_sources.add(relative)
        if relative not in SOURCES:
            errors.append(f'{relative}: MCP server has no skill route')
            continue
        for name in names:
            skill = SOURCES[relative]
            if name == 'cast':
                skill = 'horizon-cast'
            elif name == 'device_panel':
                skill = 'horizon-device'
            elif name.startswith('cloud_'):
                skill = 'horizon-cloud'
            if name not in documents.get(skill, ''):
                errors.append(f'{relative}: {name} missing from {skill}')
            count += 1
    for source in SOURCES.keys() - discovered_sources:
        errors.append(f'{source}: coverage route no longer names an MCP server')
    for term in ('Device name', 'MagicDNS', 'horizon-tailnet-contract=2', 'Connect again'):
        if term not in documents.get('horizon-cloud', ''):
            errors.append(f'horizon-cloud: tailnet device identity guidance missing {term}')
    cast_document = documents.get('horizon-cast', '')
    for name in enum_variants((ROOT / 'crates/horizon-browser-control/src/manifest/cast.rs').read_text(), 'CastSource'):
        if f'`kind: {snake_case(name)}`' not in cast_document:
            errors.append(f'CastSource.{name}: source kind missing from horizon-cast')
    for source, enum, skill, excluded, api in OPERATIONS:
        declarations = re.findall(r'`' + re.escape(api) + r'` operations are ([^.]+)\.', documents[skill])
        if len(declarations) != 1:
            errors.append(f'{api}: expected one qualified operation list in {skill}')
            continue
        documented = set(re.findall(r'`([a-z_]+)`', declarations[0]))
        for name in enum_variants((ROOT / source).read_text(), enum):
            if name not in excluded and snake_case(name) not in documented:
                errors.append(f'{enum}.{name}: operation missing from {skill}')
    standalone = ROOT / 'crates/horizon-device/skills/horizon-device/SKILL.md'
    if standalone.read_bytes() != (codex / 'horizon-device/SKILL.md').read_bytes():
        errors.append('standalone device skill differs from bundled skill')
    if errors:
        print('\n'.join(errors), file=sys.stderr)
        return 1
    print(f'Horizon skill coverage: {count} tool registrations, {len(SOURCES)} servers, {len(documents)} skills; parity, references, and embedding pass')
    return 0


if __name__ == '__main__':
    sys.exit(check())
