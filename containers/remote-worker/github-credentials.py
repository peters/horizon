#!/usr/bin/python3 -I
"""Consume the worker's protected runtime token without persisting credentials."""

import os
from pathlib import Path
import re
import stat
import sys
from urllib.parse import urlsplit

RUNTIME = Path('/run/horizon')
GH = '/usr/bin/gh'
MAX_TOKEN = 16384
MAX_REQUEST = 65536
TOKEN_VARIABLES = ('GH_TOKEN', 'GITHUB_TOKEN', 'GH_ENTERPRISE_TOKEN',
                   'GITHUB_ENTERPRISE_TOKEN', 'HORIZON_GITHUB_TOKEN')
DIRECTORY_FLAGS = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_NONBLOCK


class CredentialError(Exception):
    pass


def require(condition):
    if not condition:
        raise CredentialError()


def identity(info):
    return (info.st_dev, info.st_ino, info.st_uid, info.st_gid, info.st_mode,
            info.st_nlink, info.st_size, info.st_mtime_ns, info.st_ctime_ns)


def read_token():
    try:
        directory = os.open(RUNTIME, DIRECTORY_FLAGS)
    except FileNotFoundError:
        return None
    try:
        parent = os.fstat(directory)
        require(parent.st_uid == os.geteuid() and stat.S_IMODE(parent.st_mode) == 0o700)
        try:
            descriptor = os.open('github-token', os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                                 dir_fd=directory)
        except FileNotFoundError:
            return None
        try:
            before = os.fstat(descriptor)
            require(stat.S_ISREG(before.st_mode) and before.st_uid == os.geteuid()
                    and stat.S_IMODE(before.st_mode) == 0o600 and before.st_nlink == 1
                    and 1 <= before.st_size <= MAX_TOKEN)
            raw = os.read(descriptor, MAX_TOKEN + 1)
            require(len(raw) == before.st_size and identity(os.fstat(descriptor)) == identity(before))
            require(identity(os.stat('github-token', dir_fd=directory, follow_symlinks=False))
                    == identity(before))
            require(identity(os.stat(RUNTIME, follow_symlinks=False)) == identity(parent))
        finally:
            os.close(descriptor)
    finally:
        os.close(directory)
    # Fine-grained and classic PATs use this alphabet. Permit one file-ending LF,
    # never whitespace/control characters that could inject credential fields.
    value = raw.removesuffix(b'\n')
    require(re.fullmatch(rb'[A-Za-z0-9_]+', value) is not None)
    return value.decode('ascii')


def credential_request(stream):
    fields = {}
    total = 0
    while True:
        line = stream.readline(MAX_REQUEST + 1)
        total += len(line)
        require(total <= MAX_REQUEST)
        if line in (b'', b'\n'):
            return fields
        require(line.endswith(b'\n') and b'\0' not in line and b'\r' not in line)
        key, separator, value = line[:-1].partition(b'=')
        require(separator and key and key not in fields)
        fields[key] = value


def git_credential(operation, stream, output):
    # Read-only helper: Git's store/erase input must never replace the mounted PAT.
    if operation != 'get':
        return 0
    fields = credential_request(stream)
    if fields.get(b'protocol') != b'https' or fields.get(b'host') != b'github.com':
        return 0
    token = read_token()
    if token is None:
        output.write('quit=true\n\n')
    else:
        output.write('username=x-access-token\npassword=' + token + '\n\n')
    return 0


def gh_environment(environment):
    result = {key: value for key, value in environment.items()
              if key not in TOKEN_VARIABLES and key not in ('GH_DEBUG', 'DEBUG')}
    # No fallback to the user's normal hosts.yml or interactive login. The file
    # above is the only automatically supplied credential source on this worker.
    result.update(GH_CONFIG_DIR='/run/horizon/github-cli', GH_PROMPT_DISABLED='1')
    return result


def api_target(arguments):
    if arguments[:1] != ['api']:
        return
    # Parse only API target/options, never PR/issue text or API field values.
    # Unknown flags are left for gh, which rejects them before making a request.
    valued = {'--hostname', '--cache', '--field', '--raw-field', '--header', '--input',
              '--jq', '--method', '--template', '--preview', '-F', '-f', '-H', '-q', '-X', '-t', '-p'}
    remaining = iter(arguments[1:])
    for argument in remaining:
        flag = argument.split('=', 1)[0]
        if flag in valued:
            value = argument.split('=', 1)[1] if '=' in argument else next(remaining, None)
            require(value is not None)
            if flag == '--hostname':
                require(value == 'github.com')
        elif argument.startswith('-') and argument != '--':
            continue
        else:
            endpoint = next(remaining, '') if argument == '--' else argument
            if '://' in endpoint:
                url = urlsplit(endpoint)
                require(url.scheme == 'https' and url.hostname in ('github.com', 'api.github.com')
                        and url.port in (None, 443) and url.username is None and url.password is None)


def run_gh(arguments):
    environment = gh_environment(os.environ)
    if (arguments in (['--version'], ['version']) or arguments[:1] == ['help']
            or any(flag in arguments for flag in ('--help', '-h'))):
        os.execve(GH, [GH, *arguments], environment)
    require(environment.get('GH_HOST', 'github.com') == 'github.com')
    # Explicit GH_HOST also filters inferred Git remotes, preventing a different
    # host in the working repository from receiving GitHub's environment token.
    environment['GH_HOST'] = 'github.com'
    api_target(arguments)
    token = read_token()
    require(token is not None)
    # A missing config gives packaged gh a valid empty config. Do not admit a
    # corrupt/unreadable config (or a second stored credential source).
    require(not os.path.lexists(environment['GH_CONFIG_DIR']))
    environment['GH_TOKEN'] = token
    os.execve(GH, [GH, *arguments], environment)


def main(arguments):
    try:
        if Path(sys.argv[0]).name == 'gh':
            run_gh(arguments)
            return 0
        if len(arguments) == 1:
            return git_credential(arguments[0], sys.stdin.buffer, sys.stdout)
        raise CredentialError()
    except (OSError, ValueError, CredentialError):
        print('horizon-worker: GitHub credential unavailable or request rejected', file=sys.stderr)
        if Path(sys.argv[0]).name != 'gh' and arguments == ['get']:
            sys.stdout.write('quit=true\n\n')
            return 0
        return 1


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
