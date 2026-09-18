#!/usr/bin/env python3
"""Verify permission changes across a running MCP session and separate CLI calls."""
import argparse
import json
import subprocess
import tempfile
from pathlib import Path
from client import Client


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='resize-permission-') as directory:
        target = Path(directory) / 'target.json'
        base = {'id': 'fixture', 'endpoint': {'kind': 'local_x11', 'display': ':999999'},
                'desktop_resize': {'policy': {'max_width': 1920, 'max_height': 1080, 'max_pixels': 2073600}}}
        target.write_text(json.dumps(base))
        client = Client(args.binary, target, 'mcp')
        try:
            pid = client.process.pid
            enabled = client.call('set_resize_enabled', {'enabled': True})
            assert enabled == dict(base['desktop_resize']['policy'], enabled=True), enabled
            saved = json.loads(target.read_text())
            assert saved['desktop_resize']['policy']['enabled'] is True
            result = subprocess.run([str(args.binary), '--target', str(target), '--resize-enabled', 'false'],
                                    capture_output=True, text=True, timeout=10)
            assert result.returncode == 0, result
            assert json.loads(target.read_text())['desktop_resize']['policy']['enabled'] is False
            # Denial before device access proves this same MCP server reloaded the CLI change.
            try:
                client.call('resize', {'width': 1280, 'height': 720})
            except RuntimeError as error:
                assert error.args[0]['error']['code'] == 'resize_denied', error
            else:
                raise AssertionError('disabled resize was accepted')
            client.call('set_resize_enabled', {'enabled': True})
            for suffix in ['resize-pending', 'resize-observe']:
                target.with_name(target.name + '.' + suffix).write_text('retained')
            client.call('set_resize_enabled', {'enabled': False})
            for suffix in ['resize-pending', 'resize-observe']:
                assert target.with_name(target.name + '.' + suffix).read_text() == 'retained'
            assert client.process.pid == pid and client.process.poll() is None
            print('PASS runtime CLI/MCP permission changes without restart; limits and journals preserved')
        finally:
            client.close()


if __name__ == '__main__':
    main()
