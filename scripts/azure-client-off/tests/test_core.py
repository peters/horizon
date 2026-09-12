"""Deterministic coverage of the harness core: manifest gates, the pure verdict, cleanup
authorization and the bounded `az` client. No Azure, no network."""
from __future__ import annotations

import datetime as dt
import unittest
import unittest.mock as mock

from harness_fixtures import (A_GROUP_ID, A_INSTANCE, A_VM_ID, ADAPTER_TAGS, B_GROUP_ID, B_INSTANCE, B_VM_ID, IMAGE, JOB_ID, NOW,
                              RUN_ID, WORKFLOW_ID, client_off, client_tags, manifest, record, samples)

class ManifestTests(unittest.TestCase):
    def test_complete_manifest_is_runnable(self):
        self.assertEqual(client_off.validate_manifest(manifest(), NOW), [])

    def test_every_gate_is_named(self):
        cases = {
            "subscription_id": ("Finter As", "exact UUID"),
            "client_group": ("bad/group", "resource group"),
            "client_sha": ("77d48a81", "full commit SHA"),
            "client_vm_size": ("", "VM size"),
            "client_binary_sha256": ("deadbeef", "SHA-256"),
            "worker_image": ("x.azurecr.io/horizon-remote-worker:latest", "digest"),
            "budget_micros": (0, "positive"),
            "cleanup_deadline_utc": ((NOW - dt.timedelta(minutes=1)).isoformat(), "future"),
            "off_minutes": (5, "at least 10"),
        }
        for field, (value, expected) in cases.items():
            with self.subTest(field=field):
                problems = client_off.validate_manifest(manifest(**{field: value}), NOW)
                self.assertTrue(any(expected in problem for problem in problems), problems)
        # The deadline must outlast the bounded run, not merely lie in the future.
        soon = (NOW + dt.timedelta(minutes=20)).isoformat()
        self.assertTrue(any("at least" in p for p in client_off.validate_manifest(manifest(cleanup_deadline_utc=soon), NOW)))
        self.assertTrue(client_off.validate_manifest(manifest(cleanup_deadline_utc=soon), NOW, phase="off"))
        self.assertEqual(client_off.validate_manifest(manifest(cleanup_deadline_utc=soon), NOW, phase="return"), [])
        self.assertEqual(client_off.validate_manifest(manifest(cleanup_deadline_utc=soon), NOW, renting=False), [])
        enough = (NOW + dt.timedelta(minutes=client_off.required_minutes(manifest(), "validate"))).isoformat()
        self.assertEqual(client_off.validate_manifest(manifest(cleanup_deadline_utc=enough), NOW), [])
        self.assertEqual(client_off.validate_manifest(manifest(client_vm_size="Standard_E4-2s_v5"), NOW), [])
        for region in ("Northern Europe", "northeurope\n", "", "NorthEurope", "north europe"):
            with self.subTest(region=region):
                self.assertTrue(any("region" in p for p in client_off.validate_manifest(manifest(location=region), NOW)))
        self.assertEqual(client_off.validate_manifest(manifest(location="westus2"), NOW), [])
        far = (NOW + dt.timedelta(hours=30)).isoformat()
        self.assertTrue(any("24 hours" in p for p in client_off.validate_manifest(manifest(cleanup_deadline_utc=far), NOW, renting=False)),
                        "the upper bound holds for verdict and cleanup too")
        # `$` alone would accept a trailing newline; the manifest is matched exactly.
        for field in ("worker_image", "subscription_id", "client_group", "client_sha", "client_vm_size"):
            with self.subTest(field=field):
                self.assertTrue(client_off.validate_manifest(manifest(**{field: manifest()[field] + "\n"}), NOW))

    def test_interval_must_cross_the_lease_and_groups_must_differ(self):
        problems = client_off.validate_manifest(manifest(off_minutes=10, lease_seconds=600), NOW)
        self.assertTrue(any("exceed lease_seconds" in problem for problem in problems), problems)
        problems = client_off.validate_manifest(manifest(worker_group=manifest()["client_group"]), NOW)
        self.assertTrue(any("different exact groups" in problem for problem in problems), problems)
        # Every name the harness may create or delete is unique to one run.
        for group in ("horizon-client-475-a", f"horizon-client-{'f' * 32}", f"HORIZON-CLIENT-{RUN_ID}"):
            with self.subTest(client_group=group):
                self.assertTrue(any("no other run can reuse" in p for p in client_off.validate_manifest(manifest(client_group=group), NOW)))
        for group in ("horizon-ws-b", f"horizon-ws-{WORKFLOW_ID}", f"horizon-ws-{WORKFLOW_ID}-{'g' * 36}"):
            with self.subTest(worker_group=group):
                self.assertTrue(any("horizon-ws-<workflow>-<job>" in p for p in client_off.validate_manifest(manifest(worker_group=group), NOW)))
        self.assertTrue(client_off.validate_manifest(manifest(run_id="short"), NOW))
        far = client_off.validate_manifest(manifest(cleanup_deadline_utc=(NOW + dt.timedelta(hours=30)).isoformat()), NOW)
        self.assertTrue(any("24 hours" in problem for problem in far), far)

    def test_missing_fields_are_reported_before_anything_else(self):
        problems = client_off.validate_manifest({"location": "northeurope"}, NOW)
        self.assertTrue(problems and all(problem.startswith("missing ") for problem in problems))
        for not_object in (None, 3, "manifest", [manifest()]):
            with self.subTest(value=not_object):
                self.assertEqual(client_off.validate_manifest(not_object, NOW), ["manifest is not a JSON object"])

    def test_past_deadline_blocks_renting_but_not_verdict_or_cleanup(self):
        past = manifest(cleanup_deadline_utc=(NOW - dt.timedelta(hours=1)).isoformat())
        self.assertTrue(any("future" in problem for problem in client_off.validate_manifest(past, NOW)))
        self.assertEqual(client_off.validate_manifest(past, NOW, renting=False), [])

    def test_naive_deadlines_are_rejected_not_guessed(self):
        problems = client_off.validate_manifest(manifest(cleanup_deadline_utc="2026-09-12T15:00:00"), NOW)
        self.assertTrue(any("ISO-8601" in problem for problem in problems), problems)
        with self.assertRaises(ValueError):
            client_off.parse_utc("2026-09-12T15:00:00")
        self.assertEqual(client_off.parse_utc("2026-09-12T15:00:00Z").tzinfo, dt.timezone.utc)


class VerdictTests(unittest.TestCase):
    def test_full_off_interval_with_advancing_counter_passes_without_claiming_checkpoints(self):
        verdict = client_off.evaluate_samples(samples(49), off_minutes=12, lease_seconds=600)
        self.assertTrue(verdict["passed"], verdict)
        self.assertEqual(verdict["missed_samples"], 0)
        self.assertFalse(verdict["worker_checkpoint_progress"], "counters alone are not checkpoint proof")

    def test_checkpoint_progress_is_judged_separately(self):
        verdict = client_off.evaluate_samples(samples(49, checkpoint=lambda i: i // 10), 12, 600)
        self.assertTrue(verdict["passed"])
        self.assertTrue(verdict["worker_checkpoint_progress"])
        verdict = client_off.evaluate_samples(samples(49, checkpoint=lambda i: 3), 12, 600)
        self.assertFalse(verdict["worker_checkpoint_progress"], "a constant checkpoint sequence is not progress")

    def test_short_interval_or_lease_not_crossed_fails(self):
        short = client_off.evaluate_samples(samples(20), 12, 600)
        self.assertFalse(short["passed"])
        self.assertTrue(any("declared interval" in finding for finding in short["findings"]), short)
        lease = client_off.evaluate_samples(samples(49), 12, 720)
        self.assertTrue(any("did not exceed the lease" in finding for finding in lease["findings"]), lease)

    def test_client_seen_running_or_worker_identity_change_fails(self):
        rows = samples(49)
        rows[7]["a_power"] = "PowerState/running"
        verdict = client_off.evaluate_samples(rows, 12, 600)
        self.assertTrue(any("not deallocated" in finding for finding in verdict["findings"]), verdict)
        rows = samples(49)
        rows[30]["b_vm_id"] = "/g/b/other"
        verdict = client_off.evaluate_samples(rows, 12, 600)
        self.assertTrue(any("identity changed" in finding for finding in verdict["findings"]), verdict)
        for blank in ("", "  "):
            rows = samples(49, identity=(blank, blank), instance=blank)
            verdict = client_off.evaluate_samples(rows, 12, 600)
            self.assertTrue(any("identity changed or was unreadable" in finding for finding in verdict["findings"]),
                            f"stable empty identities are missing identities: {verdict}")
        rows = samples(49)
        rows[30]["b_instance_id"] = A_INSTANCE
        verdict = client_off.evaluate_samples(rows, 12, 600)
        self.assertTrue(any("identity changed" in finding for finding in verdict["findings"]),
                        f"a recreated B under the same IDs is a different instance: {verdict}")
        rows = samples(49)
        rows[3]["b_power"] = "PowerState/deallocated"
        verdict = client_off.evaluate_samples(rows, 12, 600)
        self.assertTrue(any("power states" in finding for finding in verdict["findings"]), verdict)

    def test_stalled_or_replayed_counter_and_misses_are_findings(self):
        stalled = client_off.evaluate_samples(samples(49, progress=lambda i: 5), 12, 600)
        self.assertTrue(any("did not advance" in finding for finding in stalled["findings"]), stalled)
        # A counter that stalls for most of the interval and moves once at the end is a stall.
        late = client_off.evaluate_samples(samples(49, progress=lambda i: 0 if i < 48 else 1), 12, 600)
        self.assertTrue(any("did not advance between 47" in finding for finding in late["findings"]), late)
        moved_host = samples(49)
        moved_host[20]["b_host"] = "52.174.10.6"
        verdict = client_off.evaluate_samples(moved_host, 12, 600)
        self.assertTrue(any("endpoint changed" in finding for finding in verdict["findings"]), verdict)
        replayed = client_off.evaluate_samples(samples(49, progress=lambda i: i if i < 40 else i - 40), 12, 600)
        self.assertTrue(any("backwards" in finding for finding in replayed["findings"]), replayed)
        unreadable = samples(49)
        unreadable[10]["progress"] = None
        verdict = client_off.evaluate_samples(unreadable, 12, 600)
        self.assertTrue(any("unreadable" in finding for finding in verdict["findings"]), verdict)
        gappy = samples(25, step=30, slot=30)
        verdict = client_off.evaluate_samples(gappy, 12, 600)
        self.assertEqual(verdict["missed_samples"], 24, "every other 15 s slot was missed")
        self.assertFalse(verdict["passed"])
        late = samples(49)
        late[7]["at"] = (NOW + dt.timedelta(seconds=15 * 7 + 9)).isoformat()
        verdict = client_off.evaluate_samples(late, 12, 600)
        self.assertEqual((verdict["missed_samples"], verdict["late_samples"]), (0, 1))
        self.assertFalse(verdict["passed"], "a late sample is a cadence finding")
        on_time = samples(49)
        on_time[7]["at"] = (NOW + dt.timedelta(seconds=15 * 7 + 4)).isoformat()
        self.assertEqual(client_off.evaluate_samples(on_time, 12, 600)["late_samples"], 0, "4 s of jitter is tolerated")
        skipped = samples(49)
        del skipped[20]
        verdict = client_off.evaluate_samples(skipped, 12, 600)
        self.assertEqual(verdict["missed_samples"], 1, "a skipped slot is one missed observation")
        unscheduled = samples(49)
        del unscheduled[3]["scheduled_at"]
        verdict = client_off.evaluate_samples(unscheduled, 12, 600)
        self.assertTrue(any("no scheduled instant" in finding for finding in verdict["findings"]), verdict)
        none_scheduled = samples(49)
        for row in none_scheduled:
            del row["scheduled_at"]
        self.assertFalse(client_off.evaluate_samples(none_scheduled, 12, 600)["passed"], "no fallback without a schedule")
        stretched = samples(49, step=20, slot=20)
        verdict = client_off.evaluate_samples(stretched, 12, 600)
        self.assertTrue(any("15-second slots" in finding for finding in verdict["findings"]), verdict)
        seesaw = samples(49)
        for index, row in enumerate(seesaw):
            row["at"] = (NOW + dt.timedelta(seconds=15 * index + (4 if index % 2 else 0))).isoformat()
        seesaw_verdict = client_off.evaluate_samples(seesaw, 12, 600)
        self.assertEqual(seesaw_verdict["late_samples"], 0)
        self.assertFalse(seesaw_verdict["passed"], "4 s alternating jitter opens 19-second gaps: A and B went unobserved")
        self.assertTrue(any("gaps over 15s" in finding for finding in seesaw_verdict["findings"]), seesaw_verdict)
        for index, row in enumerate(seesaw):
            row["at"] = (NOW + dt.timedelta(seconds=15 * index, milliseconds=(900 if index % 2 else 0))).isoformat()
        self.assertTrue(client_off.evaluate_samples(seesaw, 12, 600)["passed"], "sub-second wake-up latency is not a gap")
        for index, row in enumerate(seesaw):
            row["at"] = (NOW + dt.timedelta(seconds=15 * index + index)).isoformat()
        self.assertFalse(client_off.evaluate_samples(seesaw, 12, 600)["passed"], "16-second gaps are a cadence finding")
        early = samples(49)
        early[5]["at"] = (NOW + dt.timedelta(seconds=15 * 5 - 1)).isoformat()
        verdict = client_off.evaluate_samples(early, 12, 600)
        self.assertTrue(any("before its scheduled slot" in finding for finding in verdict["findings"]), verdict)

    def test_non_string_fields_and_case_variants_are_rejected(self):
        self.assertTrue(client_off.validate_manifest(manifest(client_group=123), NOW))
        problems = client_off.validate_manifest(manifest(client_group="Horizon-WS-B", worker_group="horizon-ws-b"), NOW)
        self.assertTrue(any("different exact groups" in problem for problem in problems), problems)
        self.assertTrue(client_off.validate_manifest(manifest(worker_image=["x"]), NOW))
        for bad in ("not-an-image@sha256:" + "a" * 64, "x.azurecr.io/repo:latest", "x.azurecr.io/Repo@sha256:" + "a" * 64):
            with self.subTest(bad=bad):
                self.assertTrue(client_off.validate_manifest(manifest(worker_image=bad), NOW))
        self.assertEqual(client_off.validate_manifest(manifest(worker_image="registry.example.com:5000/team/worker@sha256:" + "a" * 64), NOW), [])

    def test_counters_must_be_integers(self):
        rows = samples(49, progress=lambda i: str(i))
        verdict = client_off.evaluate_samples(rows, 12, 600)
        self.assertTrue(any("not an integer" in finding for finding in verdict["findings"]), verdict)
        rows = samples(49, progress=lambda i: True)
        self.assertFalse(client_off.evaluate_samples(rows, 12, 600)["passed"])
        rows = samples(49, checkpoint=lambda i: str(i))
        self.assertFalse(client_off.evaluate_samples(rows, 12, 600)["worker_checkpoint_progress"])

    def test_booleans_are_not_positive_integers(self):
        for field in ("hourly_cost_micros", "budget_micros", "off_minutes", "lease_seconds"):
            with self.subTest(field=field):
                self.assertTrue(client_off.validate_manifest(manifest(**{field: True}), NOW))

    def test_worker_image_identity_is_checked_when_the_manifest_is_known(self):
        rows = samples(49)
        self.assertTrue(client_off.evaluate_samples(rows, 12, 600, IMAGE)["passed"])
        other = "x.azurecr.io/horizon-remote-worker@sha256:" + "c" * 64
        verdict = client_off.evaluate_samples(rows, 12, 600, other)
        self.assertTrue(any("image" in finding for finding in verdict["findings"]), verdict)
        rows[4]["b_image_ref"] = None
        verdict = client_off.evaluate_samples(rows, 12, 600, IMAGE)
        self.assertTrue(any("image" in finding for finding in verdict["findings"]), verdict)

    def test_too_few_samples_never_pass(self):
        self.assertFalse(client_off.evaluate_samples(samples(1), 12, 600)["passed"])

    def test_incomplete_evidence_is_a_failed_verdict_not_a_crash(self):
        missing = samples(49)
        del missing[5]["at"]
        verdict = client_off.evaluate_samples(missing, 12, 600)
        self.assertFalse(verdict["passed"])
        self.assertTrue(any("timestamp" in finding for finding in verdict["findings"]), verdict)
        naive = samples(49)
        naive[5]["at"] = "2026-09-12T12:01:15"
        self.assertFalse(client_off.evaluate_samples(naive, 12, 600)["passed"])
        backwards = samples(49)
        backwards[10]["at"], backwards[11]["at"] = backwards[11]["at"], backwards[10]["at"]
        verdict = client_off.evaluate_samples(backwards, 12, 600)
        self.assertTrue(any("monotonic" in finding for finding in verdict["findings"]), verdict)


class SampleIdentityTests(unittest.TestCase):
    def test_identity_ids_compare_case_insensitively_within_the_interval(self):
        rows = samples(49)
        rows[10]["b_vm_id"] = rows[10]["b_vm_id"].upper()
        self.assertTrue(client_off.evaluate_samples(rows, 12, 600)["passed"])

    def test_first_sample_must_be_the_baseline_worker(self):
        expected = {"b_group_id": B_GROUP_ID.upper(), "b_vm_id": B_VM_ID, "b_instance_id": B_INSTANCE, "b_host": "52.174.10.5",
                    "a_group_id": A_GROUP_ID, "a_vm_id": A_VM_ID, "a_instance_id": A_INSTANCE}
        rows = samples(49)
        self.assertTrue(client_off.evaluate_samples(rows, 12, 600, IMAGE, expected)["passed"], "IDs compare case-insensitively")
        other = dict(expected, b_vm_id="/g/b/other")
        verdict = client_off.evaluate_samples(rows, 12, 600, IMAGE, other)
        self.assertTrue(any("baseline" in finding for finding in verdict["findings"]), verdict)
        other_client = dict(expected, a_instance_id=B_INSTANCE)
        verdict = client_off.evaluate_samples(rows, 12, 600, IMAGE, other_client)
        self.assertTrue(any("baseline" in finding for finding in verdict["findings"]), "the VM that was off must be A")
        for field in ("a_tags", "a_group_tags"):
            for value in (None, {}, dict(client_tags(), purpose="other"), "tags"):
                rows = samples(49)
                rows[20][field] = value
                verdict = client_off.evaluate_samples(rows, 12, 600, IMAGE, expected, client_tags())
                self.assertTrue(any("this run's tags" in finding for finding in verdict["findings"]), f"{field}={value!r}: {verdict}")
        rows = samples(49)
        del rows[20]["a_tags"]
        self.assertFalse(client_off.evaluate_samples(rows, 12, 600, IMAGE, expected, client_tags())["passed"],
                         "a missing tag map is a miss")
        rows = samples(49)
        rows[20]["a_instance_id"] = B_INSTANCE
        verdict = client_off.evaluate_samples(rows, 12, 600, IMAGE, expected)
        self.assertTrue(any("client A identity changed" in finding for finding in verdict["findings"]), verdict)

    def test_malformed_sample_shapes_fail_closed(self):
        rows = samples(49)
        rows[3]["b_vm_id"] = {}
        verdict = client_off.evaluate_samples(rows, 12, 600)
        self.assertFalse(verdict["passed"])
        self.assertTrue(any("wrong shape" in finding for finding in verdict["findings"]), verdict)
        rows = samples(49)
        rows[3]["b_power"] = []
        self.assertFalse(client_off.evaluate_samples(rows, 12, 600)["passed"])


class OfflineVerdictTests(unittest.TestCase):
    def test_journal_needs_the_baseline_header_and_the_same_image(self):
        m = manifest()
        header = {"baseline": {"b_group_id": B_GROUP_ID, "b_vm_id": B_VM_ID, "b_instance_id": B_INSTANCE, "b_host": "52.174.10.5",
                               "a_group_id": A_GROUP_ID, "a_vm_id": A_VM_ID, "a_instance_id": A_INSTANCE},
                  "worker_image": IMAGE,
                  "observed_image_ref": client_off.image_ref_digest(IMAGE)}
        self.assertTrue(client_off.verdict_from_records([header, *samples(49)], m)["passed"])
        # A header naming anything but an Azure worker in the manifest's group, however
        # consistently the samples repeat it, is no evidence.
        for field, value in (("b_group_id", "/g/b"), ("b_group_id", B_GROUP_ID.replace(m["subscription_id"], "0" * 36)),
                             ("b_vm_id", "/g/b/vm"), ("b_vm_id", B_VM_ID.replace("/worker", "/other")),
                             ("b_instance_id", "stable-but-not-a-vmid"), ("b_host", "10.0.0.5"),
                             ("a_group_id", "/g/client"), ("a_vm_id", A_VM_ID.replace("/client", "/other")),
                             ("a_instance_id", "stable-but-not-a-vmid")):
            with self.subTest(field=field, value=value):
                fake = dict(header, baseline=dict(header["baseline"], **{field: value}))
                rows = samples(49, identity=(fake["baseline"]["b_group_id"], fake["baseline"]["b_vm_id"]),
                               instance=fake["baseline"]["b_instance_id"], host=fake["baseline"]["b_host"])
                for row in rows:
                    row.update({k: fake["baseline"][k] for k in ("a_group_id", "a_vm_id", "a_instance_id")})
                verdict = client_off.verdict_from_records([fake, *rows], m)
                self.assertFalse(verdict["passed"])
                self.assertTrue(any(finding.startswith("baseline:") for finding in verdict["findings"]), verdict)
        for missing in ("a_instance_id", "a_vm_id"):
            with self.subTest(missing=missing):
                partial = dict(header, baseline={k: v for k, v in header["baseline"].items() if k != missing})
                self.assertFalse(client_off.verdict_from_records([partial, *samples(49)], m)["passed"],
                                 "a header that does not identify A never passes")
        # The samples' attestation evidence is re-derived offline: a retagged A fails.
        retagged = samples(49)
        retagged[30]["a_tags"] = dict(client_tags(), run_id="f" * 32)
        verdict = client_off.verdict_from_records([header, *retagged], m)
        self.assertTrue(any("exactly this run's tags" in f for f in verdict["findings"]), verdict)
        retagged = samples(49)
        retagged[30]["a_group_tags"] = {}
        self.assertFalse(client_off.verdict_from_records([header, *retagged], m)["passed"])
        strayed = samples(49)
        strayed[30]["a_vm_id"] = A_VM_ID.replace("/client", "/other")
        verdict = client_off.verdict_from_records([header, *strayed], m)
        self.assertTrue(any("recorded before the stop" in f for f in verdict["findings"]), verdict)
        other_observed = dict(header, observed_image_ref="c" * 64)
        self.assertFalse(client_off.verdict_from_records([other_observed, *samples(49)], m)["passed"], "observed tag must match")
        self.assertFalse(client_off.verdict_from_records(samples(49), m)["passed"], "no header")
        headless = {"baseline": header["baseline"]}
        verdict = client_off.verdict_from_records([headless, *samples(49)], m)
        self.assertFalse(verdict["passed"])
        self.assertTrue(any("does not name the worker image" in f for f in verdict["findings"]), verdict)
        other_image = dict(header, worker_image="x.azurecr.io/horizon-remote-worker@sha256:" + "c" * 64)
        self.assertFalse(client_off.verdict_from_records([other_image, *samples(49)], m)["passed"])
        other_worker = dict(header, baseline=dict(header["baseline"], b_instance_id=A_INSTANCE))
        verdict = client_off.verdict_from_records([other_worker, *samples(49)], m)
        self.assertTrue(any("baseline" in finding for finding in verdict["findings"]), verdict)


class CleanupTests(unittest.TestCase):
    def test_only_groups_created_by_this_run_are_deleted(self):
        m = manifest()
        created = [record(m["client_group"], lane="azure-client-off", run_id=RUN_ID), record(m["worker_group"], **ADAPTER_TAGS)]
        both = client_off.cleanup_targets(m, before=["horizon-worker-registry"], created=created)
        self.assertEqual([r["name"] for r in both["delete"]], [m["client_group"], m["worker_group"]])
        self.assertEqual(both["refused"], [])
        pre_existing = client_off.cleanup_targets(m, before=[m["worker_group"]], created=created)
        self.assertEqual([r["name"] for r in pre_existing["delete"]], [m["client_group"]])
        self.assertEqual(pre_existing["refused"], [m["worker_group"]], "a pre-existing group is never deleted")
        cased = client_off.cleanup_targets(m, before=[m["worker_group"].upper()], created=created)
        self.assertEqual(cased["refused"], [m["worker_group"]], "group names compare case-insensitively")
        unjournaled = client_off.cleanup_targets(m, before=[], created=created[:1])
        self.assertEqual(unjournaled["refused"], [m["worker_group"]], "a stale manifest name is not a creation record")
        # ARM group IDs are name-based and every reproducible tag can be restored, so a
        # record authorizes a delete only when its tags are bound to this run: A's group
        # carries exactly this run's value, B's group carries the identities its own
        # name is derived from. Anything else deletes nothing and is never even read.
        foreign = {"horizon-workflow-id": JOB_ID, "horizon-job-id": WORKFLOW_ID}
        for stranger in (record(m["worker_group"]), record(m["worker_group"], lane="azure-client-off"),
                         record(m["worker_group"], **{"horizon-workflow-id": WORKFLOW_ID}), record(m["worker_group"], **foreign),
                         record(m["worker_group"], run_id=RUN_ID), record(m["worker_group"], run_id="short")):
            with self.subTest(tags=stranger["tags"]):
                refused = client_off.cleanup_targets(m, before=[], created=[created[0], stranger])
                self.assertEqual(refused["refused"], [m["worker_group"]])
                self.assertFalse(client_off.owned_now(None, stranger, m), "never even read, let alone deleted")
        for other_run in ("f" * 32, "", "short"):
            with self.subTest(run_id=other_run):
                other = dict(m, run_id=other_run)
                refused = client_off.cleanup_targets(other, before=[], created=created)
                self.assertEqual(refused["refused"], [m["client_group"]], "A's group belongs to the run that drew its value")
        # The name itself must be this run's: a same-tagged group under any other name
        # is not the one the manifest froze.
        renamed = dict(m, client_group="horizon-client-475-a")
        refused = client_off.cleanup_targets(renamed, before=[], created=[record("horizon-client-475-a", run_id=RUN_ID), created[1]])
        self.assertEqual(refused["refused"], ["horizon-client-475-a"])
        for malformed in ("horizon-client-475-a", {"horizon-client-475-a": True}, [1], ["bad/group"], None,
                          [m["client_group"]], [{"name": m["client_group"]}], [{"name": m["client_group"], "id": "", "tags": {}}],
                          [{"name": m["client_group"], "id": "/g", "tags": {"a": 1}}]):
            with self.subTest(malformed=malformed):
                refused = client_off.cleanup_targets(m, before=[], created=malformed)
                self.assertEqual(refused["delete"], [], "a malformed journal authorizes nothing")
                self.assertTrue(refused.get("malformed"))

    def test_ownership_before_delete_requires_the_same_id_and_identical_tags(self):
        m = manifest()
        journaled = record(m["client_group"], lane="azure-client-off", client_sha="x", run_id=RUN_ID)
        answers = {}

        class FakeAz:
            def run(self, args, mutating=False, timeout=0):
                return answers.get("show")

        owned = lambda rec=journaled: client_off.owned_now(FakeAz(), rec, m)  # noqa: E731
        answers["show"] = {"id": journaled["id"].upper(), "name": journaled["name"], "tags": dict(journaled["tags"])}
        self.assertTrue(owned(), "ARM IDs compare case-insensitively")
        answers["show"] = {"id": "/subscriptions/s/resourceGroups/other", "name": journaled["name"], "tags": dict(journaled["tags"])}
        self.assertFalse(owned(), "a recreated group has a new ID")
        answers["show"] = {"id": journaled["id"], "name": journaled["name"], "tags": {"lane": "azure-client-off"}}
        self.assertFalse(owned(), "a retagged group is not ours")
        answers["show"] = {"id": journaled["id"], "name": journaled["name"], "tags": dict(journaled["tags"], run_id="f" * 32)}
        self.assertFalse(owned(), "a recreation under another run value is not ours")
        answers["show"] = {"id": journaled["id"], "name": journaled["name"], "tags": []}
        self.assertIsNone(owned(), "a malformed tag map is unknown")
        duplicate = client_off.journal_records([journaled, dict(journaled, id="/other")])
        self.assertIsNone(duplicate, "conflicting records for one name make the journal untrustworthy")
        answers["show"] = None
        self.assertIsNone(owned(), "unreadable is unknown, not absence")

    def test_dry_run_cleanup_walks_the_same_authorization_and_journals_the_delete(self):
        m = manifest()
        created = [record(m["client_group"], lane="azure-client-off", run_id=RUN_ID), record(m["worker_group"], **ADAPTER_TAGS)]
        az = client_off.Az("0f0e0d0c-0b0a-4908-8706-050403020100", dry_run=True)
        shown = {rec["name"]: {"id": rec["id"], "name": rec["name"], "tags": dict(rec["tags"])} for rec in created}
        # B was retagged since it was journaled: a dry run must say so, not list it.
        shown[m["worker_group"]]["tags"] = {}

        def fake_run(args, mutating=False, timeout=0):
            az.journal.append({"mutating": mutating, "args": args})
            if mutating:
                return None
            return shown.get(args[args.index("-n") + 1]) if args[:2] == ["group", "show"] else None

        with mock.patch.object(az, "run", side_effect=fake_run):
            result = client_off.phase_cleanup(az, m, [], created)
        self.assertTrue(result["dry_run"])
        self.assertEqual(result["would_delete"], [m["client_group"]])
        self.assertTrue(any("retagged" in finding for finding in result["findings"]), result)
        proposed = [entry["args"] for entry in az.journal if entry["mutating"]]
        self.assertEqual(len(proposed), 1)
        self.assertIn(created[0]["id"], proposed[0][-1], "the intended delete names the journaled ARM ID")


class AzTests(unittest.TestCase):
    def test_power_state_fails_closed_on_malformed_instance_views(self):
        az = client_off.Az("0f0e0d0c-0b0a-4908-8706-050403020100")
        for view in ({"instanceView": None}, {"instanceView": []}, {"instanceView": {"statuses": None}},
                     {"instanceView": {"statuses": [None, "PowerState/running"]}},
                     {"instanceView": {"statuses": [{"code": 7}]}}, {"instanceView": {"statuses": {}}}, [], None,
                     {"instanceView": {"statuses": [{"code": "PowerState/deallocated"}, {"code": "PowerState/running"}]}},
                     {"instanceView": {"statuses": [{"code": "PowerState/running"}, {"level": "Info"}]}}):
            with self.subTest(view=view), mock.patch.object(az, "run", return_value=view):
                self.assertIsNone(az.power_state("g", "client"))
        with mock.patch.object(az, "run", return_value={"instanceView": {"statuses": [{"code": "PowerState/running"}]}}):
            self.assertEqual(az.power_state("g", "client"), "PowerState/running")


    def test_a_missing_or_failing_az_is_an_unreadable_answer(self):
        az = client_off.Az("0f0e0d0c-0b0a-4908-8706-050403020100")
        for error in (FileNotFoundError("az"), PermissionError("az"), OSError("exec")):
            with self.subTest(error=error), mock.patch.object(client_off.subprocess, "run", side_effect=error):
                self.assertIsNone(az.run(["group", "exists", "-n", "g"]))

    def test_dry_run_journals_but_never_issues_a_mutation(self):
        az = client_off.Az("0f0e0d0c-0b0a-4908-8706-050403020100", dry_run=True)
        self.assertIsNone(az.run(["vm", "deallocate", "-g", "g", "-n", "client"], mutating=True))
        self.assertEqual([entry["mutating"] for entry in az.journal], [True])


if __name__ == "__main__":
    unittest.main()
