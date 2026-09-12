"""Integration coverage of provision-client.sh's paid-resource paths against a fake `az`:
lost create answers, delayed visibility, mismatched identity and a failing journal
write. No Azure, no network; the startup bound is shortened through the environment."""
from __future__ import annotations

import datetime as dt
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest

from harness_fixtures import HARNESS, RUN_ID, manifest

SUB = "0f0e0d0c-0b0a-4908-8706-050403020100"
GROUP = f"horizon-client-{RUN_ID}"
GROUP_ID = f"/subscriptions/{SUB}/resourceGroups/{GROUP}"

FAKE_AZ = r'''#!/usr/bin/env bash
# A scripted control plane: every call is logged, answers come from the scenario file.
set -u
dir=${FAKE_AZ_DIR:?}
printf '%s\n' "$*" >>"$dir/calls.log"
scenario=$(cat "$dir/scenario")
count() { local f="$dir/count.$1"; local n=0; [ -f "$f" ] && n=$(cat "$f"); n=$((n + 1)); echo "$n" >"$f"; echo "$n"; }
tags='{"issue":"475","lane":"azure-client-off","purpose":"horizon-azure-vm-spike","deadline":"'"$(cat "$dir/deadline")"'","client_sha":"'"$(cat "$dir/sha")"'","client_binary_sha256":"'"$(cat "$dir/digest")"'","run_id":"'"$(cat "$dir/run_id")"'"}'
case "$*" in
  "group exists"*) echo false ;;
  "vm list-skus"*) case "$scenario" in arm-sku) printf '0\tArm64\n' ;; *) printf '0\tx64\n' ;; esac ;;
  "group create"*) case "$scenario" in lost-create|stalled-read) exit 124 ;; esac; echo '{}' ;;
  "group show"*)
    n=$(count group_show)
    case "$scenario" in
      lost-create) [ "$n" -le 2 ] && exit 1 ;;
      stalled-read) sleep 600 ;;   # ARM accepted the create; every read hangs past the bound
      mismatched) tags='{"issue":"475","lane":"azure-client-off","run_id":"'"$(printf f%.0s {1..32})"'"}' ;;
    esac
    echo '{"id":"'"$(cat "$dir/group_id")"'","name":"'"$(cat "$dir/group")"'","location":"northeurope","tags":'"$tags"'}' ;;
  "vm create"*)
    case "$scenario" in
      success|mismatched-vm|cased-vm-id|foreign-vm-id|lost-create) echo '{}' ;;
      lost-vm-create) exit 124 ;;   # ARM accepted the create; the CLI lost the answer
      *) exit 1 ;;
    esac ;;
  "vm show"*"--query publicIps"*)
    case "$scenario" in success|mismatched-vm|lost-vm-create|cased-vm-id|foreign-vm-id) echo 52.174.10.9 ;; *) exit 1 ;; esac ;;
  "vm show"*)
    n=$(count vm_show)
    case "$scenario" in
      lost-vm-create) [ "$n" -le 2 ] && exit 1 ;;&
      success|lost-vm-create) echo '{"id":"'"$(cat "$dir/group_id")"'/providers/Microsoft.Compute/virtualMachines/client","vmId":"9b8a7c6d-1e2f-4a3b-9c8d-7e6f5a4b3c2d","tags":'"$tags"'}' ;;
      cased-vm-id) echo '{"id":"'"$(cat "$dir/group_id" | tr '[:lower:]' '[:upper:]')"'/PROVIDERS/MICROSOFT.COMPUTE/VIRTUALMACHINES/CLIENT","vmId":"9b8a7c6d-1e2f-4a3b-9c8d-7e6f5a4b3c2d","tags":'"$tags"'}' ;;
      foreign-vm-id) echo '{"id":"/subscriptions/00000000-0000-4000-8000-000000000000/resourceGroups/other/providers/Microsoft.Compute/virtualMachines/client","vmId":"9b8a7c6d-1e2f-4a3b-9c8d-7e6f5a4b3c2d","tags":'"$tags"'}' ;;
      mismatched-vm|lost-create) echo '{"id":"'"$(cat "$dir/group_id")"'/providers/Microsoft.Compute/virtualMachines/client","vmId":"9b8a7c6d-1e2f-4a3b-9c8d-7e6f5a4b3c2d","tags":{"issue":"475"}}' ;;
      *) exit 1 ;;
    esac ;;
  "network nsg rule create"*) echo '{}' ;;
  "vm run-command invoke"*) printf 'Enable succeeded: \n[stdout]\n%s\n[stderr]\n' "$(cat "$dir/host_key")" ;;
  *) echo "fake az: unscripted call: $*" >&2; exit 1 ;;
esac
'''

FAKE_SSH = r'''#!/usr/bin/env bash
# Stands in for the pinned sessions once A is reachable: records the pin file it was
# given, answers the digest check with the expected digest, and swallows the gate script.
dir=${FAKE_AZ_DIR:?}
printf 'ssh %s\n' "$*" >>"$dir/calls.log"
for arg in "$@"; do case "$arg" in UserKnownHostsFile=*) cp "${arg#UserKnownHostsFile=}" "$dir/pin.used" ;; esac; done
case "$*" in
  *sha256sum*) echo "$(cat "$dir/digest")  /home/horizon/horizon-client/$(cat "$dir/sha")/horizon" ;;
  *"bash -s"*) cat >/dev/null ;;
esac
exit 0
'''

FAKE_SCP = r'''#!/usr/bin/env bash
printf 'scp %s\n' "$*" >>"${FAKE_AZ_DIR:?}/calls.log"
exit 0
'''

FAKE_JQ = r'''#!/usr/bin/env bash
# Passes through to the real jq except for the creation-journal append, which fails.
case "$*" in
  *"--argjson shown"*) exit 1 ;;
esac
exec "$REAL_JQ" "$@"
'''


class ProvisionScriptTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.directory, ignore_errors=True)
        self.bin = pathlib.Path(self.directory) / "bin"
        self.bin.mkdir()
        self.fake_dir = pathlib.Path(self.directory) / "fake"
        self.fake_dir.mkdir()
        (self.bin / "az").write_text(FAKE_AZ, encoding="utf-8")
        (self.bin / "az").chmod(0o755)
        host_key = "ssh-ed25519 " + "A" * 68
        for name, value in (("group", GROUP), ("group_id", GROUP_ID), ("run_id", RUN_ID), ("host_key", host_key)):
            (self.fake_dir / name).write_text(value, encoding="utf-8")
        self.host_key = host_key
        key = pathlib.Path(self.directory) / "key"
        subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key)], check=True)
        self.key = str(key)
        binary = pathlib.Path(self.directory) / "horizon"
        binary.write_bytes(b"\x7fELF" + b"\x02\x01\x01" + b"\x00" * 13 + b"\x02\x00\x3e\x00" + b"\x00" * 100)
        import hashlib
        digest = hashlib.sha256(binary.read_bytes()).hexdigest()
        self.binary = str(binary)
        deadline = (dt.datetime.now(dt.timezone.utc) + dt.timedelta(hours=4)).replace(microsecond=0).isoformat()
        self.manifest = manifest(client_binary_sha256=digest, cleanup_deadline_utc=deadline)
        self.manifest["location"] = "northeurope"
        (pathlib.Path(self.directory) / "m.json").write_text(json.dumps(self.manifest), encoding="utf-8")
        (pathlib.Path(self.directory) / "record.json").write_text(
            json.dumps({"client_sha": self.manifest["client_sha"], "client_binary_sha256": digest}), encoding="utf-8")
        for name, value in (("deadline", deadline), ("sha", self.manifest["client_sha"]), ("digest", digest)):
            (self.fake_dir / name).write_text(value, encoding="utf-8")

    def run_script(self, scenario, seconds=12, fake_jq=False, fake_ssh=False, cidr="52.174.10.0/24", window=None):
        (self.fake_dir / "scenario").write_text(scenario, encoding="utf-8")
        # Each run starts with a fresh call log and fresh visibility counters.
        for stale in self.fake_dir.glob("count.*"):
            stale.unlink()
        for stale in ("calls.log", "pin.used"):
            (self.fake_dir / stale).unlink(missing_ok=True)
        if fake_ssh:
            for name, body in (("ssh", FAKE_SSH), ("scp", FAKE_SCP)):
                (self.bin / name).write_text(body, encoding="utf-8")
                (self.bin / name).chmod(0o755)
        env = dict(os.environ, PATH=f"{self.bin}:{os.environ['PATH']}", FAKE_AZ_DIR=str(self.fake_dir),
                   HORIZON_CLIENT_OFF_STARTUP_SECONDS=str(seconds), REAL_JQ=shutil.which("jq") or "jq")
        if window is not None:
            env["HORIZON_CLIENT_OFF_RECONCILE_SECONDS"] = str(window)
        if fake_jq:
            (self.bin / "jq").write_text(FAKE_JQ, encoding="utf-8")
            (self.bin / "jq").chmod(0o755)
        workdir = pathlib.Path(tempfile.mkdtemp(prefix=f"run-{scenario}-", dir=self.directory))
        completed = subprocess.run(["bash", str(HARNESS / "provision-client.sh"), "--manifest", f"{self.directory}/m.json",
                                    "--ssh-private-key", self.key, "--horizon-binary", self.binary,
                                    "--build-record", f"{self.directory}/record.json", "--ssh-source-cidr", cidr,
                                    "--out", str(workdir / "client.json")],
                                   cwd=workdir, env=env, capture_output=True, text=True, timeout=int(seconds) + 90 if str(seconds).isdigit() else 60,
                                   check=False)
        return completed, workdir

    def calls(self):
        log = self.fake_dir / "calls.log"
        return log.read_text(encoding="utf-8").splitlines() if log.exists() else []

    def test_a_lost_create_answer_is_reconciled_and_journaled_before_anything_else(self):
        # The reconcile loop keeps the kill grace out of the bound and naps five seconds
        # between reads, so two lost reads need well over ten seconds of bound.
        # The scenario ends at the VM step with a foreign VM, so the suite stays short;
        # the assertions are about the group create and its journaling.
        completed, workdir = self.run_script("lost-create", seconds=200)
        self.assertNotEqual(completed.returncode, 0)
        journal = json.loads((workdir / "created-groups.json").read_text(encoding="utf-8"))
        self.assertEqual([r["name"] for r in journal], [GROUP], "the group ARM accepted is journaled even though the create timed out")
        self.assertEqual(journal[0]["id"], GROUP_ID)
        self.assertEqual(journal[0]["tags"]["run_id"], RUN_ID)
        shows = [c for c in self.calls() if c.startswith("group show")]
        self.assertGreaterEqual(len(shows), 3, "visibility was polled until the group appeared")
        self.assertIn("refusing to adopt it", completed.stderr)
        # The descriptor path was reserved up front and never written: no A to hand on.
        self.assertEqual((workdir / "client.json").stat().st_size, 0)

    def test_an_accepted_group_is_journaled_even_when_every_read_after_the_create_stalls(self):
        # The create is sent (the window is there), then every read hangs until the bound
        # is gone: the pending record written before the create is what cleanup gets.
        completed, workdir = self.run_script("stalled-read", seconds=70, window=20)
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("pending record stays", completed.stderr)
        journal = json.loads((workdir / "created-groups.json").read_text(encoding="utf-8"))
        self.assertEqual([(r["name"], r["id"]) for r in journal], [(GROUP, GROUP_ID)])
        self.assertEqual(journal[0]["tags"]["run_id"], RUN_ID)
        from harness_fixtures import client_off
        targets = client_off.cleanup_targets(self.manifest, [], journal)
        self.assertEqual([r["name"] for r in targets["delete"]], [GROUP], "cleanup accepts the pending record and re-attests it")

    def test_a_group_with_other_tags_is_never_adopted(self):
        completed, workdir = self.run_script("mismatched", seconds=200)
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("refusing to adopt it", completed.stderr)
        # The pending record stays: cleanup re-attests it against ARM, where the foreign
        # tags make it refuse the delete, so nothing of the other party's is touched.
        journal = json.loads((workdir / "created-groups.json").read_text(encoding="utf-8"))
        self.assertEqual([r["name"] for r in journal], [GROUP])
        self.assertFalse(any(c.startswith("vm create") for c in self.calls()), "no VM is created in a foreign group")

    def test_a_failing_journal_write_stops_the_run_and_names_the_group(self):
        completed, workdir = self.run_script("plain", seconds=200, fake_jq=True)
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("creation journal could not be written", completed.stderr)
        self.assertIn(GROUP_ID, completed.stderr, "the operator is told exactly which group to delete by hand")
        self.assertFalse(any(c.startswith("vm create") for c in self.calls()), "no paid VM without a creation record")

    def test_the_whole_path_ends_in_a_descriptor_that_names_the_exact_client(self):
        completed, workdir = self.run_script("success", seconds=200, fake_ssh=True)
        self.assertEqual(completed.returncode, 0, completed.stderr[-1500:])
        descriptor = json.loads((workdir / "client.json").read_text(encoding="utf-8"))
        self.assertEqual((descriptor["client_group"], descriptor["client_group_id"], descriptor["run_id"], descriptor["client_host"]),
                         (GROUP, GROUP_ID, RUN_ID, "52.174.10.9"))
        self.assertEqual(descriptor["client_vm_id"], f"{GROUP_ID}/providers/Microsoft.Compute/virtualMachines/client")
        self.assertEqual(descriptor["client_instance_id"], "9b8a7c6d-1e2f-4a3b-9c8d-7e6f5a4b3c2d")
        self.assertEqual(descriptor["client_sha"], self.manifest["client_sha"])
        journal = json.loads((workdir / "created-groups.json").read_text(encoding="utf-8"))
        self.assertEqual([r["name"] for r in journal], [GROUP], "the pending record was replaced, not duplicated")
        self.assertEqual(journal[0]["id"], GROUP_ID)
        # The pin the sessions used is the control-plane key for the bare address.
        self.assertEqual((self.fake_dir / "pin.used").read_text(encoding="utf-8"), f"52.174.10.9 {self.host_key}\n")
        calls = self.calls()
        run_command = next(i for i, c in enumerate(calls) if c.startswith("vm run-command invoke"))
        self.assertTrue(any(c.startswith("vm show") and "--query publicIps" not in c for c in calls[run_command + 1:]),
                        "the VM is attested again after the host key was read")
        self.assertTrue(any(c.startswith("scp ") for c in calls) and any("sha256sum" in c for c in calls))
        for name in ("client.json.tmp", "created-groups.json.tmp"):
            self.assertFalse((workdir / name).exists(), f"{name} is not left behind")

    def test_a_lost_vm_create_answer_is_reconciled_and_the_create_is_never_repeated(self):
        completed, workdir = self.run_script("lost-vm-create", seconds=240, fake_ssh=True)
        self.assertEqual(completed.returncode, 0, completed.stderr[-1500:])
        calls = self.calls()
        self.assertEqual(sum(1 for c in calls if c.startswith("vm create")), 1, "an accepted create is never sent twice")
        reads = [i for i, c in enumerate(calls) if c.startswith("vm show") and "--query publicIps" not in c]
        create = next(i for i, c in enumerate(calls) if c.startswith("vm create"))
        self.assertGreaterEqual(len([i for i in reads if i > create]), 3, "the VM was polled until it appeared")
        descriptor = json.loads((workdir / "client.json").read_text(encoding="utf-8"))
        self.assertEqual(descriptor["client_instance_id"], "9b8a7c6d-1e2f-4a3b-9c8d-7e6f5a4b3c2d")

    def test_vm_and_group_ids_are_the_manifests_paths_whatever_their_casing(self):
        completed, workdir = self.run_script("cased-vm-id", seconds=200, fake_ssh=True)
        self.assertEqual(completed.returncode, 0, completed.stderr[-800:])
        descriptor = json.loads((workdir / "client.json").read_text(encoding="utf-8"))
        self.assertEqual(descriptor["client_vm_id"].casefold(), f"{GROUP_ID}/providers/Microsoft.Compute/virtualMachines/client".casefold())
        completed, workdir = self.run_script("foreign-vm-id", seconds=200, fake_ssh=True)
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("not the manifest's VM under the manifest subscription", completed.stderr)
        self.assertEqual((workdir / "client.json").stat().st_size, 0, "no descriptor anchors a foreign VM")
        self.assertEqual([r["name"] for r in json.loads((workdir / "created-groups.json").read_text(encoding="utf-8"))], [GROUP])
        self.assertFalse(any(c.startswith("vm run-command") or c.startswith("ssh ") for c in self.calls()))

    def test_a_vm_with_other_tags_is_never_adopted_and_the_group_stays_journaled(self):
        completed, workdir = self.run_script("mismatched-vm", seconds=200, fake_ssh=True)
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("its tags are not exactly this run's; refusing to adopt it", completed.stderr)
        journal = json.loads((workdir / "created-groups.json").read_text(encoding="utf-8"))
        self.assertEqual([r["name"] for r in journal], [GROUP], "the paid group stays journaled for cleanup")
        self.assertFalse(any(c.startswith("vm run-command") or c.startswith("ssh ") for c in self.calls()),
                         "nothing is read from or pinned on a VM that is not this run's")
        self.assertEqual((workdir / "client.json").stat().st_size, 0)

    def test_the_startup_override_can_shorten_but_never_lengthen_the_bound(self):
        for value in ("3600", "0", "-5", "abc", "1801"):
            with self.subTest(value=value):
                completed, workdir = self.run_script("plain", seconds=value)
                self.assertNotEqual(completed.returncode, 0)
                self.assertIn("between 1 and 1800", completed.stderr)
                self.assertEqual(self.calls(), [], "refused before any control-plane call")

    def test_the_source_range_must_be_a_public_unicast_range(self):
        for cidr in ("224.0.0.0/24", "10.0.0.0/24", "127.0.0.0/24", "52.174.0.0/16", "52.174.10.1/24"):
            with self.subTest(cidr=cidr):
                completed, workdir = self.run_script("plain", cidr=cidr)
                self.assertNotEqual(completed.returncode, 0)
                self.assertIn("globally routable unicast IPv4 range", completed.stderr)
                self.assertEqual(self.calls(), [], "refused before any control-plane call")

    def test_a_create_is_sent_only_with_a_reconciliation_window_behind_it(self):
        # With too little of the bound left to create and then read the group back, the
        # create is not sent at all: nothing can end up accepted and untracked.
        completed, workdir = self.run_script("lost-create", seconds=100)
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("not enough of the startup bound left to create", completed.stderr)
        self.assertFalse(any(c.startswith("group create") for c in self.calls()))
        self.assertEqual(json.loads((workdir / "created-groups.json").read_text(encoding="utf-8")), [])

    def test_the_reconciliation_window_override_can_shorten_but_never_lengthen(self):
        for value in ("121", "0", "x"):
            with self.subTest(value=value):
                completed, workdir = self.run_script("plain", seconds=200, window=value)
                self.assertNotEqual(completed.returncode, 0)
                self.assertIn("between 1 and 120", completed.stderr)
                self.assertEqual(self.calls(), [], "refused before any control-plane call")

    def test_an_arm64_sku_is_refused_before_any_create(self):
        completed, workdir = self.run_script("arm-sku")
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("not an unrestricted x86-64 size", completed.stderr)
        self.assertFalse(any(c.startswith("group create") for c in self.calls()), "nothing was created")
        self.assertEqual(json.loads((workdir / "created-groups.json").read_text(encoding="utf-8")), [])

    def test_preflight_refuses_a_stale_journal_before_any_cloud_call(self):
        workdir = pathlib.Path(self.directory) / "run-stale"
        workdir.mkdir()
        (workdir / "created-groups.json").write_text("[]", encoding="utf-8")
        (self.fake_dir / "scenario").write_text("plain", encoding="utf-8")
        env = dict(os.environ, PATH=f"{self.bin}:{os.environ['PATH']}", FAKE_AZ_DIR=str(self.fake_dir),
                   HORIZON_CLIENT_OFF_STARTUP_SECONDS="12")
        completed = subprocess.run(["bash", str(HARNESS / "provision-client.sh"), "--manifest", f"{self.directory}/m.json",
                                    "--ssh-private-key", self.key, "--horizon-binary", self.binary,
                                    "--build-record", f"{self.directory}/record.json", "--ssh-source-cidr", "52.174.10.0/24",
                                    "--out", str(workdir / "client.json")],
                                   cwd=workdir, env=env, capture_output=True, text=True, timeout=60, check=False)
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("already exists", completed.stderr)
        self.assertEqual(self.calls(), [], "nothing was asked of the control plane")


if __name__ == "__main__":
    unittest.main()
