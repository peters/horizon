#!/usr/bin/env python3
"""Tests of the mobile session tool of the horizon-mobile skill.

Every test runs the tool as a child process against a fake device cloud on a
loopback port, so no test reaches a real provider and no test needs a phone. The
fake answers the same REST and WebDriver routes as the provider, and it records
what the tool asked for.
"""
from __future__ import annotations

import base64
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import threading
import unittest
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parent.parent
TOOL = ROOT / 'assets/plugins/claude-code/skills/horizon-mobile/scripts/mobile_session.py'
ELEMENT_KEY = 'element-6066-11e4-a52e-4f735466cecf'
USER = 'mobile-test-user'
KEY = 'mobile-test-key'
SESSION = 'sess-0123456789'
WATCH = 'https://cloud.example.test/sessions/watch-1'
RECORDING = 'https://cloud.example.test/sessions/record-1'

ANDROID_SCREEN = """<?xml version="1.0" encoding="UTF-8"?>
<hierarchy rotation="0">
  <android.widget.FrameLayout class="android.widget.FrameLayout" displayed="true">
    <android.widget.TextView class="android.widget.TextView" text="Sign in" displayed="true"/>
    <android.widget.EditText class="android.widget.EditText" resource-id="com.example.app:id/email"
        text="agent@example.test" clickable="true" displayed="true"/>
    <android.widget.EditText class="android.widget.EditText" resource-id="com.example.app:id/password"
        text="secret" clickable="true" displayed="true"/>
    <android.widget.Button class="android.widget.Button" text="Continue" clickable="true" displayed="true"/>
    <android.widget.TextView class="android.widget.TextView" text="Hidden row" displayed="false"/>
  </android.widget.FrameLayout>
</hierarchy>
"""

IOS_SCREEN = """<?xml version="1.0" encoding="UTF-8"?>
<AppiumAUT>
  <XCUIElementTypeApplication type="XCUIElementTypeApplication" name="Sample" label="Sample">
    <XCUIElementTypeStaticText type="XCUIElementTypeStaticText" name="title" label="Text fields" visible="true"/>
    <XCUIElementTypeTextField type="XCUIElementTypeTextField" name="textField" value="hello" visible="true"/>
    <XCUIElementTypeButton type="XCUIElementTypeButton" name="next" label="Next" visible="true"/>
    <XCUIElementTypeButton type="XCUIElementTypeButton" name="gone" label="Gone" visible="false"/>
  </XCUIElementTypeApplication>
</AppiumAUT>
"""

MANY_ROWS = '\n'.join(
    f'<android.widget.TextView class="android.widget.TextView" text="row {index}" displayed="true"/>'
    for index in range(75))


def screen_of(rows: str) -> str:
    return f'<?xml version="1.0" encoding="UTF-8"?>\n<hierarchy>\n{rows}\n</hierarchy>\n'


class FakeCloud:
    """One fake device cloud on a loopback port, and the record of what arrived."""

    def __init__(self):
        self.requests = []
        self.source = screen_of(ANDROID_SCREEN.split('\n', 1)[1])
        self.capabilities = {'appium:appPackage': 'com.example.app'}
        self.faults = {}
        self.uploaded = None
        self.locked = threading.Lock()
        cloud = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'

            def log_message(self, *_):
                pass

            def do_GET(self):
                cloud.answer(self)

            def do_POST(self):
                cloud.answer(self)

            def do_DELETE(self):
                cloud.answer(self)

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(
            target=lambda: self.server.serve_forever(poll_interval=0.02), daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=10)

    @property
    def base(self):
        return f'http://127.0.0.1:{self.server.server_address[1]}'

    def environment(self, **extra):
        values = {
            'PATH': os.environ.get('PATH', '/usr/bin:/bin'),
            'HORIZON_MOBILE_HUB': f'{self.base}/wd/hub',
            'HORIZON_MOBILE_API': f'{self.base}/app-automate',
            'BROWSERSTACK_USERNAME': USER,
            'BROWSERSTACK_ACCESS_KEY': KEY,
        }
        values.update(extra)
        return values

    def body_of(self, handler):
        length = int(handler.headers.get('Content-Length') or 0)
        return handler.rfile.read(length) if length else b''

    def reply(self, handler, code, answer, content_type='application/json'):
        data = answer if isinstance(answer, bytes) else json.dumps(answer).encode()
        handler.send_response(code)
        handler.send_header('Content-Type', content_type)
        handler.send_header('Content-Length', str(len(data)))
        handler.end_headers()
        handler.wfile.write(data)

    def answer(self, handler):
        url = urllib.parse.urlsplit(handler.path)
        body = self.body_of(handler)
        with self.locked:
            self.requests.append({'method': handler.command, 'path': url.path,
                                  'query': urllib.parse.parse_qs(url.query),
                                  'body': body.decode(errors='replace')})
        if handler.headers.get('Authorization') != 'Basic ' + base64.b64encode(f'{USER}:{KEY}'.encode()).decode():
            self.reply(handler, 401, {'value': {'error': 'unauthorized'}})
            return
        for (method, prefix), (code, text) in self.faults.items():
            if handler.command == method and url.path.startswith(prefix):
                self.reply(handler, code, text.encode() if isinstance(text, str) else json.dumps(text).encode())
                return
        self.route(handler, url.path, body)

    def route(self, handler, path, body):
        method = handler.command
        if method == 'GET' and path == '/app-automate/devices.json':
            self.reply(handler, 200, [
                {'os': 'android', 'device': 'Google Pixel 8', 'os_version': '14.0', 'realMobile': True},
                {'os': 'android', 'device': 'Samsung Galaxy S23', 'os_version': '13.0', 'realMobile': True},
                {'os': 'ios', 'device': 'iPhone 15', 'os_version': '17', 'realMobile': True},
                {'os': 'android', 'device': 'Emulator', 'os_version': '11.0', 'realMobile': False},
            ])
            return
        if method == 'POST' and path == '/app-automate/upload':
            self.uploaded = body
            self.reply(handler, 200, {'app_url': 'bs://app-1'})
            return
        match = re.fullmatch(r'/app-automate/sessions/([^/]+)\.json', path)
        if method == 'GET' and match:
            if match.group(1) != SESSION:
                self.reply(handler, 404, {'error': 'the session does not exist'})
                return
            self.reply(handler, 200, {'automation_session': {
                'status': 'running', 'duration': 61.5, 'browser_url': WATCH}})
            return
        if method == 'POST' and path == '/wd/hub/session':
            self.reply(handler, 200, {'value': {'sessionId': SESSION, 'capabilities': self.capabilities}})
            return
        match = re.fullmatch(r'/wd/hub/session/([^/]+)(/.*)?', path)
        if not match:
            self.reply(handler, 404, {'value': {'error': 'unknown command'}})
            return
        if match.group(1) != SESSION:
            self.reply(handler, 404, {'value': {'error': 'invalid session id'}})
            return
        self.reply(handler, 200, self.session_route(method, match.group(2) or '', body))

    def session_route(self, method, rest, body):
        if rest == '' and method == 'DELETE':
            return {'value': None}
        if rest == '/source' and method == 'GET':
            return {'value': self.source}
        if rest == '/element' and method == 'POST':
            return {'value': {ELEMENT_KEY: 'el-1'}}
        if re.fullmatch(r'/element/[^/]+/click', rest) and method == 'POST':
            return {'value': None}
        if re.fullmatch(r'/element/[^/]+/value', rest) and method == 'POST':
            return {'value': None}
        if rest == '/window/rect' and method == 'GET':
            return {'value': {'width': 412, 'height': 915}}
        if rest == '/actions' and method == 'POST':
            return {'value': None}
        if rest == '/back' and method == 'POST':
            return {'value': None}
        if rest == '/screenshot' and method == 'GET':
            return {'value': base64.b64encode(b'\x89PNG\r\n\x1a\nfake').decode()}
        if rest == '/execute/sync' and method == 'POST':
            return {'value': None}
        return {'value': {'error': 'unknown command', 'message': rest}}

    def asked(self, method, prefix):
        return [request for request in self.requests
                if request['method'] == method and request['path'].startswith(prefix)]

    def lookups(self, session=SESSION):
        """The element lookups of a session, without the clicks and values of an element."""
        return [(request['method'], json.loads(request['body'])) for request in self.requests
                if request['method'] == 'POST' and request['path'] == f'/wd/hub/session/{session}/element']


class ToolTest(unittest.TestCase):
    """One fake cloud, one private folder, and the tool as a child process."""

    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.cloud = FakeCloud().__enter__()
        self.addCleanup(self.cloud.__exit__, None, None, None)

    @property
    def home(self):
        return Path(self.folder.name)

    def run_tool(self, *arguments, environment=None):
        result = subprocess.run(
            [sys.executable, '-B', str(TOOL), *arguments],
            cwd=self.folder.name,
            env=environment or self.cloud.environment(),
            capture_output=True, text=True, timeout=120, check=False)
        answer = json.loads(result.stdout) if result.stdout.strip() else None
        return result.returncode, answer, result

    def started(self, platform='android'):
        code, answer, result = self.run_tool(
            'start', '--platform', platform, '--device', 'Google Pixel 8', '--os-version', '14.0',
            '--app', 'bs://app-1')
        self.assertEqual(code, 0, result.stdout + result.stderr)
        return answer

    def state(self):
        return json.loads((self.home / '.mobile-session.json').read_text())


class DeviceTests(ToolTest):
    def test_the_device_list_holds_one_row_for_each_real_device_of_a_platform(self):
        code, answer, result = self.run_tool('devices', '--platform', 'android')

        self.assertEqual(code, 0, result.stdout + result.stderr)
        self.assertEqual(answer['count'], 2, 'an emulator is not a real phone')
        self.assertEqual([row['device'] for row in answer['devices']], ['Google Pixel 8', 'Samsung Galaxy S23'])

    def test_a_search_narrows_the_list(self):
        code, answer, _ = self.run_tool('devices', '--search', 'iphone')

        self.assertEqual(code, 0)
        self.assertEqual(answer['devices'], [{'platform': 'ios', 'device': 'iPhone 15', 'os_version': '17'}])

    def test_an_upload_sends_the_file_and_answers_its_app_id(self):
        apk = self.home / 'app-debug.apk'
        apk.write_bytes(b'PK\x03\x04 an apk of a test')

        code, answer, result = self.run_tool('upload', str(apk))

        self.assertEqual(code, 0, result.stdout + result.stderr)
        self.assertEqual(answer, {'app': 'bs://app-1'})
        self.assertIn(b'filename="app-debug.apk"', self.cloud.uploaded)
        self.assertIn(b'PK\x03\x04 an apk of a test', self.cloud.uploaded)


class SessionTests(ToolTest):
    def test_a_start_names_the_driver_the_app_and_the_idle_bound(self):
        answer = self.started()

        self.assertTrue(answer['started'])
        self.assertEqual(answer['session'], SESSION[:8])
        self.assertEqual(answer['watch'], WATCH)
        sent = json.loads(self.cloud.asked('POST', '/wd/hub/session')[0]['body'])['capabilities']['alwaysMatch']
        self.assertEqual(sent['platformName'], 'android')
        self.assertEqual(sent['appium:automationName'], 'UiAutomator2')
        self.assertEqual(sent['appium:deviceName'], 'Google Pixel 8')
        self.assertEqual(sent['bstack:options']['idleTimeout'], 90)

    def test_an_ios_start_uses_the_ios_driver(self):
        self.started('ios')

        sent = json.loads(self.cloud.asked('POST', '/wd/hub/session')[0]['body'])['capabilities']['alwaysMatch']
        self.assertEqual(sent['appium:automationName'], 'XCUITest')

    def test_the_state_file_holds_the_session_and_the_app_and_only_the_person_reads_it(self):
        self.started()

        mode = stat.S_IMODE((self.home / '.mobile-session.json').stat().st_mode)
        self.assertEqual(mode, 0o600)
        self.assertEqual(self.state()['app'], 'com.example.app')
        self.assertEqual(self.state()['id'], SESSION)

    def test_a_second_start_is_refused_while_a_session_is_open(self):
        self.started()

        code, answer, _ = self.run_tool(
            'start', '--platform', 'android', '--device', 'Google Pixel 8', '--os-version', '14.0',
            '--app', 'bs://app-1')

        self.assertEqual(code, 1)
        self.assertIn('a session is open in this folder', answer['error'])
        self.assertEqual(len(self.cloud.asked('POST', '/wd/hub/session')), 1, 'no second session was made')

    def test_a_command_without_a_session_says_so(self):
        code, answer, _ = self.run_tool('screen')

        self.assertEqual(code, 1)
        self.assertIn('no session: run `start` first', answer['error'])

    def test_a_state_file_that_is_not_json_is_one_error_and_not_a_trace(self):
        (self.home / '.mobile-session.json').write_text('not a session')

        code, answer, result = self.run_tool('screen')

        self.assertEqual(code, 1)
        self.assertIn('is not a session file', answer['error'])
        self.assertNotIn('Traceback', result.stderr)

    def test_a_stop_ends_the_session_answers_the_recording_link_and_removes_the_state(self):
        self.started()

        code, answer, result = self.run_tool('stop', '--passed', '--reason', 'the form is signed in')

        self.assertEqual(code, 0, result.stdout + result.stderr)
        self.assertTrue(answer['stopped'])
        self.assertEqual(answer['recording'], WATCH)
        self.assertFalse((self.home / '.mobile-session.json').exists(), 'no state stays after a stop')
        self.assertEqual(len(self.cloud.asked('DELETE', f'/wd/hub/session/{SESSION}')), 1)
        verdict = json.loads(self.cloud.asked('POST', f'/wd/hub/session/{SESSION}/execute/sync')[0]['body'])
        self.assertIn('setSessionStatus', verdict['script'])
        self.assertIn('passed', verdict['script'])

    def test_a_stop_without_a_verdict_sets_no_status(self):
        self.started()

        code, answer, _ = self.run_tool('stop')

        self.assertEqual(code, 0)
        self.assertTrue(answer['stopped'])
        self.assertEqual(self.cloud.asked('POST', f'/wd/hub/session/{SESSION}/execute/sync'), [])

    def test_a_start_whose_page_cannot_be_read_still_says_that_it_runs(self):
        self.cloud.faults[('GET', '/app-automate/sessions/')] = (503, 'the session page is not readable')

        code, answer, _ = self.run_tool(
            'start', '--platform', 'android', '--device', 'Google Pixel 8', '--os-version', '14.0',
            '--app', 'bs://app-1')

        self.assertEqual(code, 0, 'a session that runs is no failure of the start')
        self.assertTrue(answer['started'])
        self.assertIsNone(answer['watch'])
        self.assertIn('the session runs', answer['note'])
        self.assertIn('answered 503', answer['note'])
        self.assertTrue((self.home / '.mobile-session.json').exists(), 'the agent can still stop it')

    def test_the_credentials_come_from_the_environment(self):
        code, answer, _ = self.run_tool('devices', environment={'PATH': os.environ.get('PATH', '/bin')})

        self.assertEqual(code, 1)
        self.assertIn('BROWSERSTACK_USERNAME and BROWSERSTACK_ACCESS_KEY', answer['error'])

    def test_a_status_names_the_session_the_device_and_the_watch_link(self):
        self.started()

        code, answer, _ = self.run_tool('status')

        self.assertEqual(code, 0)
        self.assertEqual(answer['device'], 'Google Pixel 8')
        self.assertEqual(answer['state'], 'running')
        self.assertEqual(answer['watch'], WATCH)


class ScreenTests(ToolTest):
    def test_an_android_screen_is_a_row_for_each_shown_element(self):
        self.started()

        code, answer, result = self.run_tool('screen')

        self.assertEqual(code, 0, result.stdout + result.stderr)
        self.assertEqual([row['text'] for row in answer['rows']],
                         ['Sign in', 'agent@example.test', 'secret', 'Continue'])
        self.assertEqual(answer['rows'][1]['id'], 'com.example.app:id/email')
        self.assertTrue(answer['rows'][3]['tap'])
        self.assertNotIn('Hidden row', json.dumps(answer), 'a hidden element is no row')

    def test_an_ios_screen_reads_the_label_and_the_name_of_an_element(self):
        self.cloud.source = screen_of(IOS_SCREEN.split('\n', 1)[1])
        self.cloud.capabilities = {'appium:bundleId': 'net.example.sample'}
        self.started('ios')

        code, answer, _ = self.run_tool('screen')

        self.assertEqual(code, 0)
        self.assertEqual([row['text'] for row in answer['rows']], ['Sample', 'Text fields', 'hello', 'Next'])
        self.assertEqual(answer['rows'][3]['id'], 'next')
        self.assertTrue(answer['rows'][3]['tap'])
        self.assertEqual(answer['rows'][2]['id'], 'textField')

    def test_a_screen_above_the_cap_names_how_many_rows_it_leaves_out(self):
        self.cloud.source = screen_of(MANY_ROWS)
        self.started()

        _, capped, _ = self.run_tool('screen')
        _, every, _ = self.run_tool('screen', '--all')

        self.assertEqual(capped['count'], 75)
        self.assertEqual(len(capped['rows']), 60)
        self.assertEqual(capped['more'], 15)
        self.assertEqual(len(every['rows']), 75)
        self.assertEqual(every['more'], 0)


class ActionTests(ToolTest):
    def test_a_tap_by_text_looks_for_the_text_of_the_row(self):
        self.started()

        code, answer, _ = self.run_tool('tap', '--text', 'Continue')

        self.assertEqual(code, 0)
        self.assertEqual(answer, {'tapped': True})
        sent = json.loads(self.cloud.asked('POST', f'/wd/hub/session/{SESSION}/element')[0]['body'])
        self.assertEqual(sent['using'], '-android uiautomator')
        self.assertIn('textContains("Continue")', sent['value'])
        self.assertEqual(len(self.cloud.asked('POST', f'/wd/hub/session/{SESSION}/element/el-1/click')), 1)

    def test_a_tap_by_id_uses_the_selector_of_the_platform_of_the_session(self):
        self.started()
        self.run_tool('tap', '--id', 'com.example.app:id/email')
        self.run_tool('stop')
        self.cloud.capabilities = {'appium:bundleId': 'net.example.sample'}
        self.started('ios')
        self.run_tool('tap', '--id', 'next')

        android, ios = self.cloud.lookups()[0][1], self.cloud.lookups()[1][1]
        self.assertEqual(android['using'], 'id')
        self.assertEqual(android['value'], 'com.example.app:id/email')
        self.assertEqual(ios['using'], 'accessibility id')
        self.assertEqual(ios['value'], 'next')

    def test_a_tap_at_a_point_sends_one_pointer_gesture(self):
        self.started()

        code, answer, _ = self.run_tool('tap', '--at', '120,240')

        self.assertEqual(code, 0)
        self.assertEqual(answer, {'tapped': {'x': 120, 'y': 240}})
        sent = json.loads(self.cloud.asked('POST', f'/wd/hub/session/{SESSION}/actions')[0]['body'])
        moves = sent['actions'][0]['actions']
        self.assertEqual(moves[0]['x'], 120)
        self.assertEqual(moves[1]['type'], 'pointerDown')

    def test_a_type_enters_the_text_in_the_first_field_of_the_screen(self):
        self.started()

        code, answer, _ = self.run_tool('type', '--text', 'agent@example.test')

        self.assertEqual(code, 0)
        self.assertEqual(answer, {'typed': len('agent@example.test'), 'enter': False})
        lookups = [sent for _, sent in self.cloud.lookups()]
        self.assertIn('instance(0)', lookups[-1]['value'])
        sent = json.loads(self.cloud.asked('POST', f'/wd/hub/session/{SESSION}/element/el-1/value')[0]['body'])
        self.assertEqual(sent['text'], 'agent@example.test')

    def test_a_type_into_another_field_presses_the_key_and_ends_the_line(self):
        self.started()

        code, answer, _ = self.run_tool('type', '--text', 'a password', '--field', '2', '--enter')

        self.assertEqual(code, 0)
        self.assertTrue(answer['enter'])
        lookups = [sent for _, sent in self.cloud.lookups()]
        self.assertIn('instance(1)', lookups[-1]['value'], 'the second text field of the screen')
        sent = json.loads(self.cloud.asked('POST', f'/wd/hub/session/{SESSION}/element/el-1/value')[0]['body'])
        self.assertEqual(sent['text'], 'a password\n')

    def test_a_type_into_a_named_field_presses_that_element(self):
        self.started()

        code, _, _ = self.run_tool('type', '--text', 'x', '--into-text', 'Sign in')

        self.assertEqual(code, 0)
        lookups = [sent for _, sent in self.cloud.lookups()]
        self.assertIn('textContains("Sign in")', lookups[0]['value'])
        self.assertEqual(len(self.cloud.asked('POST', f'/wd/hub/session/{SESSION}/element/el-1/value')), 1)

    def test_a_type_with_no_target_on_ios_names_the_class_chain_of_a_field(self):
        self.cloud.source = screen_of(IOS_SCREEN.split('\n', 1)[1])
        self.started('ios')

        self.run_tool('type', '--text', 'hello')

        lookups = [sent for _, sent in self.cloud.lookups()]
        self.assertEqual(lookups[-1]['using'], '-ios class chain')
        self.assertIn('XCUIElementTypeTextField', lookups[-1]['value'])

    def test_a_swipe_closes_the_keyboard_first_and_moves_the_screen(self):
        self.started()

        code, answer, _ = self.run_tool('swipe', 'up')

        self.assertEqual(code, 0)
        self.assertEqual(answer['swiped'], 'up')
        self.assertEqual(len(self.cloud.asked('POST', f'/wd/hub/session/{SESSION}/appium/device/hide_keyboard')), 1,
                         'a swipe never crosses the keyboard')
        moves = json.loads(self.cloud.asked('POST', f'/wd/hub/session/{SESSION}/actions')[0]['body'])
        start, end = moves['actions'][0]['actions'][0], moves['actions'][0]['actions'][3]
        self.assertLess(end['y'], start['y'], 'an up swipe moves the finger up')
        self.assertEqual(start['x'], 412 // 2)

    def test_a_hidden_keyboard_answers_that_there_is_none_to_close(self):
        self.cloud.faults[('POST', f'/wd/hub/session/{SESSION}/appium/device/hide_keyboard')] = (
            500, {'value': {'message': 'No keyboard is shown'}})
        self.started()

        code, answer, _ = self.run_tool('hide-keyboard')

        self.assertEqual(code, 0)
        self.assertEqual(answer, {'keyboard': 'not shown'})

    def test_a_keyboard_that_closes_answers_hidden(self):
        self.started()

        code, answer, _ = self.run_tool('hide-keyboard')

        self.assertEqual(code, 0)
        self.assertEqual(answer, {'keyboard': 'hidden'})

    def test_a_wait_answers_the_row_as_soon_as_the_text_is_there(self):
        self.started()

        code, answer, result = self.run_tool('wait', '--text', 'Continue', '--seconds', '5')

        self.assertEqual(code, 0, result.stdout + result.stderr)
        self.assertTrue(answer['found'])
        self.assertEqual(answer['row']['text'], 'Continue')

    def test_a_wait_that_reaches_its_bound_is_one_error(self):
        self.started()

        code, answer, _ = self.run_tool('wait', '--text', 'Never shown', '--seconds', '1')

        self.assertEqual(code, 1)
        self.assertIn('no row holds "Never shown" after 1 seconds', answer['error'])

    def test_a_back_presses_the_back_key(self):
        self.started()

        code, answer, _ = self.run_tool('back')

        self.assertEqual(code, 0)
        self.assertEqual(answer, {'back': True})
        self.assertEqual(len(self.cloud.asked('POST', f'/wd/hub/session/{SESSION}/back')), 1)

    def test_a_back_on_ios_says_that_the_driver_has_no_back_key(self):
        self.cloud.source = screen_of(IOS_SCREEN.split('\n', 1)[1])
        self.started('ios')

        code, answer, _ = self.run_tool('back')

        self.assertEqual(code, 1)
        self.assertIn('the driver of iOS has no back key', answer['error'])
        self.assertEqual(self.cloud.asked('POST', f'/wd/hub/session/{SESSION}/back'), [])

    def test_an_open_brings_the_app_of_the_session_to_the_front(self):
        self.started()

        code, answer, _ = self.run_tool('open')

        self.assertEqual(code, 0)
        self.assertEqual(answer, {'opened': 'com.example.app'})
        sent = json.loads(self.cloud.asked('POST', f'/wd/hub/session/{SESSION}/execute/sync')[0]['body'])
        self.assertEqual(sent['script'], 'mobile: activateApp')
        self.assertEqual(sent['args'], [{'appId': 'com.example.app'}])

    def test_an_open_of_ios_names_the_bundle_id(self):
        self.cloud.capabilities = {'appium:bundleId': 'net.example.sample'}
        self.started('ios')

        code, answer, _ = self.run_tool('open')

        self.assertEqual(code, 0)
        self.assertEqual(answer, {'opened': 'net.example.sample'})

    def test_an_open_with_no_app_in_the_session_is_one_error(self):
        self.cloud.capabilities = {}
        self.started()

        code, answer, _ = self.run_tool('open')

        self.assertEqual(code, 1)
        self.assertIn('named no app of the device', answer['error'])

    def test_a_screenshot_writes_the_bytes_of_the_device(self):
        self.started()

        code, answer, result = self.run_tool('screenshot', 'shot.png')

        self.assertEqual(code, 0, result.stdout + result.stderr)
        self.assertEqual((self.home / 'shot.png').read_bytes(), b'\x89PNG\r\n\x1a\nfake')
        self.assertEqual(answer['bytes'], len(b'\x89PNG\r\n\x1a\nfake'))


class FailureTests(ToolTest):
    def test_a_missing_session_on_the_provider_is_the_code_of_the_answer_and_its_text(self):
        self.started()
        state = self.state()
        state['id'] = 'sess-gone'
        (self.home / '.mobile-session.json').write_text(json.dumps(state))

        code, answer, _ = self.run_tool('status')

        self.assertEqual(code, 1)
        self.assertIn('answered 404', answer['error'])
        self.assertIn('the session does not exist', answer['error'])

    def test_a_five_hundred_names_the_route_and_the_words_of_the_provider(self):
        self.cloud.faults[('GET', '/app-automate/devices.json')] = (503, 'the plan has no free device')

        code, answer, _ = self.run_tool('devices')

        self.assertEqual(code, 1)
        self.assertIn('/app-automate/devices.json', answer['error'])
        self.assertIn('answered 503', answer['error'])
        self.assertIn('the plan has no free device', answer['error'])

    def test_an_error_that_holds_the_credentials_prints_neither_of_them(self):
        self.cloud.faults[('GET', '/app-automate/devices.json')] = (
            500, f'Invalid credentials for {USER}: {KEY}')

        code, answer, result = self.run_tool('devices')

        self.assertEqual(code, 1)
        self.assertIn('<hidden>', answer['error'])
        self.assertNotIn(USER, result.stdout)
        self.assertNotIn(KEY, result.stdout)
        self.assertNotIn(USER, result.stderr)
        self.assertNotIn(KEY, result.stderr)

    def test_a_fault_of_the_tool_prints_one_json_object_and_no_trace(self):
        self.started()

        code, answer, result = self.run_tool('tap')

        self.assertEqual(code, 1)
        self.assertIn('give --text, --id, --xpath or --at', answer['error'])
        self.assertEqual(result.stderr.strip(), '')


class SkillTests(unittest.TestCase):
    def test_the_two_bundles_hold_the_same_tool(self):
        other = ROOT / 'assets/plugins/codex/skills/horizon-mobile/scripts/mobile_session.py'
        self.assertEqual(TOOL.read_bytes(), other.read_bytes())

    def test_the_tool_prints_no_credential_of_the_environment(self):
        text = TOOL.read_text()
        self.assertNotIn("print(user", text)
        self.assertNotIn("print(key", text)
        self.assertIn('scrub(', text)


if __name__ == '__main__':
    unittest.main()
