"""How the service's Git proxy ends a connection early: the log keeps every request that
carried a token, a client's own failure is never reported as GitHub's, and a client that does
not finish its TLS handshake loses its slot in time. Synthetic tokens only."""
import contextlib
import io
import socket
import ssl
import time
import unittest
from unittest import mock

from test_github_git_proxy import ProxyTestCase, git, gitproxy, relay, service

PUSH = (b'POST /example/project.git/git-receive-pack HTTP/1.1\r\nHost: github.com\r\n'
        b'Content-Type: application/x-git-receive-pack-request\r\nContent-Length: 4096\r\n\r\n')


class ConnectionEndTests(ProxyTestCase):
    def logged(self, request, **expected):
        """Asserts that a log record of `request` with the `expected` values is written."""
        expected['request'] = request
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if any(all(record.get(key) == value for key, value in expected.items()) for record in self.records()):
                return
            time.sleep(0.05)
        self.fail('no record %r in %r' % (expected, self.records()))

    def arrived(self):
        """Waits until GitHub got a request head."""
        deadline = time.monotonic() + 5
        while not self.github.heads and time.monotonic() < deadline:
            time.sleep(0.05)
        self.assertTrue(self.github.heads)

    def test_a_client_that_leaves_after_the_token_went_out_is_still_logged(self):
        with self.tunnel() as connection:
            connection.sendall(PUSH + b'0' * 100)
            self.arrived()
        self.logged('git-push', repository='example/project', granted=True, outcome='client-gone')

    def test_a_client_that_times_out_is_not_told_that_github_is_unreachable(self):
        with mock.patch.object(gitproxy, 'TIMEOUT', 1), self.tunnel() as connection:
            connection.sendall(PUSH + b'0' * 100)
            reply = b''
            with contextlib.suppress(ssl.SSLError, OSError):
                while chunk := connection.recv(65536):
                    reply += chunk
        self.assertNotIn(b'could not reach GitHub', reply)
        self.assertEqual(reply, b'', 'the client gets no answer of its own')
        self.logged('git-push', granted=True, outcome='client-gone')

    def test_a_reply_that_github_cuts_off_is_logged_and_never_looks_whole(self):
        env = self.routed()
        self.github.framing = 'cut'
        result, _ = self.clone('example/project', env)
        self.assertNotEqual(result.returncode, 0, 'Git must not take a cut reply for a whole one')
        self.logged('git-read', repository='example/project', granted=True, outcome='github-cut')
        # No close_notify follows a cut reply, so a TLS client sees that the tunnel broke.
        with self.tunnel(ragged=False) as connection:
            connection.sendall(b'GET /example/public.git/info/refs?service=git-upload-pack HTTP/1.0\r\n'
                               b'Host: github.com\r\n\r\n')
            with self.assertRaises(ssl.SSLError):
                while connection.recv(65536):
                    pass

    def test_a_tunnel_without_a_tls_handshake_loses_its_slot_in_time(self):
        with mock.patch.object(gitproxy, 'HEAD_SECONDS', 1):
            started = time.monotonic()
            with socket.create_connection(self.server.getsockname(), timeout=10) as connection:
                connection.sendall(b'CONNECT github.com:443 HTTP/1.1\r\nHost: github.com:443\r\n\r\n')
                head = b''
                while not head.endswith(b'\r\n\r\n'):
                    head += connection.recv(1)
                self.assertTrue(head.startswith(b'HTTP/1.1 200'), head)
                # The start of a TLS hello, and then nothing.
                connection.sendall(b'\x16\x03\x01')
                with contextlib.suppress(OSError):
                    self.assertEqual(connection.recv(1), b'')
        self.assertLess(time.monotonic() - started, 8)
        self.logged('invalid', granted=False, outcome='client-gone')

    def test_github_out_of_reach_is_logged_without_a_token(self):
        closed = socket.create_server(('127.0.0.1', 0))
        port = closed.getsockname()[1]
        closed.close()
        raw = b'GET /example/project.git/info/refs?service=git-upload-pack HTTP/1.1\r\nHost: github.com\r\n\r\n'
        exchange = gitproxy.Exchange(self.store, io.BytesIO(raw), io.BytesIO(),
                                     {service.TEST_URL: 'http://127.0.0.1:%d' % port}, lambda: None)
        with self.assertRaises(relay.Refusal) as refused:
            exchange.run()
        self.assertEqual(refused.exception.status, 502)
        self.assertEqual((exchange.record['granted'], exchange.record['outcome']), (False, 'unreachable'))

    def test_every_relayed_request_says_how_it_ended(self):
        env = self.routed()
        result, _ = self.clone('example/project', env)
        self.assertEqual(result.returncode, 0, result.stderr)
        git('ls-remote', 'https://github.com/example/secret.git', env=env, check=False)
        self.logged('git-read', repository='example/project', granted=True, outcome='relayed')
        self.logged('git-read', repository='example/secret', granted=False, outcome='refused')
        self.assertNotIn(None, [record['outcome'] for record in self.records()])


if __name__ == '__main__':
    unittest.main()
