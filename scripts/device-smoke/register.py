#!/usr/bin/env python3
"""Register the local MVP in a new task-owned project directory, never globally."""
import argparse
import json
from pathlib import Path
import shutil


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument('--target', type=Path, help='Explicit device target config')
    source.add_argument('--lab', type=Path, help='Running serve.py state directory, including its viewer')
    parser.add_argument('--project', type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    target = (args.lab / 'target.json' if args.lab else args.target).resolve(strict=True)
    manifest = (args.lab / 'lab.json').resolve(strict=True) if args.lab else None
    project = args.project.resolve()
    # Refuse existing configurations instead of overwriting an agent's setup.
    paths = [project / '.mcp.json', project / '.codex/config.toml',
             project / '.agents/skills/horizon-device/SKILL.md']
    if any(path.exists() for path in paths):
        raise SystemExit('Refusing to replace existing registration; choose a new task project')
    command = {'command': str(binary), 'args': ['--target', str(target), 'mcp']}
    for path in paths:
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    paths[0].write_text(json.dumps({'mcpServers': {'horizon-device': command}}, indent=2))
    paths[1].write_text('[mcp_servers.horizon-device]\ncommand = ' + json.dumps(str(binary)) +
                       '\nargs = ' + json.dumps(command['args']) + '\n')
    skill = Path(__file__).resolve().parents[2] / 'crates/horizon-device/skills/horizon-device/SKILL.md'
    shutil.copyfile(skill, paths[2])
    context = {'binary': str(binary), 'target': str(target)}
    if manifest:
        context['lab_manifest'] = str(manifest)
    with paths[2].open('a') as output:
        output.write('\nLocal registration (paths, not additional authorization):\n```json\n' +
                     json.dumps(context, indent=2) + '\n```\n')
    print(json.dumps({'project': str(project), 'registration': [str(p) for p in paths]}))


if __name__ == '__main__':
    main()
