"""Deterministic coverage of the mutation phases and the observer channel: which az calls
happen, in which order, on which resource, and what the restricted reader accepts. No
Azure, no network."""
from __future__ import annotations

import base64
import datetime as dt
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time
import unittest
import unittest.mock as mock

from harness_fixtures import A_INSTANCE, B_INSTANCE, IMAGE, RUN_ID, client_off, manifest, samples

class WorkerDescriptorTests(unittest.TestCase):
    def descriptor(self, **overrides):
        base = {"vm_name": "worker", "port": 2222, "host_key": "ssh-ed25519 " + "A" * 68, "observer_key_path": __file__,
                "progress_path": "/workspace/live/progress", "group_id": "/g/b", "vm_id": "/g/b/vm", "instance_id": B_INSTANCE,
                "host": "52.174.10.5"}
        base.update(overrides)
        return base

    def test_descriptor_gates_are_named_before_anything_is_stopped(self):
        self.assertEqual(client_off.validate_worker(self.descriptor()), [])
        self.assertEqual(client_off.validate_worker(self.descriptor(checkpoint_path="/workspace/live/checkpoint")), [])
        self.assertTrue(client_off.validate_worker(self.descriptor(checkpoint_path="/workspace/live/progress")),
                        "the counter file can never stand in for a checkpoint")
        for alias in ("/workspace/live/./progress", "/workspace/./live/progress", "/workspace/live/../live/progress"):
            with self.subTest(alias=alias):
                self.assertTrue(client_off.validate_worker(self.descriptor(checkpoint_path=alias)),
                                "no spelling of the counter path may pass as the checkpoint")
                self.assertTrue(client_off.validate_worker(self.descriptor(progress_path=alias)))
        self.assertEqual(client_off.validate_worker(self.descriptor(progress_path="/workspace/.hidden/a..b")), [])
        for field, value in (("vm_name", "a b"), ("vm_name", "other"), ("vm_name", "Worker"), ("port", "2222"), ("port", True), ("host_key", "ssh-rsa AAAA"),
                             ("host_key", "ssh-ed25519 " + "A" * 68 + "\n[evil]:22 ssh-ed25519 " + "B" * 68),
                             ("host_key", "ssh-ed25519 " + "A" * 68 + " comment"),
                             ("host_key", "ssh-ed25519 " + "A" * 68 + "\n"), ("instance_id", "not-a-vmid"),
                             ("observer_key_path", "/nonexistent/key"), ("progress_path", "/etc/passwd"),
                             ("checkpoint_path", "/dev/zero"), ("group_id", 5), ("host", "")):
            with self.subTest(field=field, value=value):
                self.assertTrue(client_off.validate_worker(self.descriptor(**{field: value})))
        self.assertEqual(client_off.validate_worker("worker"), ["worker descriptor is not an object"])
        self.assertTrue(all(problem.startswith("missing ") for problem in client_off.validate_worker({})))

    def test_endpoints_must_be_public_addresses(self):
        for host in ("10.0.0.5", "127.0.0.1", "169.254.169.254", "not-an-ip", "224.0.0.1", "::1"):
            with self.subTest(host=host):
                self.assertTrue(client_off.validate_worker(self.descriptor(host=host)))
                self.assertFalse(client_off.evaluate_samples(samples(49, host=host), 12, 600)["passed"])
        self.assertTrue(client_off.routable("52.174.10.7"))
        self.assertFalse(client_off.routable("192.168.1.1"))


def fake_ssh(payload: bytes, returncode: int = 0, diagnostics: bytes = b""):
    """A Popen stand-in that streams `payload` on stdout, writes `diagnostics` to the
    stderr file it is handed and exits with `returncode`."""
    class FakeProcess:
        def __init__(self, *args, **kwargs):
            r, w = os.pipe()
            os.write(w, payload)
            os.close(w)
            self.stdout = os.fdopen(r, "rb")
            r, w = os.pipe()
            os.write(w, diagnostics)
            os.close(w)
            self.stderr = os.fdopen(r, "rb")
            self.returncode = returncode

        def wait(self, timeout=None):
            return returncode

        def kill(self):
            pass

    return FakeProcess


class ObserverChannelTests(unittest.TestCase):
    """Observer C reads through a key sshd restricts to one forced reader."""

    HOST_KEY = "ssh-ed25519 " + "A" * 68

    PTY_DENIED = (b"Warning: remote port forwarding failed for listen port 47331\r\n"
                  b"PTY allocation request failed on channel 0\r\n")

    def observe(self, payload, returncode=0, diagnostics=PTY_DENIED):
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(client_off.subprocess, "Popen", fake_ssh(payload, returncode, diagnostics)):
            return client_off.read_observations("52.174.10.5", 2222, self.HOST_KEY, "/dev/null", directory)

    def test_only_the_forced_readers_json_answer_counts(self):
        nothing = {"progress": None, "checkpoint": None, "channel": "unavailable", "paths": None}
        paths = b'"paths": {"progress": "/workspace/live/progress", "checkpoint": null}'
        echoed = {"progress": "/workspace/live/progress", "checkpoint": None}
        self.assertEqual(self.observe(b'{"progress": "42\\n", "checkpoint": " 7 ", ' + paths + b'}'),
                         {"progress": 42, "checkpoint": 7, "channel": "answered", "paths": echoed})
        self.assertEqual(self.observe(b'{"progress": "42", "checkpoint": null, ' + paths + b'}'),
                         {"progress": 42, "checkpoint": None, "channel": "answered", "paths": echoed})
        self.assertEqual(self.observe(b'{"progress": null, "checkpoint": null, ' + paths + b'}'),
                         {"progress": None, "checkpoint": None, "channel": "answered", "paths": echoed},
                         "an authorized key whose files are missing is still an authorized key")
        self.assertEqual(self.observe(b'{"progress": "42", "checkpoint": null}')["channel"], "unavailable",
                         "a reader that does not echo its paths is not the forced reader")
        # The forced reader answered but the pty was granted: a `command=` line without
        # `restrict`, which is not the read-only channel the acceptance needs.
        self.assertEqual(self.observe(b'{"progress": "42", "checkpoint": null, ' + paths + b'}', diagnostics=b""),
                         {"progress": 42, "checkpoint": None, "channel": "unrestricted", "paths": echoed})
        self.assertEqual(self.observe(b'{"progress": "42",\r\n "checkpoint": null, ' + paths + b'}\r\n', diagnostics=b""),
                         {"progress": 42, "checkpoint": None, "channel": "unrestricted", "paths": echoed},
                         "a pty's CRLF is not a parse failure")
        for partial in (b"PTY allocation request failed on channel 0\r\n",
                        b"Warning: remote port forwarding failed for listen port 47331\r\n"):
            with self.subTest(partial=partial):
                self.assertEqual(self.observe(b'{"progress": "42", "checkpoint": null, ' + paths + b'}', diagnostics=partial)["channel"],
                                 "unrestricted", "one denied capability is not `restrict`: both must be denied")
        refused = b"root@52.174.10.5: Permission denied (publickey).\n"
        self.assertEqual(self.observe(b"", 255, refused), dict(nothing, channel="refused"))
        for returncode, diagnostics in ((255, b"ssh: connect to host 52.174.10.5 port 2222: Connection refused\n"),
                                        (255, b"Host key verification failed.\n" + refused), (255, b""), (1, refused),
                                        (0, refused)):
            with self.subTest(returncode=returncode, diagnostics=diagnostics[:30]):
                self.assertEqual(self.observe(b"", returncode, diagnostics), nothing,
                                 "only an explicit publickey refusal under the pin is a refusal")
        # The forced reader answered, but the counter is not exactly one integer: the
        # key is authorized and the sample is unreadable.
        for payload in (b'{"progress": "1\\n2\\n", "checkpoint": null, ', b'{"progress": "-3", "checkpoint": null, ',
                        b'{"progress": "4x", "checkpoint": null, ', b'{"progress": "' + b"1" * 19 + b'", "checkpoint": null, '):
            with self.subTest(payload=payload[:40]):
                self.assertEqual(self.observe(payload + paths + b'}'), dict(nothing, channel="answered", paths=echoed))
        # Not the forced reader's answer at all: nothing is known.
        for payload in (b"uid=0(root) gid=0(root)\n",  # the sent command ran: the key is not restricted
                        b'{"progress": "42"}', b'{"progress": "42", "checkpoint": null, "shell": "sh"}', b"", b"\xff",
                        b'{"progress": "' + b"9" * 2000 + b'", "checkpoint": null}'):
            with self.subTest(payload=payload[:40]):
                self.assertEqual(self.observe(payload), nothing)
        self.assertEqual(client_off.read_observations("52.174.10.5", 2222, "ssh-ed25519 AAAA\nx", "/dev/null", "/tmp"),
                         nothing, "a pin with a newline is never written")
        self.assertEqual(client_off.read_observations("203.0.113.5", 2222, self.HOST_KEY, "/dev/null", "/tmp"), nothing,
                         "an unroutable endpoint is never dialled")

    def test_forced_reader_reads_regular_files_below_the_root_only(self):
        with tempfile.TemporaryDirectory() as root:
            os.makedirs(f"{root}/task")
            pathlib.Path(f"{root}/task/progress").write_text("42\n", encoding="utf-8")
            pathlib.Path(f"{root}/secret").write_text("no\n", encoding="utf-8")
            os.symlink(f"{root}/secret", f"{root}/task/link")
            os.symlink(f"{root}", f"{root}/task/dirlink")
            pathlib.Path(f"{root}/task/padded").write_text("42" + " " * client_off.PROGRESS_READ_LIMIT + "x\n",
                                                            encoding="utf-8")
            pathlib.Path(f"{root}/task/full").write_text("4" * client_off.PROGRESS_READ_LIMIT, encoding="utf-8")
            os.link(f"{root}/task/progress", f"{root}/task/hardlink")
            command = client_off.reader_command(f"{root}/task/progress", f"{root}/task/hardlink", root=root)
            completed = subprocess.run(["sh", "-c", command], capture_output=True, text=True, check=False)
            self.assertEqual(json.loads(completed.stdout)["checkpoint"], None,
                             "a checkpoint that is the counter file under another name is never checkpoint proof")
            cases = {f"{root}/task/progress": "42\n", f"{root}/task/link": None, f"{root}/task": None,
                     f"{root}/task/dirlink/secret": None, f"{root}/task/missing": None,
                     # Over the cap is never "exactly one integer", whatever the prefix looks like.
                     f"{root}/task/padded": None, f"{root}/task/full": "4" * client_off.PROGRESS_READ_LIMIT}
            for path, expected in cases.items():
                with self.subTest(path=path):
                    command = client_off.reader_command(path, None, root=root)
                    self.assertNotIn("'", command, "the forced command must survive every quoting layer")
                    completed = subprocess.run(["sh", "-c", command], capture_output=True, text=True, check=False)
                    self.assertEqual(completed.returncode, 0, completed.stderr)
                    answer = json.loads(completed.stdout)
                    self.assertEqual((answer["progress"], answer["checkpoint"]), (expected, None))
                    self.assertEqual(answer["paths"], {"progress": path, "checkpoint": None}, "the reader echoes its paths")
            with self.assertRaises(ValueError):
                client_off.reader_command(f"{root}/../etc/passwd", None, root=root)
            with self.assertRaises(ValueError):
                client_off.reader_command("/etc/passwd", None, root=root)

    def test_authorized_line_restricts_the_key_and_survives_sshd_dequoting(self):
        line = client_off.observer_authorized_line(self.HOST_KEY, "/workspace/live/progress", "/workspace/live/ckpt")
        self.assertTrue(line.startswith('restrict,command="'))
        self.assertTrue(line.endswith(" " + self.HOST_KEY))
        self.assertNotIn("\n", line)
        quoted = line[len("restrict,command="):]
        # sshd's auth-options dequote: only a backslash before a double quote is an escape.
        dequoted, index = "", 1
        while index < len(quoted):
            if quoted[index] == "\\" and quoted[index + 1] == '"':
                index += 1
            elif quoted[index] == '"':
                break
            dequoted += quoted[index]
            index += 1
        self.assertEqual(dequoted, client_off.reader_command("/workspace/live/progress", "/workspace/live/ckpt"))
        self.assertIn(f" {client_off.PROGRESS_READ_LIMIT}", dequoted)
        for key, progress, checkpoint in (("ssh-rsa AAAA", "/workspace/p", None), (self.HOST_KEY, "/etc/passwd", None),
                                          (self.HOST_KEY, "/workspace/p", "/workspace/p"),
                                          (self.HOST_KEY + " comment", "/workspace/p", None),
                                          (self.HOST_KEY, "/workspace/p", "/workspace/../p")):
            with self.subTest(key=key, progress=progress, checkpoint=checkpoint):
                with self.assertRaises(ValueError):
                    client_off.observer_authorized_line(key, progress, checkpoint)


class ClientIdentityTests(unittest.TestCase):
    def test_client_descriptor_and_attestation_gate_every_mutation(self):
        m = manifest()
        client = {"client_group": m["client_group"], "client_vm_id": "/g/client/vm", "client_group_id": "/g/client",
                  "client_instance_id": A_INSTANCE, "run_id": RUN_ID,
                  "client_sha": m["client_sha"], "client_binary_sha256": m["client_binary_sha256"]}
        self.assertEqual(client_off.validate_client(client), [])
        self.assertTrue(client_off.validate_client({}))
        self.assertTrue(client_off.validate_client(dict(client, client_vm_id="")))
        answers = {}

        class FakeAz:
            def run(self, args, mutating=False, timeout=0):
                return answers.get(args[0])

        good_tags = client_off.client_tags(m)
        good_vm = {"id": "/g/client/vm", "vmId": A_INSTANCE, "tags": dict(good_tags)}
        answers.update(group={"id": "/G/CLIENT", "tags": dict(good_tags)}, vm=dict(good_vm))
        self.assertIsNone(client_off.attest_client(FakeAz(), m, client))
        answers["vm"] = dict(good_vm, id="/g/client/other")
        self.assertIn("not the resource provisioned", client_off.attest_client(FakeAz(), m, client))
        # A same-name recreation keeps the ARM ID and every manifest-derived tag; it
        # cannot keep the instance identity or this run's drawn value.
        answers["vm"] = dict(good_vm, vmId="3f2c9a1e-5d4b-4c6a-8e7f-0a1b2c3d4e5f")
        self.assertIn("not the instance provisioned", client_off.attest_client(FakeAz(), m, client))
        answers["vm"] = dict(good_vm, tags=dict(good_tags, run_id="f" * 32))
        self.assertIn("exactly this run's tags", client_off.attest_client(FakeAz(), m, client))
        answers["vm"] = {"id": "/g/client/vm", "vmId": A_INSTANCE, "tags": {"lane": "azure-client-off", "client_sha": m["client_sha"]}}
        self.assertIn("exactly this run's tags", client_off.attest_client(FakeAz(), m, client))
        answers["vm"] = dict(good_vm)
        answers["group"] = {"id": "/G/CLIENT", "tags": dict(good_tags, purpose="other")}
        self.assertIn("exactly this run's tags", client_off.attest_client(FakeAz(), m, client))
        answers["group"] = {"id": "/G/CLIENT", "tags": dict(good_tags)}
        answers["vm"] = None
        self.assertIn("could not be read", client_off.attest_client(FakeAz(), m, client))
        self.assertIn("does not belong", client_off.attest_client(FakeAz(), m, dict(client, client_sha="f" * 40)))
        self.assertIn("does not belong", client_off.attest_client(FakeAz(), m, dict(client, client_binary_sha256="f" * 64)))
        self.assertIn("client_binary_sha256", client_off.client_tags(m))
        self.assertTrue(client_off.validate_client(dict(client, run_id="short")))
        self.assertTrue(client_off.validate_client(dict(client, client_instance_id="not-a-vmid")))


class OffPhaseTests(unittest.TestCase):
    """The irreversible boundary: which az calls happen, in which order, on which resource."""

    def plane(self, *, a_power="PowerState/running", b_power="PowerState/running", b_vm_id="/g/b/vm",
              client_vm_id="/g/client/vm", image=IMAGE, b_instance=B_INSTANCE, a_instance=A_INSTANCE,
              retag_client_after_deallocate=False, hours_left=3, deadline_in=None):
        # The phases bind themselves to the manifest deadline against real time.
        m = manifest(cleanup_deadline_utc=(client_off.utc_now() + (deadline_in or dt.timedelta(hours=hours_left))).isoformat())
        calls = []
        tags = client_off.client_tags(m)

        class FakeAz:
            dry_run = False
            journal = []
            deadline = None

            def left(self):
                return None if self.deadline is None else self.deadline - time.monotonic()

            def run(self, args, mutating=False, timeout=0):
                calls.append(("mutate" if mutating else "read", tuple(args)))
                if args[:2] == ["vm", "deallocate"] or args[:3] == ["vm", "run-command", "invoke"]:
                    return {}
                if args[:2] == ["group", "show"]:
                    name = args[args.index("-n") + 1]
                    if name == m["client_group"]:
                        return {"id": "/g/client", "name": name, "tags": dict(tags)}
                    return {"id": "/g/b", "name": name, "tags": {}}
                if args[:2] == ["vm", "show"]:
                    group = args[args.index("-g") + 1]
                    if group == m["client_group"]:
                        # Retag once sampling has begun: the reads of A after the mutation are
                        # the deallocation poll, then two per sample.
                        mutated = next((i for i, c in enumerate(calls) if c[0] == "mutate"), None)
                        reads_after = 0 if mutated is None else sum(
                            1 for c in calls[mutated:] if c[1][:2] == ("vm", "show") and m["client_group"] in c[1])
                        retag = retag_client_after_deallocate and reads_after >= 4
                        client_tags = dict(tags, run_id="f" * 32) if retag else dict(tags)
                        return {"id": client_vm_id, "vmId": a_instance, "tags": client_tags}
                    return {"id": b_vm_id, "vmId": b_instance,
                            "tags": {client_off.TAG_IMAGE_REF: client_off.image_ref_digest(image)}}
                if args[:3] == ["network", "public-ip", "show"]:
                    return {"ipAddress": "52.174.10.5"}
                if args[:2] == ["vm", "get-instance-view"]:
                    group = args[args.index("-g") + 1]
                    power = a_power if group == m["client_group"] else b_power
                    # A follows the last power call made on it.
                    if group == m["client_group"]:
                        last = [c[1][1] for c in calls if c[0] == "mutate" and c[1][:2] in (("vm", "deallocate"), ("vm", "start"))]
                        if last:
                            power = "PowerState/deallocated" if last[-1] == "deallocate" else "PowerState/running"
                    return {"instanceView": {"statuses": [{"code": power}]}}
                return None

            def power_state(self, group, name, timeout=0):
                return client_off.Az.power_state(self, group, name, timeout)

            def vm_identity(self, group, name):
                return client_off.Az.vm_identity(self, group, name)

        return m, FakeAz(), calls

    def descriptors(self, m):
        worker = {"vm_name": "worker", "port": 2222, "host_key": "ssh-ed25519 " + "A" * 68, "observer_key_path": __file__,
                  "progress_path": "/workspace/live/progress", "group_id": "/g/b", "vm_id": "/g/b/vm", "instance_id": B_INSTANCE,
                "host": "52.174.10.5"}
        client = {"client_group": m["client_group"], "client_vm_id": "/g/client/vm", "client_group_id": "/g/client",
                  "client_instance_id": A_INSTANCE, "run_id": RUN_ID,
                  "client_sha": m["client_sha"], "client_binary_sha256": m["client_binary_sha256"]}
        return worker, client

    def run_off(self, m, az, worker, client, directory, seconds=0.2):
        counter = iter(range(1, 10_000))
        journal = pathlib.Path(directory) / "journal.ndjson"
        return client_off.phase_off(az, m, worker, client, str(journal), sample_seconds=1, interval_seconds=seconds,
                                    reader=lambda *args: {"progress": next(counter), "checkpoint": None, "channel": "answered", "paths": {"progress": "/workspace/live/progress", "checkpoint": None}})

    def mutations(self, calls):
        return [c[1][:2] for c in calls if c[0] == "mutate"]

    def test_all_gates_pass_then_only_client_a_is_deallocated(self):
        m, az, calls = self.plane()
        worker, client = self.descriptors(m)
        with tempfile.TemporaryDirectory() as directory:
            result = self.run_off(m, az, worker, client, directory)
            journal = (pathlib.Path(directory) / "journal.ndjson").read_text(encoding="utf-8").splitlines()
        self.assertEqual(self.mutations(calls), [("vm", "deallocate")])
        deallocate = next(c[1] for c in calls if c[1][:2] == ("vm", "deallocate"))
        self.assertIn(m["client_group"], deallocate)
        self.assertIn("client", deallocate)
        self.assertNotIn(m["worker_group"], deallocate)
        self.assertIn("baseline", journal[0], "the header is the first record")
        self.assertGreaterEqual(len(journal), 2)
        # A one-second test cadence cannot satisfy the 15-second verdict; the boundary and
        # the journal are what this test proves.
        self.assertFalse(result["passed"])
        self.assertEqual(result["samples"], len(journal) - 1)

    def test_no_gate_failure_reaches_the_deallocate_call(self):
        cases = {
            "bad descriptor": dict(worker_patch={"host_key": "ssh-rsa AAAA"}),
            "foreign client": dict(client_patch={"client_vm_id": "/g/client/other"}),
            "replaced worker": dict(plane={"b_vm_id": "/g/b/other"}),
            "recreated worker": dict(plane={"b_instance": A_INSTANCE}),
            "recreated client": dict(plane={"a_instance": B_INSTANCE}),
            "other image": dict(plane={"image": "x.azurecr.io/horizon-remote-worker@sha256:" + "c" * 64}),
            "worker not running": dict(plane={"b_power": "PowerState/deallocated"}),
            "client already off": dict(plane={"a_power": "PowerState/deallocated"}),
            "observer channel unavailable": dict(reader=lambda *args: {"progress": None, "checkpoint": None, "channel": "unavailable"}),
            "observer key refused": dict(reader=lambda *args: {"progress": None, "checkpoint": None, "channel": "refused"}),
            "observer key not restricted": dict(reader=lambda *args: {"progress": 3, "checkpoint": None, "channel": "unrestricted", "paths": {"progress": "/workspace/live/progress", "checkpoint": None}}),
        }
        for label, case in cases.items():
            with self.subTest(label=label):
                m, az, calls = self.plane(**case.get("plane", {}))
                worker, client = self.descriptors(m)
                worker.update(case.get("worker_patch", {}))
                client.update(case.get("client_patch", {}))
                with tempfile.TemporaryDirectory() as directory:
                    if "reader" in case:
                        result = client_off.phase_off(az, m, worker, client, f"{directory}/j.ndjson", sample_seconds=1,
                                                      interval_seconds=0.2, reader=case["reader"])
                    else:
                        result = self.run_off(m, az, worker, client, directory)
                self.assertFalse(result["passed"], label)
                self.assertEqual(self.mutations(calls), [], f"{label}: a gate failure must never deallocate")

    def test_the_client_wait_hands_each_request_what_is_left_of_the_bound(self):
        m, az, calls = self.plane(a_power="PowerState/running")
        _, client = self.descriptors(m)
        budgets = []
        original = az.run

        def budgeted(args, mutating=False, timeout=0):
            budgets.append(timeout)
            return original(args, mutating, timeout)

        az.run = budgeted
        # Simulated time: sleeps advance the clock instantly and are recorded, so the
        # bound arithmetic runs exactly without a real second passing.
        clock = {"now": 1000.0, "sleeps": []}

        def fake_sleep(seconds):
            clock["sleeps"].append(seconds)
            clock["now"] += seconds

        real_started = time.monotonic()
        with mock.patch.object(client_off.time, "monotonic", lambda: clock["now"]), \
                mock.patch.object(client_off.time, "sleep", fake_sleep):
            # A that never deallocates against a 60-second bound: the wait ends at the
            # bound, every request got a share of what was left, no sleep crossed it.
            problem = client_off.await_client_state(az, m, client, "PowerState/deallocated", bound_seconds=60)
            self.assertIn("did not reach PowerState/deallocated", problem)
            self.assertTrue(budgets and all(1 <= budget <= 60 // 5 for budget in budgets), budgets)
            self.assertEqual(budgets[0], 12, "the first poll shares the whole bound five ways")
            self.assertLessEqual(clock["now"] - 1000.0, 60, "no sleep crossed the bound")
            self.assertTrue(all(0 < sleep <= 5 for sleep in clock["sleeps"]), clock["sleeps"])
            # Below five seconds no poll starts: a poll cannot be cut shorter than its five
            # one-second requests, so it would overrun the bound.
            budgets.clear()
            problem = client_off.await_client_state(az, m, client, "PowerState/deallocated", bound_seconds=4)
            self.assertEqual(budgets, [])
            self.assertIn("did not reach", problem)
        self.assertLess(time.monotonic() - real_started, 3, "the fake answers instantly; only the bound arithmetic runs")

    def test_a_state_counts_only_when_the_identity_around_it_agrees(self):
        m, az, calls = self.plane(a_power="PowerState/deallocated")
        _, client = self.descriptors(m)
        evidence, state = client_off.observe_client_state(az, m, client)
        self.assertEqual((evidence["a_instance_id"], state), (A_INSTANCE, "PowerState/deallocated"))
        # A replaced between the identity read and the state read: the state is unreadable.
        shows = iter([A_INSTANCE, B_INSTANCE])
        original = az.run

        def flipping(args, mutating=False, timeout=0):
            answer = original(args, mutating, timeout)
            if args[:2] == ["vm", "show"] and m["client_group"] in args:
                answer = dict(answer, vmId=next(shows, B_INSTANCE))
            return answer

        az.run = flipping
        evidence, state = client_off.observe_client_state(az, m, client)
        self.assertEqual(evidence["a_instance_id"], A_INSTANCE)
        self.assertIsNone(state, "a state between two different identities belongs to neither")
        # Two identical but failing attestations (a retagged A) yield no state either.
        m, az, calls = self.plane(a_power="PowerState/deallocated")
        _, client = self.descriptors(m)
        _, state = client_off.observe_client_state(az, m, dict(client, client_instance_id=B_INSTANCE))
        self.assertIsNone(state, "a stable identity that is not the provisioned one yields no state")

    def test_off_and_return_read_a_identity_together_with_its_state_around_the_mutation(self):
        # The fake reports A's identity from the plane; every power read of A is preceded
        # by a fresh `vm show` and `group show` of A (the attestation), before the
        # mutation and on every poll after it, and each sample carries A's instance.
        m, az, calls = self.plane()
        worker, client = self.descriptors(m)
        with tempfile.TemporaryDirectory() as directory:
            result = self.run_off(m, az, worker, client, directory)
            journal = (pathlib.Path(directory) / "journal.ndjson").read_text(encoding="utf-8").splitlines()
        client_group = m["client_group"]
        mutation = next(i for i, c in enumerate(calls) if c[0] == "mutate")
        before = [c for c in calls[:mutation] if c[1][:2] == ("vm", "show") and client_group in c[1]]
        self.assertGreaterEqual(len(before), 2, "attested at the gate and again immediately before the call")
        # Immediately before the call: identity, state, identity again (one bound read).
        self.assertEqual([c[1][:2] for c in calls[mutation - 5:mutation]],
                         [("group", "show"), ("vm", "show"), ("vm", "get-instance-view"), ("group", "show"), ("vm", "show")],
                         f"the state read is bracketed by A's identity: {calls[mutation - 6:mutation]}")
        self.assertTrue(all(json.loads(line).get("a_instance_id") == A_INSTANCE for line in journal[1:]), journal[:2])
        expected_tags = client_off.client_tags(m)
        for line in journal[1:]:
            row = json.loads(line)
            self.assertEqual((row["a_group_id"], row["a_vm_id"], row["a_tags"], row["a_group_tags"]),
                             ("/g/client", "/g/client/vm", expected_tags, expected_tags), "the attestation evidence is journaled")
        # A retagged after the deallocation is recorded as unattested in every later sample.
        m, az, calls = self.plane(retag_client_after_deallocate=True)
        worker, client = self.descriptors(m)
        with tempfile.TemporaryDirectory() as directory:
            result = self.run_off(m, az, worker, client, directory, seconds=1.5)
            journal = (pathlib.Path(directory) / "journal.ndjson").read_text(encoding="utf-8").splitlines()
        tags_seen = [json.loads(line).get("a_tags", {}).get("run_id") for line in journal[1:]]
        self.assertIn("f" * 32, tags_seen, journal[:3])
        self.assertFalse(result["passed"])  # the verdict's own rule on the evidence is covered in test_core
        m, az, calls = self.plane(a_power="PowerState/deallocated")
        _, client = self.descriptors(m)
        result = client_off.phase_return(az, m, client)
        self.assertEqual(self.mutations(calls), [("vm", "start")])
        mutation = next(i for i, c in enumerate(calls) if c[0] == "mutate")
        self.assertEqual([c[1][:2] for c in calls[mutation - 5:mutation]],
                         [("group", "show"), ("vm", "show"), ("vm", "get-instance-view"), ("group", "show"), ("vm", "show")],
                         "return brackets A's state with its identity immediately before starting it")

    def test_a_sample_is_unattested_when_b_changes_around_the_reading(self):
        # B's instance flips between the identity read before and after the reading.
        m, az, calls = self.plane()
        worker, client = self.descriptors(m)
        original = az.run

        def flipping(args, mutating=False, timeout=0):
            answer = original(args, mutating, timeout)
            if args[:2] == ["vm", "show"] and m["worker_group"] in args:
                # After the mutation, B's identity reads come in pairs per sample (before
                # and after the reading): the second read of the first sample sees another
                # instance.
                mutated = next((i for i, c in enumerate(calls) if c[0] == "mutate"), None)
                reads_after = 0 if mutated is None else sum(
                    1 for c in calls[mutated:] if c[1][:2] == ("vm", "show") and m["worker_group"] in c[1])
                answer = dict(answer, vmId=A_INSTANCE if reads_after == 2 else B_INSTANCE)
            return answer

        az.run = flipping
        with tempfile.TemporaryDirectory() as directory:
            result = self.run_off(m, az, worker, client, directory, seconds=0.2)
            journal = (pathlib.Path(directory) / "journal.ndjson").read_text(encoding="utf-8").splitlines()
        first = json.loads(journal[1])
        self.assertEqual(first["b_instance_id"], B_INSTANCE, "the identity read before the reading is what the sample names")
        self.assertIsNone(first["b_power"], "a state read between two different B identities belongs to neither")
        self.assertIsNone(first["progress"], "so does the reading")
        self.assertFalse(result["passed"])
        # The observation instant is the start of the sample, before any acquisition.
        m, az, calls = self.plane()
        worker, client = self.descriptors(m)
        slow = az.run

        def slow_run(args, mutating=False, timeout=0):
            if args[:3] == ["network", "public-ip", "show"]:
                time.sleep(0.3)
            return slow(args, mutating, timeout)

        az.run = slow_run
        with tempfile.TemporaryDirectory() as directory:
            self.run_off(m, az, worker, client, directory, seconds=0.2)
            journal = (pathlib.Path(directory) / "journal.ndjson").read_text(encoding="utf-8").splitlines()
        row = json.loads(journal[1])
        lag = (client_off.parse_utc(row["at"]) - client_off.parse_utc(row["scheduled_at"])).total_seconds()
        self.assertLess(lag, 0.25, f"acquisition latency does not make the sample late: {lag}s")

    def test_phases_bind_themselves_to_the_manifest_deadline(self):
        # A deadline that leaves nothing after the reserved margin: nothing is stopped.
        m, az, calls = self.plane(deadline_in=dt.timedelta(minutes=client_off.AFTER_OFF_MINUTES))
        worker, client = self.descriptors(m)
        with tempfile.TemporaryDirectory() as directory:
            result = self.run_off(m, az, worker, client, directory)
        self.assertFalse(result["passed"])
        self.assertTrue(any("reserved margin" in f for f in result["findings"]), result)
        self.assertEqual(self.mutations(calls), [])
        # With time left, the phase deadline is armed on the az client and the return
        # phase keeps the cleanup window: its deadline is earlier than the off phase's.
        m, az, calls = self.plane()
        worker, client = self.descriptors(m)
        with tempfile.TemporaryDirectory() as directory:
            self.run_off(m, az, worker, client, directory)
        off_deadline = az.deadline
        self.assertIsNotNone(off_deadline)
        m2, az2, _ = self.plane(a_power="PowerState/deallocated")
        m2["cleanup_deadline_utc"] = m["cleanup_deadline_utc"]
        client_off.phase_return(az2, m2, client)
        self.assertGreater(az2.deadline, off_deadline, "the return may run later, keeping only the cleanup bound")
        # Past its deadline the az client asks ARM nothing more.
        real = client_off.Az("0f0e0d0c-0b0a-4908-8706-050403020100")
        real.deadline = time.monotonic() - 1
        with mock.patch.object(client_off.subprocess, "run", side_effect=AssertionError("must not be called")):
            self.assertIsNone(real.run(["group", "exists", "-n", "g"]))
        real.deadline = time.monotonic() + 30
        with mock.patch.object(client_off.subprocess, "run") as run:
            run.return_value = mock.Mock(returncode=0, stdout="true")
            real.run(["group", "exists", "-n", "g"], timeout=90)
            self.assertLessEqual(run.call_args.kwargs["timeout"], 30, "a call is cut off at the phase deadline")

    def test_observations_get_what_is_left_of_the_phase_and_none_starts_too_late(self):
        m, az, calls = self.plane()
        worker, client = self.descriptors(m)
        budgets = []

        def reader(*args):
            budgets.append(args[5] if len(args) > 5 else None)
            return {"progress": len(budgets), "checkpoint": None, "channel": "answered", "paths": {"progress": "/workspace/live/progress", "checkpoint": None}}

        with tempfile.TemporaryDirectory() as directory:
            client_off.phase_off(az, m, worker, client, f"{directory}/j.ndjson", sample_seconds=1, interval_seconds=0.2,
                                 reader=reader)
        self.assertTrue(budgets and all(b is not None and 0 < b <= client_off.OBSERVATION_SECONDS for b in budgets), budgets)
        # With the phase deadline only seconds away, the probe is not even started.
        m, az, calls = self.plane(deadline_in=dt.timedelta(minutes=client_off.AFTER_OFF_MINUTES, seconds=70))
        worker, client = self.descriptors(m)
        budgets.clear()
        with tempfile.TemporaryDirectory() as directory:
            result = client_off.phase_off(az, m, worker, client, f"{directory}/j.ndjson", sample_seconds=1,
                                          interval_seconds=0.2, reader=reader)
        self.assertEqual(self.mutations(calls), [], "an interval that cannot fit before the deadline stops nothing")
        self.assertTrue(any("do not fit" in f for f in result["findings"]), result)
        # read_observations itself refuses a budget below the minimum without dialling.
        with mock.patch.object(client_off.subprocess, "Popen", side_effect=AssertionError("must not dial")):
            self.assertEqual(client_off.read_observations("52.174.10.5", 2222, "ssh-ed25519 " + "A" * 68, "/dev/null", "/tmp", 5)["channel"],
                             "unavailable")

    def test_a_sample_from_a_channel_that_is_not_the_restricted_reader_is_unreadable(self):
        m, az, calls = self.plane()
        worker, client = self.descriptors(m)
        answers = iter([{"progress": 1, "checkpoint": None, "channel": "answered", "paths": {"progress": "/workspace/live/progress", "checkpoint": None}}])  # the gate probe
        later = {"progress": 9, "checkpoint": None, "channel": "unrestricted", "paths": {"progress": "/workspace/live/progress", "checkpoint": None}}
        with tempfile.TemporaryDirectory() as directory:
            client_off.phase_off(az, m, worker, client, f"{directory}/j.ndjson", sample_seconds=1, interval_seconds=0.2,
                                 reader=lambda *args: next(answers, later))
            journal = (pathlib.Path(directory) / "j.ndjson").read_text(encoding="utf-8").splitlines()
        rows = [json.loads(line) for line in journal[1:]]
        self.assertTrue(rows and all(row["progress"] is None and row["b_power"] is None for row in rows), rows)
        # A dry run keeps its plan in memory and leaves the production journal path alone.
        m, az, calls = self.plane()
        az.dry_run = True
        worker, client = self.descriptors(m)
        with tempfile.TemporaryDirectory() as directory:
            result = client_off.phase_off(az, m, worker, client, f"{directory}/j.ndjson",
                                          reader=lambda *args: {"progress": 1, "checkpoint": None, "channel": "answered", "paths": {"progress": "/workspace/live/progress", "checkpoint": None}})
            self.assertFalse(os.path.exists(f"{directory}/j.ndjson"), "no header is left behind")
        self.assertTrue(result["dry_run"])

    def test_a_refusal_before_the_deallocation_leaves_no_journal_behind(self):
        m, az, calls = self.plane()
        worker, client = self.descriptors(m)
        with tempfile.TemporaryDirectory() as directory:
            result = client_off.phase_off(az, m, worker, client, f"{directory}/j.ndjson", sample_seconds=1,
                                          interval_seconds=0.2,
                                          reader=lambda *args: {"progress": None, "checkpoint": None, "channel": "unavailable"})
            self.assertFalse(result["passed"])
            self.assertFalse(os.path.exists(f"{directory}/j.ndjson"), "the same path can be retried")
            self.assertEqual(self.mutations(calls), [])
            # Once the deallocation is attempted, the header exists first.
            result = self.run_off(m, az, worker, client, directory)
            self.assertIn("baseline", (pathlib.Path(directory) / "journal.ndjson").read_text(encoding="utf-8").splitlines()[0])

    def test_a_sample_reads_the_client_before_the_worker_and_the_observation(self):
        m, az, calls = self.plane()
        worker, client = self.descriptors(m)
        with tempfile.TemporaryDirectory() as directory:
            self.run_off(m, az, worker, client, directory, seconds=0.2)
        mutation = next(i for i, c in enumerate(calls) if c[0] == "mutate")
        after = [c[1] for c in calls[mutation + 1:]]
        # The first worker read after the deallocation belongs to the first sample, and
        # the five reads right before it are A's bracketed state: A is read first.
        worker_first = next(i for i, c in enumerate(after) if m["worker_group"] in c)
        bracket = [c[:2] for c in after[worker_first - 5:worker_first]]
        self.assertEqual(bracket, [("group", "show"), ("vm", "show"), ("vm", "get-instance-view"), ("group", "show"), ("vm", "show")])
        self.assertTrue(all(m["client_group"] in c for c in after[worker_first - 5:worker_first]))

    def test_a_journal_that_cannot_be_written_is_removed_again(self):
        m, az, calls = self.plane()
        worker, client = self.descriptors(m)
        real_fsync = os.fsync

        def failing_fsync(fd):
            raise OSError("disk full")

        with tempfile.TemporaryDirectory() as directory, mock.patch.object(client_off.os, "fsync", failing_fsync):
            result = self.run_off(m, az, worker, client, directory)
            self.assertFalse(os.path.exists(f"{directory}/journal.ndjson"), "the path stays retryable")
        self.assertFalse(result["passed"])
        self.assertTrue(any("cannot be written" in f for f in result["findings"]), result)
        self.assertEqual(self.mutations(calls), [], "nothing was stopped")
        os.fsync = real_fsync

    def test_the_slot_at_the_end_of_the_interval_is_still_sampled(self):
        m, az, calls = self.plane()
        worker, client = self.descriptors(m)
        with tempfile.TemporaryDirectory() as directory:
            result = self.run_off(m, az, worker, client, directory, seconds=3)
            journal = (pathlib.Path(directory) / "journal.ndjson").read_text(encoding="utf-8").splitlines()
        self.assertEqual(result["samples"], 4, "slots at 0, 1, 2 and 3 seconds: the interval is closed on its end")
        first, last = json.loads(journal[1]), json.loads(journal[-1])
        span = (client_off.parse_utc(last["scheduled_at"]) - client_off.parse_utc(first["scheduled_at"])).total_seconds()
        self.assertEqual(span, 3)

    def test_dry_run_off_journals_the_intended_call_and_never_waits(self):
        m, az, calls = self.plane()
        worker, client = self.descriptors(m)
        az.dry_run = True
        started = time.monotonic()
        with tempfile.TemporaryDirectory() as directory:
            result = client_off.phase_off(az, m, worker, client, f"{directory}/j.ndjson",
                                          reader=lambda *args: {"progress": 1, "checkpoint": None, "channel": "answered", "paths": {"progress": "/workspace/live/progress", "checkpoint": None}})
        self.assertLess(time.monotonic() - started, 5, "a dry run returns without polling for a state it never caused")
        self.assertFalse(result["passed"])
        self.assertTrue(result["dry_run"])
        self.assertEqual(self.mutations(calls), [("vm", "deallocate")], "the intended call is journaled, not issued")

    def test_install_appends_the_restricted_line_through_run_command_and_proves_it(self):
        m, az, calls = self.plane()
        worker, _ = self.descriptors(m)
        answers = iter([{"progress": None, "checkpoint": None, "channel": "refused"}, {"progress": 3, "checkpoint": None, "channel": "answered", "paths": {"progress": "/workspace/live/progress", "checkpoint": None}}])
        az.edit_container_authorized_keys = lambda group, name, line, action: client_off.Az.edit_container_authorized_keys(
            az, group, name, line, action)
        az.append_container_authorized_key = lambda group, name, line: client_off.Az.append_container_authorized_key(
            az, group, name, line)
        with tempfile.TemporaryDirectory() as directory:
            private, public, public_key = self.observer_pair(directory)
            result = client_off.phase_install_observer(az, m, dict(worker, observer_key_path=private), public, directory,
                                                       reader=lambda *args: next(answers))
        self.assertEqual(result, {"passed": True, "installed": True, "progress": 3, "checkpoint": None})
        invoke = [c[1] for c in calls if c[0] == "mutate"]
        self.assertEqual([c[:3] for c in invoke], [("vm", "run-command", "invoke")])
        self.assertIn(m["worker_group"], invoke[0])
        script = invoke[0][invoke[0].index("--scripts") + 1]
        self.assertIn("docker exec horizon-worker", script)
        self.assertIn("python3 - append ", script, "the editor runs in the container, relative to a verified /root/.ssh")
        line = base64.b64decode(script.rsplit(" ", 1)[1].rstrip("'")).decode("ascii")
        self.assertEqual(line, client_off.observer_authorized_line(public_key, worker["progress_path"], None))
        program = base64.b64decode(script.split("echo ")[1].split(" |")[0]).decode("ascii")
        self.assertIn("O_NOFOLLOW", program)
        self.assertIn("dir_fd=ssh", program)
        # The worker is attested immediately before the append and again after it.
        mutation_index = next(i for i, c in enumerate(calls) if c[0] == "mutate")
        shows = [i for i, c in enumerate(calls) if c[0] == "read" and c[1][:2] == ("vm", "show") and m["worker_group"] in c[1]]
        self.assertTrue(any(i < mutation_index for i in shows) and any(i > mutation_index for i in shows), calls)

    def test_install_reports_an_unanswered_append_by_what_the_reader_proves(self):
        for answers, installed, passed in (([{"progress": None, "checkpoint": None, "channel": "refused"}, {"progress": 5, "checkpoint": None, "channel": "answered", "paths": {"progress": "/workspace/live/progress", "checkpoint": None}}], True, True),
                                           ([{"progress": None, "checkpoint": None, "channel": "refused"}, {"progress": None, "checkpoint": None, "channel": "refused"}], "unknown", False)):
            with self.subTest(installed=installed):
                m, az, calls = self.plane()
                worker, _ = self.descriptors(m)
                az.append_container_authorized_key = lambda group, name, line: None  # answer lost, not a dry run
                del calls[:]
                replies = iter(answers)
                with tempfile.TemporaryDirectory() as directory:
                    private, public, _ = self.observer_pair(directory)
                    result = client_off.phase_install_observer(az, m, dict(worker, observer_key_path=private), public,
                                                               directory, reader=lambda *args: next(replies))
                self.assertEqual((result["passed"], result["installed"]), (passed, installed), result)
                if not passed:
                    self.assertIn("retry could append a second line", result["findings"][0])
                shows = [c for c in calls if c[1][:2] == ("vm", "show") and m["worker_group"] in c[1]]
                self.assertGreaterEqual(len(shows), 2, "B is attested before the append and again before the reader is believed")
        m, az, calls = self.plane(b_vm_id="/g/b/other")
        worker, _ = self.descriptors(m)
        az.append_container_authorized_key = lambda group, name, line: None
        with tempfile.TemporaryDirectory() as directory:
            private, public, _ = self.observer_pair(directory)
            result = client_off.phase_install_observer(az, m, dict(worker, observer_key_path=private), public, directory,
                                                       reader=lambda *args: {"progress": 5, "checkpoint": None, "channel": "answered", "paths": {"progress": "/workspace/live/progress", "checkpoint": None}})
        self.assertFalse(result["passed"], "a reader answering from a replaced B proves nothing")
        m, az, calls = self.plane()
        worker, _ = self.descriptors(m)
        az.dry_run = True
        az.edit_container_authorized_keys = lambda group, name, line, action: client_off.Az.edit_container_authorized_keys(
            az, group, name, line, action)
        az.append_container_authorized_key = lambda group, name, line: client_off.Az.append_container_authorized_key(
            az, group, name, line)
        with tempfile.TemporaryDirectory() as directory:
            private, public, _ = self.observer_pair(directory)
            result = client_off.phase_install_observer(az, m, dict(worker, observer_key_path=private), public, directory,
                                                       reader=lambda *args: {"progress": None, "checkpoint": None, "channel": "refused"})
        self.assertTrue(result["dry_run"] and not result["installed"] and not result["passed"], result)
        self.assertEqual([c[1][:3] for c in calls if c[0] == "mutate"], [("vm", "run-command", "invoke")], "journaled, not issued")

    def test_install_refuses_a_public_key_that_is_not_the_observer_private_keys(self):
        m, az, calls = self.plane()
        worker, _ = self.descriptors(m)
        with tempfile.TemporaryDirectory() as directory:
            private, _, _ = self.observer_pair(directory)
            other = pathlib.Path(directory) / "other.pub"
            other.write_text("ssh-ed25519 " + "B" * 68 + " other\n", encoding="utf-8")
            result = client_off.phase_install_observer(az, m, dict(worker, observer_key_path=private), str(other), directory,
                                                       reader=lambda *args: {"progress": None, "checkpoint": None, "channel": "refused"})
        self.assertFalse(result["passed"])
        self.assertIn("not the observer private key's", result["findings"][0])
        self.assertEqual(self.mutations(calls), [], "a foreign key is never appended")

    def test_install_applies_the_off_phase_identity_gates_before_the_append(self):
        with tempfile.TemporaryDirectory() as directory:
            public = pathlib.Path(directory) / "observer.pub"
            public.write_text("ssh-ed25519 " + "B" * 68 + "\n", encoding="utf-8")
            cases = {"replaced worker": dict(plane={"b_vm_id": "/g/b/other"}),
                     "other image": dict(plane={"image": "x.azurecr.io/horizon-remote-worker@sha256:" + "c" * 64}),
                     "worker not running": dict(plane={"b_power": "PowerState/deallocated"}),
                     "stale address": dict(worker_patch={"host": "52.174.10.6"})}
            for label, case in cases.items():
                with self.subTest(label=label):
                    m, az, calls = self.plane(**case.get("plane", {}))
                    worker, _ = self.descriptors(m)
                    worker.update(case.get("worker_patch", {}))
                    result = client_off.phase_install_observer(az, m, worker, str(public), directory,
                                                               reader=lambda *args: {"progress": None, "checkpoint": None, "channel": "refused"})
                    self.assertFalse(result["passed"], label)
                    self.assertEqual(self.mutations(calls), [], f"{label}: never a run-command on another VM")

    def test_install_never_appends_when_the_key_already_answers_or_the_gates_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            private, public, _ = self.observer_pair(directory)
            m, az, calls = self.plane()
            worker, _ = self.descriptors(m)
            worker["observer_key_path"] = private
            result = client_off.phase_install_observer(az, m, worker, str(public), directory,
                                                       reader=lambda *args: {"progress": 1, "checkpoint": None, "channel": "answered", "paths": {"progress": "/workspace/live/progress", "checkpoint": None}})
            self.assertTrue(result["passed"] and not result["installed"])
            self.assertEqual(self.mutations(calls), [])
            # A probe that neither answers nor refuses authorizes nothing: an append on top
            # of an unknown state could stack a second line.
            for unknown in ({"progress": None, "checkpoint": None, "channel": "unavailable"},
                            {"progress": None, "checkpoint": None, "channel": "answered", "paths": {"progress": "/workspace/live/progress", "checkpoint": None}}):
                result = client_off.phase_install_observer(az, m, worker, str(public), directory, reader=lambda *args: unknown)
                self.assertEqual(self.mutations(calls), [], unknown)
                if unknown["channel"] == "unavailable":
                    self.assertEqual((result["passed"], result["installed"]), (False, "unknown"), result)
                else:
                    self.assertTrue(result["passed"] and not result["installed"], "an answer with missing files is still installed")
            result = client_off.phase_install_observer(az, m, dict(worker, host_key="ssh-rsa AAAA"), str(public), directory,
                                                       reader=lambda *args: {"progress": None, "checkpoint": None, "channel": "refused"})
            self.assertFalse(result["passed"])
            self.assertEqual(self.mutations(calls), [])

    def observer_pair(self, directory):
        """A real Ed25519 pair for observer C, so the pair check runs for real."""
        private = pathlib.Path(directory) / "observer"
        subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "observer@c", "-f", str(private)], check=True)
        public_key = " ".join(private.with_suffix(".pub").read_text(encoding="utf-8").split()[:2])
        return str(private), str(private.with_suffix(".pub")), public_key

    def test_the_key_editor_appends_and_removes_relative_to_a_verified_directory(self):
        line = client_off.observer_authorized_line("ssh-ed25519 " + "A" * 68, "/workspace/live/progress", None)
        encoded = base64.b64encode(line.encode("ascii")).decode("ascii")
        with tempfile.TemporaryDirectory() as root:
            os.makedirs(f"{root}/.ssh", mode=0o700)
            program = client_off.AUTHORIZED_KEYS_EDITOR.replace('os.open("/root", flags)', f'os.open("{root}", flags)')
            keys = pathlib.Path(root) / ".ssh" / "authorized_keys"
            keys.write_text("ssh-ed25519 " + "B" * 68 + " client\n", encoding="utf-8")

            def run(action):
                return subprocess.run([sys.executable, "-", action, encoded], input=program, text=True, capture_output=True, check=False)

            self.assertEqual(run("append").returncode, 0)
            self.assertEqual(keys.read_text(encoding="utf-8").splitlines()[1], line)
            self.assertEqual(run("remove").returncode, 0)
            self.assertEqual(keys.read_text(encoding="utf-8"), "ssh-ed25519 " + "B" * 68 + " client\n", "only the observer line went")
            self.assertEqual(sorted(os.listdir(f"{root}/.ssh")), ["authorized_keys"], "no temporary left behind")
            # A symlinked .ssh, or a symlinked key file, is never followed.
            os.rename(f"{root}/.ssh", f"{root}/elsewhere")
            os.symlink(f"{root}/elsewhere", f"{root}/.ssh")
            self.assertEqual(run("append").returncode, 2)
            os.unlink(f"{root}/.ssh")
            os.rename(f"{root}/elsewhere", f"{root}/.ssh")
            os.rename(keys, f"{root}/real_keys")
            os.symlink(f"{root}/real_keys", keys)
            self.assertEqual(run("append").returncode, 2)
            self.assertEqual(pathlib.Path(f"{root}/real_keys").read_text(encoding="utf-8").count("\n"), 1, "the target was not written")
            # A hard link or a FIFO at the key path is refused as well; the other file is untouched.
            os.unlink(keys)
            os.link(f"{root}/real_keys", keys)
            self.assertEqual(run("append").returncode, 3)
            self.assertEqual(pathlib.Path(f"{root}/real_keys").read_text(encoding="utf-8").count("\n"), 1)
            os.unlink(keys)
            os.mkfifo(keys)
            self.assertEqual(run("append").returncode, 3, "a FIFO never blocks the editor")
            os.unlink(keys)
            # Appending to a missing file creates it; a long line is written in full.
            long_line = line + " " + "x" * 50_000
            long_encoded = base64.b64encode(long_line.encode("ascii")).decode("ascii")
            self.assertEqual(subprocess.run([sys.executable, "-", "append", long_encoded], input=program, text=True,
                                            capture_output=True, check=False).returncode, 0)
            self.assertEqual(keys.read_text(encoding="utf-8"), long_line + "\n")
            self.assertEqual(oct(keys.stat().st_mode)[-3:], "600")

    def test_remove_observer_key_is_reconciled_by_the_reader(self):
        for answers, passed, removed in (([{"progress": None, "checkpoint": None, "channel": "refused"}], True, True),
                                         ([{"progress": 4, "checkpoint": None, "channel": "answered", "paths": {"progress": "/workspace/live/progress", "checkpoint": None}}], False, False),
                                         ([{"progress": None, "checkpoint": None, "channel": "answered", "paths": {"progress": "/workspace/live/progress", "checkpoint": None}}], False, False),
                                         ([{"progress": None, "checkpoint": None, "channel": "unavailable"}], False, "unknown")):
            with self.subTest(channel=answers[0]["channel"]):
                m, az, calls = self.plane()
                worker, _ = self.descriptors(m)
                az.edit_container_authorized_keys = lambda group, name, line, action: client_off.Az.edit_container_authorized_keys(
                    az, group, name, line, action)
                az.remove_container_authorized_key = lambda group, name, line: client_off.Az.remove_container_authorized_key(
                    az, group, name, line)
                replies = iter(answers)
                with tempfile.TemporaryDirectory() as directory:
                    private, public, public_key = self.observer_pair(directory)
                    result = client_off.phase_remove_observer(az, m, dict(worker, observer_key_path=private), public, directory,
                                                              reader=lambda *args: next(replies))
                self.assertEqual((result["passed"], result["removed"]), (passed, removed), result)
                invoke = [c[1] for c in calls if c[0] == "mutate"]
                self.assertEqual([c[:3] for c in invoke], [("vm", "run-command", "invoke")])
                script = invoke[0][invoke[0].index("--scripts") + 1]
                self.assertIn("python3 - remove ", script)
                self.assertEqual(base64.b64decode(script.rsplit(" ", 1)[1].rstrip("'")).decode("ascii"),
                                 client_off.observer_authorized_line(public_key, worker["progress_path"], None))
                program = base64.b64decode(script.split("echo ")[1].split(" |")[0]).decode("ascii")
                self.assertIn("os.replace(name, \"authorized_keys\", src_dir_fd=ssh, dst_dir_fd=ssh)", program)
        m, az, calls = self.plane(b_vm_id="/g/b/other")
        worker, _ = self.descriptors(m)
        with tempfile.TemporaryDirectory() as directory:
            private, public, _ = self.observer_pair(directory)
            result = client_off.phase_remove_observer(az, m, dict(worker, observer_key_path=private), public, directory,
                                                      reader=lambda *args: {"progress": None, "checkpoint": None, "channel": "refused"})
        self.assertFalse(result["passed"])
        self.assertEqual(self.mutations(calls), [], "a replaced worker is never touched")
        # A refusal from a B that changed identity during the probe proves nothing.
        m, az, calls = self.plane()
        worker, _ = self.descriptors(m)
        az.remove_container_authorized_key = lambda group, name, line: {}
        original = az.run
        state = {"probed": False}

        def flipping(args, mutating=False, timeout=0):
            answer = original(args, mutating, timeout)
            if args[:2] == ["vm", "show"] and m["worker_group"] in args and state["probed"]:
                answer = dict(answer, vmId=A_INSTANCE)
            return answer

        def refusing(*args):
            state["probed"] = True
            return {"progress": None, "checkpoint": None, "channel": "refused"}

        az.run = flipping
        with tempfile.TemporaryDirectory() as directory:
            private, public, _ = self.observer_pair(directory)
            result = client_off.phase_remove_observer(az, m, dict(worker, observer_key_path=private), public, directory,
                                                      reader=refusing)
        self.assertEqual((result["passed"], result["removed"]), (False, "unknown"), result)
        m, az, calls = self.plane()
        worker, _ = self.descriptors(m)
        with tempfile.TemporaryDirectory() as directory:
            private, _, _ = self.observer_pair(directory)
            other = pathlib.Path(directory) / "other.pub"
            other.write_text("ssh-ed25519 " + "B" * 68 + " other\n", encoding="utf-8")
            result = client_off.phase_remove_observer(az, m, dict(worker, observer_key_path=private), str(other), directory,
                                                      reader=lambda *args: {"progress": None, "checkpoint": None, "channel": "refused"})
        self.assertFalse(result["passed"])
        self.assertEqual(self.mutations(calls), [], "somebody else's line is never removed")

    def test_return_requires_a_deallocated_client(self):
        m, az, calls = self.plane(a_power="PowerState/running")
        _, client = self.descriptors(m)
        result = client_off.phase_return(az, m, client)
        self.assertFalse(result["passed"])
        self.assertEqual(self.mutations(calls), [], "a running client is never re-started")
        m, az, calls = self.plane(a_power="PowerState/deallocated")
        az.dry_run = True
        started = time.monotonic()
        result = client_off.phase_return(az, m, client)
        self.assertLess(time.monotonic() - started, 5, "a dry run never waits for a start it did not issue")
        self.assertTrue(result["dry_run"])
        self.assertEqual(self.mutations(calls), [("vm", "start")], "the intended call is journaled, not issued")

    def test_existing_journal_blocks_the_mutation(self):
        m, az, calls = self.plane()
        worker, client = self.descriptors(m)
        with tempfile.TemporaryDirectory() as directory:
            (pathlib.Path(directory) / "journal.ndjson").write_text("old\n", encoding="utf-8")
            result = self.run_off(m, az, worker, client, directory)
        self.assertFalse(result["passed"])
        self.assertEqual(self.mutations(calls), [])


if __name__ == "__main__":
    unittest.main()
