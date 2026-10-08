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
    ('crates/horizon-browser-control/src/manifest/cast.rs', 'CastOperation', 'horizon-cast', set()),
    ('crates/horizon-browser-control/src/manifest/device.rs', 'Operation', 'horizon-device', {'BrowserScreenshot'}),
    ('crates/horizon-browser-control/src/manifest/device.rs', 'VideoAction', 'horizon-device', set()),
    ('crates/horizon-browser-mcp/src/model.rs', 'ActKind', 'horizon-browser', set()),
    ('crates/horizon-browser-mcp/src/model/network.rs', 'NetworkOperation', 'horizon-browser', set()),
    ('crates/horizon-browser-mcp/src/model/video.rs', 'VideoOperation', 'horizon-browser', set()),
    ('crates/horizon-browser-mcp/src/model/http_auth.rs', 'HttpAuthOperation', 'horizon-browser', set()),
    ('crates/horizon-browser-mcp/src/model/recovery.rs', 'RecoveryOperation', 'horizon-browser', set()),
    ('crates/horizon-app-host/src/mcp/model.rs', 'VideoOperation', 'horizon-app-testing', set()),
    ('crates/horizon-app-testing/src/recipe.rs', 'Action', 'horizon-app-testing', set()),
    ('crates/horizon-device/src/model.rs', 'Action', 'horizon-device', set()),
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
    for source, enum, skill, excluded in OPERATIONS:
        for name in enum_variants((ROOT / source).read_text(), enum):
            if name not in excluded and not re.search(r'\b' + re.escape(snake_case(name)) + r'\b', documents[skill]):
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
