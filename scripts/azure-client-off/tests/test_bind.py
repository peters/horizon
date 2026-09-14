"""The unbound manifest state, `bind-worker`, the unbound cleanup mode and the
observer-removal bound: real files, a scripted `az`, no Azure, no network."""
from __future__ import annotations

import json
import os
import shutil
import tempfile
import time
import unittest
from typing import Any, Dict, List, Optional

from harness_fixtures import ADAPTER_TAGS, B_GROUP_ID, B_VM_ID, JOB_ID, RUN_ID, WORKFLOW_ID, cli, client_off, manifest, record

B_GROUP = f"horizon-ws-{WORKFLOW_ID}-{JOB_ID}"
B_JOURNALED = {"name": B_GROUP, "id": B_GROUP_ID, "tags": dict(ADAPTER_TAGS)}


def unbound(**overrides):
    return manifest(worker_group=client_off.UNBOUND, **overrides)


def descriptor(m, **overrides):
    base = {"run_id": m["run_id"], "manifest_sha256": client_off.manifest_digest(m)}
    base.update(overrides)
    return base


class FakeAz:
    """Answers by command prefix; every call is journaled like the real client."""

    def __init__(self, answers: Dict[str, Any], dry_run: bool = False):
        self.answers, self.dry_run, self.journal, self.deadline = answers, dry_run, [], None
        self.calls: List[List[str]] = []

    on_call = None

    def run(self, args: List[str], mutating: bool = False, timeout: float = 90) -> Optional[Any]:
        self.calls.append(list(args))
        if self.on_call is not None:
            self.on_call(args)
        self.journal.append({"mutating": mutating, "args": list(args)})
        if self.dry_run and mutating:
            return None
        key = " ".join(args)
        for prefix, answer in self.answers.items():
            if key.startswith(prefix):
                return answer
        return None

    def left(self):
        return None


def healthy_answers(m, tagged_after=True):
    """A control plane where B's group, its worker VM and the tag write all behave."""
    deadline = m["cleanup_deadline_utc"]
    return {
        f"group show -n {B_GROUP}": dict(B_JOURNALED, location="northeurope"),
        f"vm show --ids {B_VM_ID} --query tags": ({"purpose": "horizon-azure-vm-spike", "deadline": deadline, **ADAPTER_TAGS}
                                                  if tagged_after else dict(ADAPTER_TAGS)),
        f"vm show --ids {B_VM_ID}": {"id": B_VM_ID, "vmId": "3f2c9a1e-5d4b-4c6a-8e7f-0a1b2c3d4e5f", "tags": dict(ADAPTER_TAGS)},
        "tag update": {"properties": {"tags": {}}},
    }


class UnboundManifestTests(unittest.TestCase):
    def test_unbound_is_accepted_only_by_commands_that_do_not_act_on_b(self):
        m = unbound()
        for phase in ("validate", "journal-group", "bind-worker", "cleanup", "observer-key-line"):
            with self.subTest(phase=phase):
                self.assertEqual(client_off.validate_manifest(m, now=client_off.utc_now(), renting=False, phase=phase), [])
        for phase in client_off.BOUND_COMMANDS:
            with self.subTest(phase=phase):
                problems = client_off.validate_manifest(m, now=client_off.utc_now(), renting=False, phase=phase)
                self.assertTrue(any("unbound" in problem and "bind-worker" in problem for problem in problems), problems)
        self.assertFalse(client_off.is_bound(m))
        self.assertTrue(client_off.is_bound(manifest()))

    def test_a_bound_manifest_is_validated_exactly_as_before(self):
        self.assertEqual(client_off.validate_manifest(manifest(), now=client_off.utc_now(), renting=False, phase="off"), [])
        broken = manifest(worker_group="horizon-ws-not-uuids")
        problems = client_off.validate_manifest(broken, now=client_off.utc_now(), renting=False, phase="off")
        self.assertTrue(any("horizon-ws-<workflow>-<job>" in problem for problem in problems), problems)

    def test_the_unbound_digest_ignores_the_binding_and_nothing_else(self):
        m = unbound()
        bound = dict(m, worker_group=B_GROUP)
        self.assertEqual(client_off.manifest_digest(m), client_off.manifest_digest(bound))
        self.assertNotEqual(client_off.manifest_digest(m), client_off.manifest_digest(dict(m, off_minutes=13)))
        self.assertNotEqual(client_off.manifest_digest(bound, bound=True), client_off.manifest_digest(m, bound=True))


class BindWorkerTests(unittest.TestCase):
    def test_binding_journals_before_the_mutation_and_writes_worker_group_last(self):
        m = unbound()
        az = FakeAz(healthy_answers(m))
        steps: List[str] = []
        az.on_call = lambda args: steps.append("mutate:" + " ".join(args[:2]) if False else " ".join(args[:2]))
        journals: List[Any] = []

        def persist(journal):
            journals.append(journal)
            steps.append("journal")

        result = client_off.bind_worker(az, m, B_GROUP, ["horizon-worker-registry"],
                                        [record(m["client_group"], run_id=RUN_ID)], descriptor(m), persist)
        self.assertTrue(result["passed"], result)
        self.assertEqual(result["worker_group"], B_GROUP)
        self.assertEqual(result["manifest"]["worker_group"], B_GROUP)
        self.assertEqual([r["name"] for r in journals[0]], [m["client_group"], B_GROUP])
        self.assertEqual(journals[0][1], B_JOURNALED,
                         "the record is ARM's identity and tag set, as journal-group writes it")
        self.assertEqual(result["bound_manifest_sha256"], client_off.manifest_digest(result["manifest"], bound=True))
        mutations = [call for call in az.journal if call["mutating"]]
        self.assertEqual(len(mutations), 1)
        self.assertEqual(mutations[0]["args"][:6], ["tag", "update", "--resource-id", B_VM_ID, "--operation", "merge"])
        self.assertIn(f"deadline={m['cleanup_deadline_utc']}", mutations[0]["args"])
        self.assertIn("purpose=horizon-azure-vm-spike", mutations[0]["args"])
        # Identity reads, then the journal, then the one mutation and its read-back: a
        # crash at any point leaves either no tag or a deletable, journaled worker.
        self.assertEqual(steps, ["group show", "vm show", "journal", "tag update", "vm show"])

    def test_a_failing_journal_write_leaves_the_worker_untagged_and_unbound(self):
        m = unbound()
        az = FakeAz(healthy_answers(m))

        def refuse(_journal):
            raise OSError("synthetic journal failure")

        result = client_off.bind_worker(az, m, B_GROUP, [], [], descriptor(m), refuse)
        self.assertFalse(result["passed"])
        self.assertFalse(result["bound"])
        self.assertTrue(any("journal could not be written" in finding for finding in result["findings"]), result)
        self.assertEqual([call for call in az.journal if call["mutating"]], [], "nothing tagged without the record")

    def test_a_failed_read_back_still_leaves_the_group_journaled(self):
        m = unbound()
        az = FakeAz(healthy_answers(m, tagged_after=False))
        journals: List[Any] = []
        result = client_off.bind_worker(az, m, B_GROUP, [], [], descriptor(m), journals.append)
        self.assertFalse(result["passed"])
        self.assertEqual(result["journaled"], B_JOURNALED)
        self.assertEqual([r["name"] for r in journals[0]], [B_GROUP], "the tagged worker is deletable")

    def test_every_local_refusal_precedes_any_read_or_write(self):
        m = unbound()
        good = descriptor(m)
        cases = {
            "already bound to another": (manifest(worker_group=f"horizon-ws-{JOB_ID}-{WORKFLOW_ID}"), B_GROUP, [], [], good),
            "already bound to this": (manifest(), B_GROUP, [], [], good),
            "not an adapter name": (m, "horizon-ws-short", [], [], good),
            "pre-existing": (m, B_GROUP, [B_GROUP.upper()], [], good),
            "malformed inventory": (m, B_GROUP, "no", [], good),
            "malformed journal": (m, B_GROUP, [], [1], good),
            "no descriptor digest": (m, B_GROUP, [], [], {"run_id": RUN_ID}),
            "another run": (m, B_GROUP, [], [], descriptor(m, run_id="f" * 32)),
            "manifest changed since provisioning": (m, B_GROUP, [], [], descriptor(unbound(off_minutes=14))),
        }
        for name, (mani, group, before, created, client) in cases.items():
            with self.subTest(case=name):
                az = FakeAz(healthy_answers(mani))
                result = client_off.bind_worker(az, mani, group, before, created, client)
                self.assertFalse(result["passed"], name)
                self.assertFalse(result["bound"])
                self.assertEqual(az.calls, [], "refused before any ARM call")

    def test_arm_refusals_never_tag_or_bind(self):
        m = unbound()
        wrong_tags = healthy_answers(m)
        wrong_tags[f"group show -n {B_GROUP}"] = {"name": B_GROUP, "id": B_GROUP_ID,
                                                  "tags": {"horizon-workflow-id": WORKFLOW_ID}}
        vm_absent = healthy_answers(m)
        del vm_absent[f"vm show --ids {B_VM_ID}"]
        vm_absent[f"vm show --ids {B_VM_ID} --query tags"] = None
        foreign_vm = healthy_answers(m)
        foreign_vm[f"vm show --ids {B_VM_ID}"] = {"id": B_VM_ID,
                                                  "tags": {"horizon-workflow-id": JOB_ID, "horizon-job-id": WORKFLOW_ID}}
        unreadable = dict(healthy_answers(m))
        del unreadable[f"group show -n {B_GROUP}"]
        for name, answers in {"group without adapter tags": wrong_tags, "worker VM not yet present": vm_absent,
                              "worker VM of another workspace": foreign_vm, "group unreadable": unreadable}.items():
            with self.subTest(case=name):
                az = FakeAz(answers)
                result = client_off.bind_worker(az, m, B_GROUP, [], [], descriptor(m))
                self.assertFalse(result["passed"], name)
                self.assertFalse(result["bound"])
                self.assertEqual([call for call in az.journal if call["mutating"]], [], "nothing tagged")
        # A journal record for the same group must carry the same identity.
        az = FakeAz(healthy_answers(m))
        other = {"name": B_GROUP, "id": B_GROUP_ID + "-other", "tags": dict(ADAPTER_TAGS)}
        result = client_off.bind_worker(az, m, B_GROUP, [], [other], descriptor(m))
        self.assertFalse(result["passed"])
        self.assertEqual([call for call in az.journal if call["mutating"]], [])

    def test_a_group_already_journaled_by_journal_group_binds_without_a_second_record(self):
        m = unbound()
        az = FakeAz(healthy_answers(m))
        journals: List[Any] = []
        result = client_off.bind_worker(az, m, B_GROUP, [], [B_JOURNALED], descriptor(m), journals.append)
        self.assertTrue(result["passed"], result)
        self.assertEqual(journals[0], [B_JOURNALED], "the journal is append-only and holds B once")

    def test_a_dry_run_journals_nothing_tags_nothing_and_binds_nothing(self):
        m = unbound()
        dry = FakeAz(healthy_answers(m), dry_run=True)
        journals: List[Any] = []
        result = client_off.bind_worker(dry, m, B_GROUP, [], [], descriptor(m), journals.append)
        self.assertFalse(result["passed"])
        self.assertTrue(result["dry_run"])
        self.assertEqual(result["would_bind"], B_GROUP)
        self.assertNotIn("manifest", result)
        self.assertEqual(journals, [])
        self.assertEqual([call for call in dry.journal if call["mutating"]], [])

    def test_the_command_writes_the_journal_before_the_manifest_and_refuses_twice(self):
        m = unbound()
        directory = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, directory, ignore_errors=True)
        paths = {name: os.path.join(directory, name) for name in ("m.json", "groups.json", "created.json", "client.json")}
        for name, value in (("m.json", m), ("groups.json", ["horizon-worker-registry"]),
                            ("created.json", [record(m["client_group"], run_id=RUN_ID)]), ("client.json", descriptor(m))):
            with open(paths[name], "w", encoding="utf-8") as handle:
                json.dump(value, handle)
        answers = healthy_answers(m)
        original = cli.Az
        cli.Az = lambda subscription, dry_run=False: FakeAz(answers, dry_run=dry_run)
        self.addCleanup(setattr, cli, "Az", original)
        args = ["--manifest", paths["m.json"], "bind-worker", "--group", B_GROUP, "--groups-before", paths["groups.json"],
                "--created", paths["created.json"], "--client", paths["client.json"]]
        self.assertEqual(cli.main(args), 0)
        with open(paths["m.json"], encoding="utf-8") as handle:
            bound = json.load(handle)
        self.assertEqual(bound["worker_group"], B_GROUP)
        self.assertEqual({k: v for k, v in bound.items() if k != "worker_group"},
                         {k: v for k, v in m.items() if k != "worker_group"}, "only the worker group changes")
        with open(paths["created.json"], encoding="utf-8") as handle:
            self.assertEqual([r["name"] for r in json.load(handle)], [m["client_group"], B_GROUP])
        # Bound now: a second binding is refused before any call, and the bound manifest
        # is accepted by the commands that act on B.
        self.assertEqual(cli.main(args), 1)
        self.assertEqual(client_off.validate_manifest(bound, now=client_off.utc_now(), renting=False, phase="off"), [])
        self.assertFalse(os.path.exists(paths["m.json"] + ".tmp"))


class BindCommandPathTests(unittest.TestCase):
    def test_an_output_that_aliases_an_input_is_refused_before_anything_is_read(self):
        m = unbound()
        directory = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, directory, ignore_errors=True)
        paths = {name: os.path.join(directory, name) for name in ("m.json", "groups.json", "created.json", "client.json")}
        for name, value in (("m.json", m), ("groups.json", []),
                            ("created.json", [record(m["client_group"], run_id=RUN_ID)]), ("client.json", descriptor(m))):
            with open(paths[name], "w", encoding="utf-8") as handle:
                json.dump(value, handle)
        original = cli.Az
        cli.Az = lambda subscription, dry_run=False: FakeAz(healthy_answers(m), dry_run=dry_run)
        self.addCleanup(setattr, cli, "Az", original)
        for out in (paths["created.json"], paths["created.json"] + ".tmp", paths["groups.json"], paths["client.json"]):
            with self.subTest(out=os.path.basename(out)):
                code = cli.main(["--manifest", paths["m.json"], "bind-worker", "--group", B_GROUP,
                                 "--groups-before", paths["groups.json"], "--created", paths["created.json"],
                                 "--client", paths["client.json"], "--out", out])
                self.assertEqual(code, 2)
        # The journal and every input survived untouched.
        with open(paths["created.json"], encoding="utf-8") as handle:
            self.assertEqual([r["name"] for r in json.load(handle)], [m["client_group"]])
        with open(paths["m.json"], encoding="utf-8") as handle:
            self.assertEqual(json.load(handle)["worker_group"], client_off.UNBOUND)


class UnboundCleanupTests(unittest.TestCase):
    def test_crash_before_bind_leaves_the_journaled_worker_deletable(self):
        m = unbound()
        created = [record(m["client_group"], lane="azure-client-off", run_id=RUN_ID), B_JOURNALED]
        targets = client_off.cleanup_targets(m, before=["horizon-worker-registry"], created=created)
        self.assertEqual([r["name"] for r in targets["delete"]], [m["client_group"], B_GROUP])
        self.assertEqual(targets["refused"], [])
        self.assertTrue(client_off.run_bound(B_JOURNALED, m))

    def test_unbound_mode_never_derives_a_name_and_still_refuses_strangers(self):
        m = unbound()
        client = record(m["client_group"], run_id=RUN_ID)
        only_client = client_off.cleanup_targets(m, before=[], created=[client])
        self.assertEqual([r["name"] for r in only_client["delete"]], [m["client_group"]])
        self.assertEqual(only_client["refused"], [], "an unbound manifest names no worker to refuse")
        pre_existing = client_off.cleanup_targets(m, before=[B_GROUP], created=[client, B_JOURNALED])
        self.assertEqual([r["name"] for r in pre_existing["delete"]], [m["client_group"]])
        for stranger in (record(B_GROUP), record(B_GROUP, **{"horizon-workflow-id": WORKFLOW_ID}),
                         record(B_GROUP, **{"horizon-workflow-id": JOB_ID, "horizon-job-id": WORKFLOW_ID}),
                         record("horizon-client-" + "e" * 32, run_id="e" * 32)):
            with self.subTest(stranger=stranger["name"]):
                targets = client_off.cleanup_targets(m, before=[], created=[client, stranger])
                self.assertEqual([r["name"] for r in targets["delete"]], [m["client_group"]])
                self.assertFalse(client_off.run_bound(stranger, m))
        self.assertTrue(client_off.cleanup_targets(m, before=[], created="no").get("malformed"))


class ObserverRemovalBoundTests(unittest.TestCase):
    def test_removal_binds_itself_to_a_wall_clock_before_anything_else(self):
        az = FakeAz({})
        result = client_off.phase_remove_observer(az, manifest(), {"vm_name": "worker"}, "/nonexistent.pub", "/tmp")
        self.assertFalse(result["passed"])
        self.assertIsNotNone(az.deadline, "the phase bound is armed before the first check")
        self.assertLessEqual(az.deadline - time.monotonic(), client_off.REMOVE_BOUND_SECONDS)


if __name__ == "__main__":
    unittest.main()
