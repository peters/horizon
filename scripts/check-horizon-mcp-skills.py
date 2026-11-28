#!/usr/bin/env python3
"""Check shipped MCP tool coverage, skill parity, references, and embedded assets."""
from pathlib import Path
import json
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
SOURCES = {
    'crates/horizon-browser-mcp/src/server.rs': 'horizon-browser',
    'crates/horizon-device/src/cli/mcp.rs': 'horizon-device',
}
TOOL = re.compile(r'#\[tool\(\s*name\s*=\s*"([^"]+)"')
OPERATIONS = [
    ('crates/horizon-browser-control/src/manifest/block.rs', 'Operation', 'horizon-block', set(), 'block_panel'),
    ('crates/horizon-browser-control/src/manifest/device.rs', 'Operation', 'horizon-device', {'BrowserScreenshot'}, 'device_panel'),
    ('crates/horizon-browser-control/src/manifest/device.rs', 'VideoAction', 'horizon-device', set(), 'device_panel video'),
    ('crates/horizon-browser-mcp/src/model.rs', 'ActKind', 'horizon-browser', set(), 'browser_act'),
    ('crates/horizon-browser-mcp/src/model/network.rs', 'NetworkOperation', 'horizon-browser', set(), 'browser_network'),
    ('crates/horizon-browser-mcp/src/model/video.rs', 'VideoOperation', 'horizon-browser', set(), 'browser_video'),
    ('crates/horizon-browser-mcp/src/model/http_auth.rs', 'HttpAuthOperation', 'horizon-browser', set(), 'browser_http_auth'),
    ('crates/horizon-browser-mcp/src/model/recovery.rs', 'RecoveryOperation', 'horizon-browser', set(), 'browser_remote_allocations'),
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


# The operations whose fenced examples are checked against their own fields.
# A skill example is a request that a caller sends, so an example of the skill
# can name an operation and no other field than the ones that operation has.
EXAMPLE_OPERATIONS = {
    'block_panel': (
        'crates/horizon-browser-control/src/manifest/block.rs',
        'Operation',
        'horizon-block',
    ),
}
# The skills that this build installs, by name. A skill that vanishes from a
# bundle, or one that arrives without its name here, fails the check.
REQUIRED_SKILLS = {
    'horizon-block',
    'horizon-browser',
    'horizon-device',
    'horizon-front-desk',
    'horizon-machines',
    'horizon-mobile',
    'horizon-plan',
    'horizon-server',
    'horizon-server-move',
    'horizon-setup',
    'horizon-speech',
    'horizon-worktree',
}
# The operations of the block tool that ask the person for a setting of this
# computer: each one names its card and the two answers of the person.
REQUIRED_SETTING_OPERATIONS = ('server_link_request', 'browser_host_request')
# The nested object that an operation field carries, with the struct of its
# fields, so an example of a route is checked like an operation.
NESTED_FIELDS = {
    ('block_panel', 'ssh'): ('crates/horizon-browser-control/src/manifest/block_terminal.rs', 'TerminalRoute'),
}
EXAMPLE = re.compile(r'```json\n(.*?)```', re.S)
# The top-level fields of a card document. The skill horizon-block names each of
# them, and a card example of every skill names no other field.
CARD_DOCUMENT = ('crates/horizon-browser-control/src/block/model.rs', 'BlockDocument')
# The buttons of a machine card, in the order of the skill horizon-machines. A card
# example that offers one of them offers all eight, so a set-up continues from the
# card in every skill that draws it.
MACHINE_CARD_BUTTONS = (
    'connect',
    'server_here',
    'browser_here',
    'join_network',
    'exact_state',
    'cards_here',
    'stop',
    'delete',
)
# The button that starts an agent program of the machine, one for each program that
# the machine has. It stands directly after `Connect`, before the server button.
MACHINE_CARD_AGENT_BUTTON = re.compile(r'start_[a-z0-9]+_here$')


def variant_fields(source, name):
    """The top-level field names of each variant of an externally tagged enum."""
    start = re.search(r'\benum\s+' + re.escape(name) + r'\s*\{', source)
    if start is None:
        raise ValueError(f'enum {name} not found')
    fields = {}
    current = None
    depth = 1
    for line in source[start.end():].splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith(('//', '#')):
            continue
        if depth == 1:
            variant = re.match(r'([A-Z]\w*)\s*(?:[,({]|$)', stripped)
            if variant:
                current = snake_case(variant.group(1))
                fields[current] = set()
                # A variant whose fields fit on its own line.
                if '{' in stripped and '}' in stripped:
                    inside = stripped.split('{', 1)[1].rsplit('}', 1)[0]
                    for part in inside.split(','):
                        field = re.match(r'\s*([a-z_]\w*)\s*:', part)
                        if field:
                            fields[current].add(field.group(1))
        elif depth == 2 and current is not None:
            field = re.match(r'([a-z_]\w*)\s*:', stripped)
            if field:
                fields[current].add(field.group(1))
        depth += stripped.count('{') - stripped.count('}')
        if depth <= 1:
            current = None
        if depth == 0:
            break
    return fields


def struct_fields(source, name):
    """The field names of a plain struct."""
    start = re.search(r'\bstruct\s+' + re.escape(name) + r'\s*\{', source)
    if start is None:
        raise ValueError(f'struct {name} not found')
    fields = set()
    for line in source[start.end():].split('\n}')[0].splitlines():
        stripped = line.strip()
        if stripped.startswith(('//', '#')):
            continue
        field = re.match(r'(?:pub\s+)?([a-z_]\w*)\s*:', stripped)
        if field:
            fields.add(field.group(1))
    return fields


def example_errors(documents):
    """Every fenced JSON example names only fields of its operation schema."""
    errors = []
    for api, (source_name, enum, skill) in EXAMPLE_OPERATIONS.items():
        fields = variant_fields((ROOT / source_name).read_text(), enum)
        nested = {
            field: struct_fields((ROOT / nested_source).read_text(), nested_name)
            for (nested_api, field), (nested_source, nested_name) in NESTED_FIELDS.items()
            if nested_api == api
        }
        for block in EXAMPLE.findall(documents.get(skill, '')):
            try:
                value = json.loads(block)
            except json.JSONDecodeError:
                errors.append(f'{skill}: a fenced JSON example does not parse: {block.strip()[:60]}')
                continue
            if not isinstance(value, dict) or 'operation' not in value:
                continue
            operation = value['operation']
            if operation not in fields:
                errors.append(f'{skill}: an example names no operation of {api}: {operation}')
                continue
            allowed = ', '.join(sorted(fields[operation] | {'operation'}))
            for key, entry in value.items():
                if key not in fields[operation] and key != 'operation':
                    errors.append(f'{skill}: the {operation} example of {api} names {key}; the schema has {allowed}')
                if key in nested and isinstance(entry, dict):
                    for nested_key in entry:
                        if nested_key not in nested[key]:
                            names = ', '.join(sorted(nested[key]))
                            errors.append(
                                f'{skill}: the {operation} example of {api} names {key}.{nested_key}; '
                                f'the schema has {names}'
                            )
    return errors


def card_document_errors(documents):
    """The skill horizon-block names every top-level field of a card document, and a
    card example of every skill names only those fields."""
    fields = struct_fields((ROOT / CARD_DOCUMENT[0]).read_text(), CARD_DOCUMENT[1])
    block = documents.get('horizon-block', '')
    errors = [
        f'{name}: the field of a card document is missing from horizon-block'
        for name in sorted(fields)
        if f'`{name}`' not in block and f'`"{name}"' not in block
    ]
    for skill, document in sorted(documents.items()):
        for example in EXAMPLE.findall(document):
            try:
                value = json.loads(example)
            except json.JSONDecodeError:
                continue
            card = value.get('document', value) if isinstance(value, dict) else None
            if not isinstance(card, dict) or 'components' not in card:
                continue
            for key in sorted(card.keys() - fields):
                names = ', '.join(sorted(fields))
                errors.append(f'{skill}: a card example names the field {key}; a card document has {names}')
    return errors


def machine_card_errors(documents):
    """Every card example with a machine button offers all eight buttons, in the
    order of the skill, and the buttons that start an agent program of the machine
    directly after `Connect`."""
    errors = []
    for skill, document in sorted(documents.items()):
        for block in EXAMPLE.findall(document):
            try:
                value = json.loads(block)
            except json.JSONDecodeError:
                continue
            if not isinstance(value, dict):
                continue
            components = value.get('components')
            if not isinstance(components, list):
                continue
            ids = [
                item.get('id')
                for component in components
                if isinstance(component, dict)
                for item in component.get('items') or []
                if isinstance(item, dict)
            ]
            if 'server_here' not in ids:
                continue
            missing = [name for name in MACHINE_CARD_BUTTONS if name not in ids]
            if missing:
                errors.append(f'{skill}: a machine card example lacks the buttons {", ".join(missing)}')
                continue
            order = [name for name in ids if name in MACHINE_CARD_BUTTONS]
            if order != list(MACHINE_CARD_BUTTONS):
                errors.append(
                    f'{skill}: a machine card example draws the buttons as {", ".join(order)}; '
                    f'the skill draws {", ".join(MACHINE_CARD_BUTTONS)}'
                )
            between = ids[ids.index('connect') + 1 : ids.index('server_here')]
            strangers = [name for name in between if not (name and MACHINE_CARD_AGENT_BUTTON.match(name))]
            if strangers:
                errors.append(
                    f'{skill}: a machine card example draws {", ".join(strangers)} between Connect and '
                    'the server button; only the buttons that start an agent program stand there'
                )
    return errors


def check():
    errors = []
    codex = ROOT / 'assets/plugins/codex/skills'
    claude = ROOT / 'assets/plugins/claude-code/skills'
    build = (ROOT / 'crates/horizon-ui/build.rs').read_text()
    # The installer writes the bundled files from the embedded lists, which the
    # module of the lists and the installer hold between them.
    installed = '\n'.join(
        path.read_text() for path in sorted((ROOT / 'crates/horizon-ui/src/plugin_install').glob('*.rs'))
    )
    documents = {}
    primary_skills = {p.name for p in codex.iterdir() if p.is_dir()}
    other_skills = {p.name for p in claude.iterdir() if p.is_dir()}
    if primary_skills != other_skills:
        errors.append('bundle skill sets differ: ' + ', '.join(sorted(primary_skills ^ other_skills)))
    if primary_skills != REQUIRED_SKILLS:
        errors.append('bundle skills differ from this build: ' + ', '.join(sorted(primary_skills ^ REQUIRED_SKILLS)))
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
            if name == 'device_panel':
                skill = 'horizon-device'
            if name == 'block_panel':
                skill = 'horizon-block'
            if name not in documents.get(skill, ''):
                errors.append(f'{relative}: {name} missing from {skill}')
            count += 1
    for source in SOURCES.keys() - discovered_sources:
        errors.append(f'{source}: coverage route no longer names an MCP server')
    for source, enum, skill, excluded, api in OPERATIONS:
        declarations = re.findall(r'`' + re.escape(api) + r'` operations are ([^.]+)\.', documents[skill])
        if len(declarations) != 1:
            errors.append(f'{api}: expected one qualified operation list in {skill}')
            continue
        documented = set(re.findall(r'`([a-z_]+)`', declarations[0]))
        for name in enum_variants((ROOT / source).read_text(), enum):
            if name not in excluded and snake_case(name) not in documented:
                errors.append(f'{enum}.{name}: operation missing from {skill}')
    block_document = documents.get('horizon-block', '')
    for name in REQUIRED_SETTING_OPERATIONS:
        if name not in block_document:
            errors.append(f'{name}: the setting operation missing from horizon-block')
        if name not in documents.get('horizon-setup', ''):
            errors.append(f'{name}: the setting operation missing from horizon-setup')
    errors.extend(example_errors(documents))
    errors.extend(machine_card_errors(documents))
    errors.extend(card_document_errors(documents))
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
