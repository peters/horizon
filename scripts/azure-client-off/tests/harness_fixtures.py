"""Shared fixtures for the client-off harness tests: a flat view over the harness modules
and the manifest, sample and record builders. No Azure, no network."""
from __future__ import annotations

import datetime as dt
import importlib.util
import pathlib
import types

HARNESS = pathlib.Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("client_off", HARNESS / "client_off.py")
cli = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(cli)  # puts the harness directory on sys.path for the package below
import clientoff  # noqa: E402

# One flat view over the harness for the assertions: every public name of every module
# the harness ships, the CLI's `main` included.
client_off = types.SimpleNamespace()
for module in (*clientoff.MODULES, cli):
    for name in dir(module):
        if not name.startswith("_"):
            setattr(client_off, name, getattr(module, name))

NOW = dt.datetime(2026, 9, 12, 12, 0, tzinfo=dt.timezone.utc)
IMAGE = "x.azurecr.io/horizon-remote-worker@sha256:" + "a" * 64
B_INSTANCE = "3f2c9a1e-5d4b-4c6a-8e7f-0a1b2c3d4e5f"
A_INSTANCE = "9b8a7c6d-1e2f-4a3b-9c8d-7e6f5a4b3c2d"
RUN_ID = "0123456789abcdef0123456789abcdef"
WORKFLOW_ID = "4c5d6e7f-8a9b-4c0d-8e1f-2a3b4c5d6e7f"
JOB_ID = "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d"
ADAPTER_TAGS = {"horizon-workflow-id": WORKFLOW_ID, "horizon-job-id": JOB_ID}


def manifest(**overrides):
    base = {
        "subscription_id": "0f0e0d0c-0b0a-4908-8706-050403020100",
        "location": "northeurope",
        "client_group": "horizon-client-475-a",
        "client_vm_size": "Standard_B2s",
        "client_sha": "77d48a81" + "0" * 32,
        "client_binary_sha256": "b" * 64,
        "worker_group": f"horizon-ws-{WORKFLOW_ID}-{JOB_ID}",
        "worker_image": IMAGE,
        "hourly_cost_micros": 120_000,
        "budget_micros": 2_000_000,
        "cleanup_deadline_utc": (NOW + dt.timedelta(hours=3)).isoformat(),
        "off_minutes": 12,
        "lease_seconds": 600,
    }
    base.update(overrides)
    return base


def samples(count, *, start=NOW, step=15, a_power="PowerState/deallocated", progress=None, checkpoint=None,
            b_power="PowerState/running", identity=("/g/b", "/g/b/vm"), host="52.174.10.5", image=IMAGE, slot=15,
            instance=B_INSTANCE):
    rows = []
    for index in range(count):
        rows.append({
            "scheduled_at": (start + dt.timedelta(seconds=slot * index)).isoformat(),
            "at": (start + dt.timedelta(seconds=step * index)).isoformat(),
            "a_power": a_power,
            "b_group_id": identity[0],
            "b_vm_id": identity[1],
            "b_instance_id": instance,
            "b_host": host,
            "b_image_ref": client_off.image_ref_digest(image),
            "b_power": b_power,
            "progress": (progress or (lambda i: i))(index),
            "checkpoint": checkpoint(index) if checkpoint else None,
        })
    return rows


def record(name, **tags):
    return {"name": name, "id": f"/subscriptions/s/resourceGroups/{name}", "tags": tags}
