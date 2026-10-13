"""A cloud panel gets the session's own output, so its history scrolls in the panel (#1509)."""
import contextlib
import fcntl
import importlib.machinery
import importlib.util
import os
from pathlib import Path
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import types
import unittest

SCRIPT = Path(__file__).with_name('horizon-worker-attach')
_loader = importlib.machinery.SourceFileLoader('worker_attach', str(SCRIPT))
attach = importlib.util.module_from_spec(importlib.util.spec_from_loader(_loader.name, _loader))
_loader.exec_module(attach)

# The queries and key modes that Claude Code sends when it starts, recorded through tmux control
# mode on a worker.
CLAUDE_QUERIES = (b'\x1b[c', b'\x1b[>0q', b'\x1b[?u', b'\x1b[>5u', b'\x1b[<u', b'\x1b[>4;2m', b'\x1b[?2026$p',
                  b'\x1b]7501;?\x07')
# How the terminal of a panel answers the color query of the helper.
PANEL_COLORS = b'\x1b]10;rgb:a1a1/b2b2/c3c3\x1b\\\x1b]11;rgb:1010/2020/3030\x1b\\\x1b[?62;22c'
ANSWERED = rb'\x1b\]10;rgb:a1a1/b2b2/c3c3(\x07|\x1b\\)\x1b\]11;rgb:1010/2020/3030(\x07|\x1b\\)'


class OutputTests(unittest.TestCase):
    def test_terminal_queries_never_reach_the_terminal(self):
        # tmux answers them, so an answer from the panel would reach the session a second time.
        # tmux 3.4 answers a color query with its default colors (seen on a worker).
        queries = CLAUDE_QUERIES + (b'\x1bZ', b'\x1b[6n', b'\x1b[?6n', b'\x1b[18t', b'\x1b]11;?\x1b\\', b'\x1b]4;1;?\x07',
                                    b'\x1b]52;c;?\x07', b'\x1bP+q544e\x1b\\', b'\x1bP$qm\x1b\\')
        self.assertEqual(attach.Output().feed(b'a' + b'b'.join(queries) + b'c'), b'a' + b'b' * (len(queries) - 1) + b'c')

    def test_the_session_never_reaches_the_clipboard(self):
        self.assertEqual(attach.Output().feed(b'a\x1b]52;c;aGk=\x07b\x1b]52;c;?\x1b\\c'), b'abc')

    def test_modes_that_tmux_owns_are_taken_out_of_mode_changes(self):
        output = attach.Output().feed(b'\x1b[?1049;2004h\x1b[?1004h\x1b[?2004l\x1b[?25;2031l\x1b[?1;1004h')
        self.assertEqual(output, b'\x1b[?1049h\x1b[?25l\x1b[?1h')

    def test_display_sequences_pass_unchanged(self):
        shown = (b'\x1b[1;31mred\x1b[0m', b'\x1b[2J\x1b[3J\x1b[H', b'\x1b[5A\x1b[K', b'\x1b[u', b'\x1b[2 q',
                 b'\x1b]0;title\x07', b'\x1b]8;;https://example.com/a?b\x1b\\link\x1b]8;;\x1b\\',
                 b'\x1b]9;Build finished?\x07',
                 b'\x1bPq#0;2;0;0;0#0~~\x1b\\', b'\x1b7\x1b8\x1b=\x1b>', b'\x1b[?1049h',
                 'é ✳'.encode())
        data = b''.join(shown)
        self.assertEqual(attach.Output().feed(data), data)

    def test_sequences_split_between_reads_are_filtered_the_same(self):
        data = b'x\x1b[?2004h\x1b]52;c;?\x1b\\\x1b[1my\x1b[c\x1b]0;t\x07z' + CLAUDE_QUERIES[1]
        output = attach.Output()
        self.assertEqual(b''.join(output.feed(data[i:i + 1]) for i in range(len(data))), b'x\x1b[1my\x1b]0;t\x07z')
        self.assertEqual(output.pending, b'')

    def test_the_alternate_screen_of_the_session_is_followed(self):
        output = attach.Output()
        for data, alternate in ((b'\x1b[?25;1049h', True), (b'\x1b[?1l', True), (b'\x1b[?1049l', False),
                                (b'\x1b[?47h', True), (b'\x1bc', False)):
            output.feed(data)
            self.assertEqual(output.alternate, alternate, data)

    def test_control_bytes_inside_a_sequence_hide_nothing_from_the_filter(self):
        # The terminal (vte) executes C0 controls after ESC and inside a control sequence, ignores
        # them, DEL and 8-bit bytes in places, and ends a string at CAN or SUB.
        output = attach.Output()
        for data, shown in ((b'\x1b\x01]52;c;aGk=\x07', b'\x01'), (b'\x1b\x1b]52;c;aGk=\x07', b''),
                            (b'\x1b]5\x012;c;aGk=\x07', b''), (b'\x1b]52;c;aGk=\x18x', b'x'),
                            (b'\x1b]11;\x01?\x1b\\', b''), (b'\x1bP+\x01q544e\x1b\\', b''),
                            (b'\x1b[6\nn', b'\n'), (b'\x1b[6\xc3\xa9n', b''), (b'\x1b[?20\x0104h', b'\x01'),
                            (b'\x1b[?1049;20\x7f04h', b'\x1b[?1049h'), (b'\x1b[1\n;31m', b'\x1b[1\n;31m'),
                            (b'\x1b[6\x18n', b'\x1b[6\x18n'), (b'\x1b]0;t\x18x', b'\x1b]0;t\x18x')):
            with self.subTest(data=data):
                self.assertEqual(output.feed(data), shown)

    def test_a_reset_keeps_paste_brackets_on(self):
        self.assertEqual(attach.Output().feed(b'\x1bc\x1b[!p'), b'\x1bc\x1b[?2004h\x1b[!p\x1b[?2004h')

    def test_a_string_without_end_is_passed_on_after_the_limit(self):
        output = attach.Output()
        self.assertEqual(output.feed(b'\x1bPq#0' + b'~' * 10), b'')
        long = b'~' * attach.Output.LIMIT
        self.assertEqual(output.feed(long), b'\x1bPq#0' + b'~' * 10 + long)

    def test_a_clipboard_request_longer_than_the_limit_is_dropped_to_its_end(self):
        for end in (b'\x07', b'\x1b\\'):
            with self.subTest(end=end):
                output = attach.Output()
                shown = output.feed(b'a\x1b]52;c;' + b'A' * attach.Output.LIMIT)
                shown += output.feed(b'A' * 100 + end[:1])
                shown += output.feed(end[1:] + b'b\x1b[1mc')
                self.assertEqual(shown, b'a' + b'b\x1b[1mc')
                self.assertIsNone(output.skipping)


class InputTests(unittest.TestCase):
    def test_keys_reach_the_session_byte_for_byte(self):
        self.assertEqual(attach.Input('%3').feed('aé\x1b[A\x03'.encode()), ['send-keys -H -t %3 61 c3 a9 1b 5b 41 03'])
        commands = attach.Input('%3').feed(b'k' * (attach.KEY_CHUNK + 1))
        self.assertEqual([command.count(' 6b') for command in commands], [attach.KEY_CHUNK, 1])

    def test_a_paste_goes_through_tmux_which_brackets_it_only_when_asked(self):
        commands = attach.Input('%3').feed(b'x\x1b[200~line 1\nl"$\\#{a}\x1b[201~y')
        name = 'horizon-paste-%d-1' % os.getpid()
        self.assertEqual(commands, [
            'send-keys -H -t %3 78',
            'set-buffer -b %s -- "line 1\\012l\\042\\044\\134\\043\\173a\\175"' % name,
            'paste-buffer -p -r -d -b %s -t %%3' % name,
            'send-keys -H -t %3 79'])

    def test_a_paste_loses_only_its_nul_bytes(self):
        commands = attach.Input('%3').feed(b'\x1b[200~a\0b\x1b[201~\x1b[200~\0\x1b[201~')
        self.assertEqual([command.split(' ', 4)[-1] for command in commands[:1]], ['"ab"'])
        self.assertEqual(len(commands), 2)

    def test_a_paste_split_between_reads_is_sent_once_complete(self):
        data = b'\x1b[200~' + b'p' * (attach.PASTE_CHUNK + 1) + b'\x1b[201~'
        typed = attach.Input('%3')
        commands = [command for i in range(len(data)) for command in typed.feed(data[i:i + 1])]
        self.assertEqual([command.split(' ')[0] for command in commands], ['set-buffer', 'set-buffer', 'paste-buffer'])
        self.assertIn('-a ', commands[1])
        self.assertIsNone(typed.waiting())

    def test_a_long_paste_reaches_tmux_while_it_arrives(self):
        typed = attach.Input('%3')
        commands = typed.feed(b'\x1b[200~' + b'p' * (2 * attach.PASTE_CHUNK + 10))
        self.assertEqual([command.split(' ')[1] for command in commands], ['-b', '-a'])
        self.assertLess(len(typed.paste), attach.PASTE_CHUNK)
        self.assertEqual([command.split(' ')[0] for command in typed.feed(b'q\x1b[201~')], ['set-buffer', 'paste-buffer'])

    def test_an_escape_key_waits_only_briefly_for_a_paste_bracket(self):
        typed = attach.Input('%3')
        self.assertEqual(typed.feed(b'\x1b'), [])
        self.assertEqual(typed.waiting(), attach.MARKER_SECONDS)
        self.assertEqual(typed.flush(), ['send-keys -H -t %3 1b'])
        self.assertEqual(typed.feed(b'\x1b[2~'), ['send-keys -H -t %3 1b 5b 32 7e'])

    def test_a_paste_whose_end_never_comes_is_still_pasted(self):
        typed = attach.Input('%3')
        self.assertEqual(typed.feed(b'\x1b[200~text\x1b[20'), [])
        self.assertEqual(typed.waiting(), attach.PASTE_SECONDS)
        self.assertEqual([command.split(' ')[0] for command in typed.flush()], ['set-buffer', 'paste-buffer'])
        # Its closing bracket, when it comes after all, is no key for the session.
        self.assertEqual(typed.feed(b'more\x1b[201~x'), ['send-keys -H -t %3 6d 6f 72 65', 'send-keys -H -t %3 78'])

    def test_an_answer_that_comes_after_the_wait_is_removed_and_still_gives_the_colors(self):
        attachment = types.SimpleNamespace(outstanding=True, target='%3', colors={})
        commands, typed = attach.Attachment.answered(attachment, b'a\x1b]10;rgb:a1a1/b2b2/c3c3\x1b\\')
        self.assertEqual(typed, b'a')
        self.assertIn("set-option -wg window-style 'fg=#a1b2c3'", commands)
        self.assertTrue(attachment.outstanding)
        commands, typed = attach.Attachment.answered(attachment, b'\x1b]11;rgb:1010/2020/3030\x07\x1b[?62cb')
        self.assertEqual((commands[0], typed), ("set-option -p -t %3 window-style 'fg=#a1b2c3,bg=#102030'", b'b'))
        self.assertFalse(attachment.outstanding)
        # Once the answer is complete, input is keys again.
        self.assertEqual(attach.Attachment.answered(attachment, b'\x1b[?62c'), ([], b'\x1b[?62c'))

    def test_the_answer_to_the_color_query_is_never_typed(self):
        found, typed = attach.colors(b'a\x1b]11;rgba:1/2/3/4\x1b\\b\x1b]10;rgb:ff/0/8080\x07\x1b[?62;22cc')
        # A color in a form tmux does not take is left out, but its answer is no key either.
        self.assertEqual((found, typed), ({'fg': '#ff0080'}, b'abc'))
        self.assertEqual(attach.colors(b'\x1b]11;rgb:1010/2020/3030\x1b\\'), ({'bg': '#102030'}, b''))


class TargetTests(unittest.TestCase):
    def test_a_session_name_or_a_socket_and_pane_name_the_target(self):
        self.assertEqual(attach.targets(['attach', 'a-1']), (attach.TMUX, '=a-1', '=a-1:'))
        self.assertEqual(attach.targets(['attach', '--socket', '/proc/7/fd/3/s', '%12']),
                         (['tmux', '-u', '-N', '-S', '/proc/7/fd/3/s'], '%12', '%12'))
        for arguments in (['attach'], ['attach', 'a;b'], ['attach', '--socket', 'relative/s', '%1'],
                          ['attach', '--socket', '/s', '1'], ['attach', '--socket', '/s', '%1', 'extra'],
                          ['attach', '--socket', '/s']):
            with self.subTest(arguments=arguments):
                self.assertIsNone(attach.targets(arguments))


class ControlTests(unittest.TestCase):
    def test_output_that_comes_with_the_last_reply_is_kept(self):
        # The reply to attach-session itself (flags 0) is no reply to a command of the helper.
        read, written = os.pipe()
        os.write(written, b'%begin 1 6 0\n%end 1 6 0\n%output %1 before\n%begin 1 7 1\nscreen row\n%end 1 7 1\n'
                          b'%output %2 other\n%output %1 after\\012\\134\n')
        os.close(written)
        control = attach.Control.__new__(attach.Control)
        control.process = types.SimpleNamespace(stdout=os.fdopen(read, 'rb'))
        self.addCleanup(control.process.stdout.close)
        control.partial, control.block, control.replies, control.ended = b'', None, [], False
        attachment = attach.Attachment.__new__(attach.Attachment)
        attachment.control = control
        self.assertEqual(attachment.wait(1), ([[b'screen row']], [('%2', b'other'), ('%1', b'after\n\\')]))

    def test_input_after_tmux_ended_ends_the_attach_without_an_error(self):
        read, written = os.pipe()
        os.close(read)
        control = attach.Control.__new__(attach.Control)
        control.process = types.SimpleNamespace(stdin=os.fdopen(written, 'wb'))
        control.ended = False
        control.send(['send-keys -H -t %1 61'])
        self.assertTrue(control.ended)
        control.send(['send-keys -H -t %1 62'])


class RenderTests(unittest.TestCase):
    STATE = dict(zip(attach.STATE, ('%1', '20', '3', '2', '2', '2', '1', '0', '4294967295', '4294967295', '0', '0', '0',
                                    '0', '1', '0', '2', '0', '0', '0', '0', '0', 'worker.example', 'worker',
                                    'worker.example')))

    def test_the_terminal_is_cleared_with_its_scrollback_and_shows_the_pane(self):
        shown = attach.render(self.STATE, [[b'old 1', b'old 2', b'a', b'b', b'$ ']], [])
        self.assertTrue(shown.startswith(b'\x1b[?1049l'))
        self.assertIn(b'\x1b[H\x1b[2J\x1b[3Jold 1\r\nold 2\r\na\r\nb\r\n$ \x1b[3;3H', shown)
        # tmux names a pane after the host until the session sets a title, so no title is set.
        self.assertTrue(shown.endswith(b'\x1b]2;\x07\x1b[?2004h'))
        titled = attach.render(dict(self.STATE, pane_title='✳ Build and test'), [[b'$ ']], [])
        self.assertTrue(titled.endswith('\x1b]2;✳ Build and test\x07\x1b[?2004h'.encode()))

    def test_a_full_row_whose_next_character_wraps_is_written_again(self):
        state = dict(self.STATE, pane_width='3', cursor_x='3', cursor_y='1')
        shown = attach.render(state, [[b'abc', b'\x1b[31mxyz', b'']], [], [b'\x1b[31mxyz'])
        self.assertIn(b'\x1b[2;1H\x1b[31mxyz\x1b[0m\x1b]2;', shown)
        self.assertNotIn(b'\x1b[2;4H', shown)

    def test_attributes_continue_on_the_next_row_without_filling_it(self):
        text, carried = attach.lines([b'\x1b[41mred', b'still red', b'\x1b[0mplain'], [])
        self.assertEqual(text, b'\x1b[41mred\x1b[0m\r\n\x1b[41mstill red\x1b[0m\r\n\x1b[41m\x1b[0mplain\x1b[0m')
        self.assertEqual(carried, [])

    def test_an_alternate_screen_is_restored_above_the_normal_history(self):
        state = dict(self.STATE, alternate_on='1', alternate_saved_x='1', alternate_saved_y='2', cursor_flag='0',
                     keypad_cursor_flag='1', mouse_button_flag='1', mouse_sgr_flag='1', scroll_region_lower='1')
        shown = attach.render(state, [[b'\x1b[32mhistory'], [b'$ less']], [[b'page 1', b'page 2', b':']])
        # Each capture starts with default attributes: the green history does not color the screen.
        self.assertIn(b'\x1b[32mhistory\x1b[0m\r\n$ less\x1b[3;2H\x1b[?1049h\x1b[Hpage 1\r\npage 2\r\n:\x1b[1;2r',
                      shown)
        for mode in (b'\x1b[?1h', b'\x1b[?1002h', b'\x1b[?1006h', b'\x1b[?25l'):
            self.assertIn(mode, shown)


def plain(data):
    """The text rows of terminal output, without escape sequences."""
    text = re.sub(rb'\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[()][0-9A-Za-z]|\x1b[=>78c]', b'',
                  data).decode('utf-8', 'replace')
    return [row.strip('\r') for row in re.split(r'\r?\n', text)]


def numbers(data):
    return [int(row) for row in plain(data) if row.isdigit()]


@unittest.skipUnless(shutil.which('tmux'), 'the attach tests need tmux')
class TmuxTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        self.env = {'PATH': os.environ['PATH'], 'HOME': str(root), 'TMUX_TMPDIR': str(root), 'TERM': 'xterm-256color',
                    'PS1': '$ ', 'LANG': 'C.UTF-8'}
        self.root = root
        self.tmux('-f', '/dev/null', 'set-option', '-g', 'history-limit', '10000', ';',
                  'new-session', '-d', '-s', 'a', '-x', '80', '-y', '24', 'sh')
        self.addCleanup(self.tmux, 'kill-server')
        self.wait_for(lambda: self.tmux('capture-pane', '-p', '-t', '=a:').strip() == '$')

    def tmux(self, *args):
        result = subprocess.run(['tmux', '-L', 'horizon-cloud', *args], env=self.env, capture_output=True, text=True,
                                timeout=10)
        return result.stdout if result.returncode == 0 else None

    @staticmethod
    def wait_for(condition, seconds=10):
        deadline = time.monotonic() + seconds
        while not condition():
            if time.monotonic() > deadline:
                raise AssertionError('condition not met in time')
            time.sleep(0.05)

    def attach(self, session='a', cols=80, rows=24, arguments=None, answer=PANEL_COLORS):
        terminal = self.start(arguments or [session], cols, rows)
        terminal.read_until(lambda data: attach.COLOR_QUERY in data)
        terminal.write(answer)
        terminal.read_until(lambda data: data.endswith(b'\x1b[?2004h'))
        return terminal

    def start(self, arguments, cols=80, rows=24):
        pid, fd = pty.fork()
        if pid == 0:
            fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, 0, 0))
            os.execve(sys.executable, [sys.executable, str(SCRIPT), *arguments], self.env)
        terminal = Terminal(pid, fd)
        self.addCleanup(terminal.close)
        return terminal

    def private(self, socket, *args):
        result = subprocess.run(['tmux', '-S', socket, *args], env=self.env, capture_output=True, text=True, timeout=10)
        return result.stdout if result.returncode == 0 else None

    def test_history_and_later_output_reach_the_terminal_in_full(self):
        self.tmux('send-keys', '-t', '=a:', 'seq 1 3000', 'Enter')
        self.wait_for(lambda: '3000' in self.tmux('capture-pane', '-p', '-t', '=a:'))
        terminal = self.attach()
        self.assertEqual(numbers(terminal.data), list(range(1, 3001)))
        # A plain tmux client redraws a full screen instead and leaves gaps.
        terminal.write(b'seq 3001 30000\r')
        terminal.read_until(lambda data: b'\r\n30000\r\n' in data)
        self.assertEqual(numbers(terminal.data), list(range(1, 30001)))

    def test_output_written_while_detached_is_shown_once_at_the_next_attach(self):
        first = self.attach()
        first.write(b'echo before\r')
        first.read_until(lambda data: b'\r\nbefore\r\n' in data)
        first.close()
        self.assertEqual(first.status, 0)
        self.assertEqual(self.tmux('list-clients', '-t', '=a'), '')
        self.tmux('send-keys', '-t', '=a:', 'seq 1 100', 'Enter')
        self.wait_for(lambda: '100' in self.tmux('capture-pane', '-p', '-t', '=a:'))
        second = self.attach()
        self.assertIn(b'\x1b[3J', second.data)
        rows = plain(second.data.split(b'\x1b[3J', 1)[1])
        self.assertEqual(rows.count('before'), 1)
        self.assertEqual(numbers(second.data), list(range(1, 101)))

    def test_queries_reach_only_tmux_and_paste_brackets_follow_the_session(self):
        received = self.root / 'received'
        # Asks for the terminal's attributes once attached, then records all input up to "end".
        reader = ("import os, sys, tty\ntty.setraw(0); os.write(1, b'waiting\\r\\n'); os.read(0, 1)\n"
                  "os.write(1, (b'\\x1b[c\\x1b]11;?\\x07\\x1b[?2004h' if sys.argv[1] == 'yes' else b'') + b'ready\\r\\n'); data = b''\n"
                  "while not data.endswith(b'end'): data += os.read(0, 100)\n"
                  "open(sys.argv[2], 'wb').write(data)")
        for asked in ('yes', 'no'):
            with self.subTest(asked=asked):
                session = 'reader-' + asked
                self.tmux('new-session', '-d', '-s', session, '-x', '80', '-y', '24',
                          sys.executable, '-c', reader, asked, str(received))
                self.wait_for(lambda: 'waiting' in (self.tmux('capture-pane', '-p', '-t', '=%s:' % session) or ''))
                terminal = self.attach(session)
                terminal.write(b'q')
                terminal.read_until(lambda data: b'ready\r\n' in data)
                # A paste that starts with "-" is text, not tmux options.
                terminal.write('\x1b[200~-rf één\n日本\x1b[201~øend'.encode())
                self.wait_for(received.exists)
                pasted = received.read_bytes()
                received.unlink()
                terminal.close()
                # The color query of the helper itself comes first.
                shown = terminal.data.split(attach.COLOR_QUERY, 1)[1]
                self.assertNotIn(b'\x1b[c', shown)
                self.assertNotIn(b'\x1b]11;?', shown)
                if asked == 'yes':
                    # tmux's own answers, once each, then the paste in the brackets the session asked for.
                    self.assertRegex(pasted, rb'\A\x1b\[\?[0-9;]*c\x1b\]11;rgb:[0-9a-f/]+(\x07|\x1b\\)'
                                             + re.escape('\x1b[200~-rf één\n日本\x1b[201~øend'.encode()) + rb'\Z')
                else:
                    self.assertEqual(pasted, '-rf één\n日本øend'.encode())

    def test_color_queries_get_the_colors_of_the_panel(self):
        received = self.root / 'colors'
        # Asks for the colors once attached, as an agent does when it starts or redraws.
        reader = ("import os, sys, tty\ntty.setraw(0); os.write(1, b'waiting\\r\\n'); os.read(0, 1)\n"
                  "os.write(1, b'\\x1b]10;?\\x07\\x1b]11;?\\x07\\x1b[c'); data = b''\n"
                  "while not data.endswith(b'c'): data += os.read(0, 100)\n"
                  "open(sys.argv[1], 'wb').write(data)")
        self.tmux('new-session', '-d', '-s', 'colors', '-x', '80', '-y', '24', sys.executable, '-c', reader, str(received))
        self.wait_for(lambda: 'waiting' in (self.tmux('capture-pane', '-p', '-t', '=colors:') or ''))
        terminal = self.attach('colors')
        terminal.write(b'q')
        self.wait_for(received.exists)
        self.assertRegex(received.read_bytes(), rb'\A' + ANSWERED)
        self.assertNotIn(b'\x1b]11;?', terminal.data.split(attach.COLOR_QUERY, 1)[1])
        # A session that starts later answers with them before its first attach.
        received.unlink()
        self.tmux('new-session', '-d', '-s', 'later', '-x', '80', '-y', '24', sys.executable, '-c',
                  reader.replace("os.read(0, 1)", "None"), str(received))
        self.wait_for(received.exists)
        self.assertRegex(received.read_bytes(), rb'\A' + ANSWERED)

    def test_keys_typed_before_a_slow_answer_reach_the_session_after_the_colors(self):
        terminal = self.start(['a'])
        terminal.read_until(lambda data: attach.COLOR_QUERY in data)
        terminal.write(b'echo $((6*7))\r')
        time.sleep(3)
        # The session is not shown yet, and the keys did not reach it.
        self.assertFalse(terminal.data.endswith(b'\x1b[?2004h'))
        self.assertNotIn('42', self.tmux('capture-pane', '-p', '-t', '=a:'))
        terminal.write(PANEL_COLORS)
        terminal.read_until(lambda data: b'\r\n42\r\n' in data)
        self.assertIn('fg=#a1b2c3,bg=#102030', self.tmux('show-options', '-wg', 'window-style'))

    def test_a_panel_that_reports_no_colors_sets_no_style_and_keeps_typed_keys(self):
        terminal = self.attach(answer=b'echo early\r\x1b[?1;2c')
        terminal.read_until(lambda data: b'\r\nearly\r\n' in data)
        self.assertEqual(self.tmux('show-options', '-p', '-t', '=a:', 'window-style'), '')
        self.assertEqual(self.tmux('show-options', '-wg', 'window-style').strip(), 'window-style default')

    def test_an_alternate_screen_comes_back_above_the_normal_history_and_screen(self):
        script = ("seq -f 'normal %g' 1 40; printf '\\033[?1049h\\033[Hfull screen'; read x; printf '\\033[?1049l'; "
                  "echo back; exec sh")
        self.tmux('new-session', '-d', '-s', 'full', '-x', '80', '-y', '24', 'sh', '-c', script)
        self.wait_for(lambda: 'full screen' in (self.tmux('capture-pane', '-p', '-t', '=full:') or ''))
        terminal = self.attach('full')
        normal, alternate = bytes(terminal.data).split(b'\x1b[?1049h', 1)
        self.assertEqual([row for row in plain(normal) if row.startswith('normal')], ['normal %d' % n for n in range(1, 41)])
        self.assertEqual([row for row in plain(alternate) if row], ['full screen'])
        terminal.write(b'\r')
        terminal.read_until(lambda data: b'back' in data)
        self.assertIn(b'\x1b[?1049l', terminal.data[len(normal):])

    def test_a_session_that_ends_on_its_alternate_screen_leaves_the_panel_on_its_history(self):
        self.tmux('new-session', '-d', '-s', 'ends', '-x', '80', '-y', '24', 'sh', '-c',
                  "seq 1 30; printf '\\033[?1049hfull screen'; read x")
        self.wait_for(lambda: 'full screen' in (self.tmux('capture-pane', '-p', '-t', '=ends:') or ''))
        terminal = self.attach('ends')
        terminal.write(b'\r')
        terminal.wait()
        self.assertEqual(terminal.status, 0)
        self.assertTrue(bytes(terminal.data).endswith(b'\x1b[?1049l' + attach.RESET_MODES))

    def test_a_cursor_after_a_full_row_keeps_its_pending_wrap(self):
        self.tmux('new-session', '-d', '-s', 'full', '-x', '80', '-y', '24', 'sh', '-c', 'printf %080d 0; exec sleep 30')
        self.wait_for(lambda: '0' * 80 in (self.tmux('capture-pane', '-p', '-t', '=full:') or ''))
        terminal = self.attach('full')
        self.assertIn(b'\x1b[1;1H' + b'0' * 80 + b'\x1b]2;', terminal.data)

    def test_a_title_set_while_detached_comes_back_at_the_next_attach(self):
        self.tmux('new-session', '-d', '-s', 'titled', '-x', '80', '-y', '24', 'sh', '-c',
                  "printf '\\033]2;Detached title with spaces\\007'; echo ready; exec sh")
        self.wait_for(lambda: 'ready' in (self.tmux('capture-pane', '-p', '-t', '=titled:') or ''))
        terminal = self.attach('titled')
        self.assertTrue(bytes(terminal.data).endswith(b'\x1b]2;Detached title with spaces\x07\x1b[?2004h'))

    def test_a_paste_cut_off_by_a_disconnect_leaves_no_tmux_buffer(self):
        terminal = self.attach()
        terminal.write(b'\x1b[200~' + b'p' * (2 * attach.PASTE_CHUNK))
        self.wait_for(lambda: 'horizon-paste-' in (self.tmux('list-buffers', '-F', '#{buffer_name}') or ''))
        terminal.close()
        self.assertEqual(self.tmux('list-buffers', '-F', '#{buffer_name}'), '')

    def test_a_pane_on_a_private_socket_attaches_with_its_history(self):
        # How a signed project session attaches: its own server, reached by socket path and pane.
        socket = str(self.root / 'private')
        self.private(socket, '-f', '/dev/null', 'set-option', '-g', 'history-limit', '10000', ';',
                     'new-session', '-d', '-s', 'p', '-x', '80', '-y', '24', 'sh')
        self.addCleanup(self.private, socket, 'kill-server')
        pane = self.private(socket, 'display-message', '-p', '-t', '=p:', '#{pane_id}').strip()
        self.wait_for(lambda: self.private(socket, 'capture-pane', '-p', '-t', pane).strip() == '$')
        self.private(socket, 'send-keys', '-t', pane, 'seq 1 3000', 'Enter')
        self.wait_for(lambda: '3000' in self.private(socket, 'capture-pane', '-p', '-t', pane))
        terminal = self.attach(arguments=['--socket', socket, pane])
        self.assertEqual(numbers(terminal.data), list(range(1, 3001)))
        terminal.write(b'echo private-socket\r')
        terminal.read_until(lambda data: b'\r\nprivate-socket\r\n' in data)
        # A paste cut off by a disconnect is deleted through the attach itself.
        terminal.write(b'\x1b[200~' + b'p' * (2 * attach.PASTE_CHUNK))
        self.wait_for(lambda: 'horizon-paste-' in (self.private(socket, 'list-buffers', '-F', '#{buffer_name}') or ''))
        terminal.close()
        self.assertEqual(terminal.status, 0)
        self.assertEqual(self.private(socket, 'list-buffers', '-F', '#{buffer_name}'), '')
        self.assertEqual(self.private(socket, 'list-clients'), '')

    def test_a_private_socket_without_a_server_gets_no_server(self):
        socket = self.root / 'absent'
        terminal = self.start(['--socket', str(socket), '%0'])
        terminal.wait()
        self.assertNotEqual(terminal.status, 0)
        self.assertFalse(socket.exists())

    def test_the_session_follows_the_terminal_size(self):
        terminal = self.attach(cols=100, rows=30)
        self.assertEqual(self.tmux('display-message', '-p', '-t', '=a:', '#{pane_width}x#{pane_height}').strip(), '100x30')
        fcntl.ioctl(terminal.fd, termios.TIOCSWINSZ, struct.pack('HHHH', 20, 70, 0, 0))
        os.kill(terminal.pid, signal.SIGWINCH)
        self.wait_for(lambda: self.tmux('display-message', '-p', '-t', '=a:', '#{pane_width}x#{pane_height}').strip()
                      == '70x20')

    def test_only_this_session_reaches_the_terminal_and_its_end_ends_the_attach(self):
        self.tmux('new-session', '-d', '-s', 'b', '-x', '80', '-y', '24', 'sh')
        terminal = self.attach()
        self.tmux('send-keys', '-t', '=b:', 'echo other-session', 'Enter')
        terminal.write(b'echo this-session\r')
        terminal.read_until(lambda data: b'\r\nthis-session\r\n' in data)
        self.wait_for(lambda: 'other-session\n' in self.tmux('capture-pane', '-p', '-t', '=b:'))
        terminal.write(b'exit\r')
        terminal.wait()
        self.assertEqual(terminal.status, 0)
        self.assertNotIn(b'other-session', terminal.data)
        self.assertIsNotNone(self.tmux('has-session', '-t', '=b'))


class Terminal:
    """The terminal side of one attach: a pseudo-terminal that records what the helper shows."""

    def __init__(self, pid, fd):
        self.pid, self.fd = pid, fd
        self.data = bytearray()
        self.status = None

    def read_until(self, condition, seconds=20):
        deadline = time.monotonic() + seconds
        while not condition(bytes(self.data)):
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise AssertionError('terminal output did not arrive: %r' % bytes(self.data[-200:]))
            if select.select([self.fd], [], [], remaining)[0]:
                try:
                    chunk = os.read(self.fd, 65536)
                except OSError:
                    chunk = b''
                if not chunk:
                    raise AssertionError('the attach ended: %r' % bytes(self.data[-200:]))
                self.data += chunk

    def write(self, data):
        os.write(self.fd, data)

    def wait(self, seconds=10):
        deadline = time.monotonic() + seconds
        while self.status is None:
            with contextlib.suppress(OSError):
                if select.select([self.fd], [], [], 0.05)[0]:
                    self.data += os.read(self.fd, 65536)
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.status = os.waitstatus_to_exitcode(status)
            elif time.monotonic() > deadline:
                raise AssertionError('the attach did not end')

    def close(self):
        if self.status is None:
            with contextlib.suppress(OSError):
                os.close(self.fd)
            try:
                self.wait()
            except AssertionError:
                os.kill(self.pid, signal.SIGKILL)
                os.waitpid(self.pid, 0)
                raise


if __name__ == '__main__':
    unittest.main()
