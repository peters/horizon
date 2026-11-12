#!/usr/bin/env python3
"""Drive one app on one real phone of a device cloud, one step at a time.

An agent runs this tool from a shell. Each call does one thing and prints one
JSON object, so the agent looks at the screen, decides, and acts again.

The tool speaks plain W3C WebDriver (Appium) and the REST API of the device cloud
over HTTPS, with the standard library only. The user name and the access key come
from the environment (BROWSERSTACK_USERNAME, BROWSERSTACK_ACCESS_KEY) and are
never printed.

Commands:
  devices [--platform android|ios] [--search TEXT]   list real devices
  upload PATH_OR_URL                                 upload an .apk/.aab/.ipa, prints an app id
  start --platform android|ios --device NAME --os-version V --app bs://ID [--name TEXT]
  screen [--all]                                     the texts and controls of the screen
  tap (--text TEXT | --id ID | --xpath XPATH | --at X,Y)
  type --text TEXT [--field N | --into-text LABEL | --into-id ID | --into-xpath XPATH] [--enter]
                                                     with no target: the first text field
  swipe up|down|left|right                           hides the keyboard, then swipes
  hide-keyboard                                      closes the on-screen keyboard
  wait --text TEXT [--seconds N]                     waits until a text is on the screen
  back                                               Android back key
  open                                               brings the app under test to the front
  screenshot PATH.png
  status                                             session state and the watch link
  stop [--passed | --failed] [--reason TEXT]          ends the session and prints the recording link

The session id is kept in ./.mobile-session.json, which is mode 0600, so the next
call continues the same session. `stop` ends the session and deletes that file. A
session that gets no command for 90 seconds is ended by the device cloud.
"""
from __future__ import annotations

import argparse
import base64
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
import xml.etree.ElementTree as ElementTree

# The two base addresses of the provider. A test of this tool sets both to a fake
# on a loopback port; no other caller sets them.
DEFAULT_HUB = 'https://hub-cloud.browserstack.com/wd/hub'
DEFAULT_API = 'https://api-cloud.browserstack.com/app-automate'

STATE = '.mobile-session.json'
ELEMENT_KEY = 'element-6066-11e4-a52e-4f735466cecf'
MAX_ROWS = 60
MAX_WAIT_SECONDS = 60
POLL_SECONDS = 0.5

# The capabilities that name the app of the device, in the order that they are read.
APP_CAPABILITIES = ('appium:appPackage', 'appPackage', 'appium:bundleId', 'bundleId')


class Failure(Exception):
    pass


def hub() -> str:
    return os.environ.get('HORIZON_MOBILE_HUB') or DEFAULT_HUB


def api() -> str:
    return os.environ.get('HORIZON_MOBILE_API') or DEFAULT_API


def credentials() -> tuple[str, str]:
    user = os.environ.get('BROWSERSTACK_USERNAME', '')
    key = os.environ.get('BROWSERSTACK_ACCESS_KEY', '')
    if not user or not key:
        raise Failure('BROWSERSTACK_USERNAME and BROWSERSTACK_ACCESS_KEY are not in the environment; '
                      'store both as named secrets in Settings, Secrets')
    return user, key


def scrub(text: str) -> str:
    """`text` with no access key and no user name of the session in it."""
    secrets = (os.environ.get('BROWSERSTACK_ACCESS_KEY', ''), os.environ.get('BROWSERSTACK_USERNAME', ''))
    for secret in secrets:
        if secret:
            text = text.replace(secret, '<hidden>')
    return text


def route(url: str) -> str:
    """The path of `url`, so an error names no host, no user name and no key."""
    return urllib.parse.urlsplit(url).path


def call(method: str, url: str, body=None, timeout: float = 120, raw: bytes | None = None,
         content_type: str = 'application/json'):
    user, key = credentials()
    data = raw if raw is not None else (None if body is None else json.dumps(body).encode())
    request = urllib.request.Request(url, data=data, method=method, headers={
        'Authorization': 'Basic ' + base64.b64encode(f'{user}:{key}'.encode()).decode(),
        'Content-Type': content_type})
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return json.loads(response.read() or b'{}')
    except urllib.error.HTTPError as error:
        text = scrub(error.read().decode(errors='replace'))[:500]
        raise Failure(f'{method} {route(url)} answered {error.code}: {text}') from None
    except urllib.error.URLError as error:
        raise Failure(f'{method} {route(url)} failed: {scrub(str(error.reason))}') from None
    except (TimeoutError, OSError) as error:
        raise Failure(f'{method} {route(url)} failed: {scrub(str(error))}') from None


def session() -> dict:
    try:
        with open(STATE, encoding='utf-8') as handle:
            return json.load(handle)
    except OSError:
        raise Failure('no session: run `start` first') from None
    except ValueError:
        raise Failure(f'{STATE} is not a session file: remove it and run `start`') from None


def keep(state: dict) -> None:
    with open(STATE, 'w', encoding='utf-8') as handle:
        json.dump(state, handle)
    os.chmod(STATE, 0o600)


def wd(method: str, path: str, body=None, timeout: float = 120):
    return call(method, f'{hub()}/session/{session()["id"]}{path}', body, timeout).get('value')


def find(using: str, value: str) -> str:
    found = wd('POST', '/element', {'using': using, 'value': value})
    if ELEMENT_KEY in found:
        return found[ELEMENT_KEY]
    return next(iter(found.values()))


def locate(args, platform: str, prefix: str = '') -> str | None:
    text = getattr(args, prefix + 'text', None)
    ident = getattr(args, prefix + 'id', None)
    xpath = getattr(args, prefix + 'xpath', None)
    if ident:
        return find('accessibility id', ident) if platform == 'ios' else find('id', ident)
    if xpath:
        return find('xpath', xpath)
    if text:
        quoted = json.dumps(text)
        if platform == 'ios':
            return find('-ios predicate string', f'label == {quoted} OR name == {quoted} OR value == {quoted}')
        return find('-android uiautomator', f'new UiSelector().textContains({quoted})')
    return None


def cmd_devices(args) -> dict:
    devices = call('GET', f'{api()}/devices.json')
    rows = [d for d in devices if d.get('realMobile', True)
            and (not args.platform or d.get('os') == args.platform)
            and (not args.search or args.search.lower() in d.get('device', '').lower())]
    rows.sort(key=lambda d: (d.get('os', ''), d.get('device', ''), d.get('os_version', '')))
    return {'count': len(rows), 'devices': [
        {'platform': d.get('os'), 'device': d.get('device'), 'os_version': d.get('os_version')}
        for d in rows[:MAX_ROWS]], 'more': max(0, len(rows) - MAX_ROWS)}


def cmd_upload(args) -> dict:
    boundary = uuid.uuid4().hex
    if re.match(r'^https://', args.source):
        part = (f'--{boundary}\r\nContent-Disposition: form-data; name="url"\r\n\r\n{args.source}\r\n'
                f'--{boundary}--\r\n').encode()
    else:
        with open(args.source, 'rb') as handle:
            content = handle.read()
        name = os.path.basename(args.source)
        part = (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\n'
                f'Content-Type: application/octet-stream\r\n\r\n').encode() + content + \
            f'\r\n--{boundary}--\r\n'.encode()
    answer = call('POST', f'{api()}/upload', raw=part, timeout=600,
                  content_type=f'multipart/form-data; boundary={boundary}')
    if 'app_url' not in answer:
        raise Failure(f'the upload gave no app id: {scrub(json.dumps(answer))[:300]}')
    return {'app': answer['app_url']}


def app_of(capabilities: dict) -> str | None:
    """The package of Android or the bundle id of iOS, as the session names it."""
    for name in APP_CAPABILITIES:
        value = capabilities.get(name)
        if isinstance(value, str) and value:
            return value
    return None


def cmd_start(args) -> dict:
    if os.path.exists(STATE):
        raise Failure('a session is open in this folder: run `stop` first, or run the tool in another folder')
    automation = 'XCUITest' if args.platform == 'ios' else 'UiAutomator2'
    called = {'capabilities': {'alwaysMatch': {
        'platformName': args.platform, 'appium:app': args.app, 'appium:deviceName': args.device,
        'appium:platformVersion': args.os_version, 'appium:automationName': automation,
        'bstack:options': {'projectName': 'Horizon mobile', 'buildName': time.strftime('%Y-%m-%d'),
                           'sessionName': args.name or 'agent session', 'idleTimeout': 90,
                           'interactiveDebugging': True}}}}
    started = time.time()
    created = call('POST', f'{hub()}/session', called, timeout=300)['value']
    state = {'id': created['sessionId'], 'platform': args.platform, 'device': args.device,
             'os_version': args.os_version, 'app': app_of(created.get('capabilities') or {})}
    keep(state)
    answer = {'started': True, 'start_seconds': round(time.time() - started, 1)}
    try:
        answer.update(status_of(state))
    except Failure as failure:
        # The session bills from here, so a start that cannot read its own page still
        # answers that it runs: the agent must be able to run `stop`.
        answer.update({'session': state['id'][:8], 'platform': state['platform'], 'device': state['device'],
                       'os_version': state['os_version'], 'app': state['app'], 'watch': None,
                       'note': f'the session runs; the read of its page failed: {failure}'})
    return answer


def node_row(node, platform: str) -> dict | None:
    get = node.attrib.get
    if platform == 'ios':
        label = get('label') or get('value') or get('name') or ''
        kind = node.tag.replace('XCUIElementType', '')
        tappable = kind in {'Button', 'Cell', 'Link', 'TextField', 'SecureTextField', 'Switch', 'Tab', 'SearchField'}
        ident = get('name') or ''
        shown = get('visible', 'true') == 'true'
    else:
        label = get('text') or get('content-desc') or ''
        kind = (get('class') or node.tag).rsplit('.', 1)[-1]
        tappable = get('clickable') == 'true'
        ident = get('resource-id') or ''
        shown = get('displayed', 'true') == 'true'
    if not shown or not (label.strip() or (tappable and ident)):
        return None
    row = {'kind': kind, 'text': label.strip()[:80]}
    if tappable:
        row['tap'] = True
    if ident and ident != label:
        row['id'] = ident[:80]
    return row


def rows_of(source: str, platform: str) -> list[dict]:
    rows, seen = [], set()
    for node in ElementTree.fromstring(source).iter():
        row = node_row(node, platform)
        if row and (key := (row['kind'], row['text'], row.get('id'))) not in seen:
            seen.add(key)
            rows.append(row)
    return rows


def cmd_screen(args) -> dict:
    state = session()
    rows = rows_of(wd('GET', '/source'), state['platform'])
    shown = rows if args.all else rows[:MAX_ROWS]
    return {'count': len(rows), 'rows': shown, 'more': max(0, len(rows) - len(shown))}


def pointer(x0: int, y0: int, x1: int, y1: int, millis: int) -> None:
    wd('POST', '/actions', {'actions': [{'type': 'pointer', 'id': 'finger', 'parameters': {'pointerType': 'touch'},
        'actions': [{'type': 'pointerMove', 'duration': 0, 'x': x0, 'y': y0},
                    {'type': 'pointerDown', 'button': 0}, {'type': 'pause', 'duration': 60},
                    {'type': 'pointerMove', 'duration': millis, 'x': x1, 'y': y1},
                    {'type': 'pointerUp', 'button': 0}]}]})


def cmd_tap(args) -> dict:
    state = session()
    if args.at:
        x, y = (int(part) for part in args.at.split(','))
        pointer(x, y, x, y, 80)
        return {'tapped': {'x': x, 'y': y}}
    element = locate(args, state['platform'])
    if not element:
        raise Failure('give --text, --id, --xpath or --at')
    wd('POST', f'/element/{element}/click', {})
    return {'tapped': True}


def first_field(platform: str, number: int) -> str:
    """The text field of the screen with that number, counted from 1 in reading order."""
    if platform == 'ios':
        return find('-ios class chain',
                    f'**/*[`type == "XCUIElementTypeTextField" OR type == "XCUIElementTypeSecureTextField" '
                    f'OR type == "XCUIElementTypeSearchField" OR type == "XCUIElementTypeTextView"`][{number}]')
    return find('-android uiautomator',
                f'new UiSelector().className("android.widget.EditText").instance({number - 1})')


def cmd_type(args) -> dict:
    state = session()
    element = locate(args, state['platform'], 'into_') or first_field(state['platform'], args.field)
    wd('POST', f'/element/{element}/click', {})
    text = args.text + ('\n' if args.enter else '')
    wd('POST', f'/element/{element}/value', {'text': text})
    return {'typed': len(args.text), 'enter': bool(args.enter)}


def hide_keyboard() -> bool:
    """Close the on-screen keyboard. A keyboard that is not shown is already closed."""
    try:
        wd('POST', '/appium/device/hide_keyboard', {})
    except Failure as failure:
        if ' answered 404' in str(failure) or ' answered 500' in str(failure):
            return False
        raise
    return True


def cmd_hide_keyboard(args) -> dict:
    hidden = hide_keyboard()
    return {'keyboard': 'hidden' if hidden else 'not shown'}


def cmd_swipe(args) -> dict:
    # A swipe that crosses the keyboard types letters instead of moving the screen.
    hidden = hide_keyboard()
    size = wd('GET', '/window/rect')
    width, height = size['width'], size['height']
    mid_x, mid_y = width // 2, height // 2
    far, near = 0.75, 0.25
    moves = {'up': (mid_x, int(height * far), mid_x, int(height * near)),
             'down': (mid_x, int(height * near), mid_x, int(height * far)),
             'left': (int(width * far), mid_y, int(width * near), mid_y),
             'right': (int(width * near), mid_y, int(width * far), mid_y)}
    pointer(*moves[args.direction], 350)
    return {'swiped': args.direction, 'keyboard': 'hidden' if hidden else 'not shown'}


def cmd_wait(args) -> dict:
    state = session()
    seconds = min(max(args.seconds, 1), MAX_WAIT_SECONDS)
    started = time.monotonic()
    deadline = started + seconds
    while True:
        for row in rows_of(wd('GET', '/source'), state['platform']):
            if args.text in row['text']:
                return {'found': True, 'row': row, 'seconds': round(time.monotonic() - started, 1)}
        if time.monotonic() >= deadline:
            raise Failure(f'no row holds {json.dumps(args.text)} after {seconds} seconds')
        time.sleep(min(POLL_SECONDS, max(deadline - time.monotonic(), 0.01)))


def cmd_back(args) -> dict:
    state = session()
    if state['platform'] == 'ios':
        raise Failure('the driver of iOS has no back key: tap the back control of the app, or run `open`')
    wd('POST', '/back', {})
    return {'back': True}


def cmd_open(args) -> dict:
    state = session()
    app = state.get('app')
    if not app:
        raise Failure('the session named no app of the device: run `stop`, then `start` again')
    wd('POST', '/execute/sync', {'script': 'mobile: activateApp', 'args': [{'appId': app}]})
    return {'opened': app}


def cmd_screenshot(args) -> dict:
    image = base64.b64decode(wd('GET', '/screenshot'))
    with open(args.path, 'wb') as handle:
        handle.write(image)
    return {'screenshot': os.path.abspath(args.path), 'bytes': len(image)}


def session_info(state: dict) -> dict:
    return call('GET', f'{api()}/sessions/{state["id"]}.json').get('automation_session', {})


def status_of(state: dict) -> dict:
    info = session_info(state)
    return {'session': state['id'][:8], 'platform': state['platform'], 'device': state['device'],
            'os_version': state['os_version'], 'app': state.get('app'), 'state': info.get('status'),
            'seconds': info.get('duration'), 'watch': info.get('browser_url')}


def cmd_status(args) -> dict:
    return status_of(session())


def cmd_stop(args) -> dict:
    state = session()
    verdict = 'passed' if args.passed else 'failed' if args.failed else None
    try:
        if verdict:
            script = 'browserstack_executor: ' + json.dumps({'action': 'setSessionStatus', 'arguments': {
                'status': verdict, 'reason': (args.reason or '')[:200]}})
            wd('POST', '/execute/sync', {'script': script, 'args': []})
        wd('DELETE', '')
    finally:
        os.remove(STATE)
    info = session_info(state)
    return {'stopped': True, 'state': info.get('status'), 'seconds': info.get('duration'),
            'recording': info.get('browser_url')}


def parser() -> argparse.ArgumentParser:
    main = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = main.add_subparsers(dest='command', required=True)
    devices = sub.add_parser('devices')
    devices.add_argument('--platform', choices=('android', 'ios'))
    devices.add_argument('--search')
    sub.add_parser('upload').add_argument('source')
    start = sub.add_parser('start')
    start.add_argument('--platform', choices=('android', 'ios'), required=True)
    start.add_argument('--device', required=True)
    start.add_argument('--os-version', required=True)
    start.add_argument('--app', required=True)
    start.add_argument('--name')
    screen = sub.add_parser('screen')
    screen.add_argument('--all', action='store_true', help='every row, above the cap of 60')
    tap = sub.add_parser('tap')
    for flag in ('--text', '--id', '--xpath', '--at'):
        tap.add_argument(flag)
    typed = sub.add_parser('type')
    typed.add_argument('--text', required=True)
    for flag in ('--into-text', '--into-id', '--into-xpath'):
        typed.add_argument(flag)
    typed.add_argument('--field', type=int, default=1,
                       help='with no --into-*: the text field of the screen, counted from 1')
    typed.add_argument('--enter', action='store_true')
    sub.add_parser('swipe').add_argument('direction', choices=('up', 'down', 'left', 'right'))
    sub.add_parser('hide-keyboard')
    wait = sub.add_parser('wait')
    wait.add_argument('--text', required=True)
    wait.add_argument('--seconds', type=int, default=20, help=f'at most {MAX_WAIT_SECONDS}')
    sub.add_parser('back')
    sub.add_parser('open')
    sub.add_parser('screenshot').add_argument('path')
    sub.add_parser('status')
    stop = sub.add_parser('stop')
    stop.add_argument('--passed', action='store_true')
    stop.add_argument('--failed', action='store_true')
    stop.add_argument('--reason')
    return main


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        answer = globals()['cmd_' + args.command.replace('-', '_')](args)
    except Failure as failure:
        print(json.dumps({'error': str(failure)}))
        return 1
    print(json.dumps(answer))
    return 0


if __name__ == '__main__':
    sys.exit(main())
