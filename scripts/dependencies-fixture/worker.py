#!/usr/bin/env python3
"""Synthetic maintenance worker. Parses real YAML and runs explicit fixture checks.

This never contacts GitHub or a cloud provider. AGENTS.md is read as instructions,
not interpreted as an executable program. Only fixture-authored checks are run.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

import yaml


def now():
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def atomic_json(path, value):
    pending = path.with_suffix(".pending")
    pending.write_text(json.dumps(value, indent=2) + "\n")
    pending.replace(path)


REPOS = [
    ("sample-web", "npm", "frontend", 124, "Update frontend dependency group"),
    ("sample-service", "nuget", "service-libraries", 58, "Update service dependency group"),
    ("sample-desktop", "cargo", "desktop-libraries", 93, "Update desktop dependency group"),
    ("sample-api", "pip", "api-libraries", 106, "Update API dependency group"),
    ("sample-cli", "gomod", "cli-libraries", 212, "Update CLI dependency group"),
    ("sample-admin", "npm", "admin-libraries", 76, "Update admin dependency group"),
    ("sample-events", "maven", "event-libraries", 143, "Update event dependency group"),
    ("sample-mobile", "gradle", "mobile-libraries", 88, "Update mobile dependency group"),
    ("sample-docs", "npm", "docs-libraries", 61, "Update docs dependency group"),
    ("sample-jobs", "pip", "job-libraries", 134, "Update job dependency group"),
    ("sample-tools", "cargo", "tool-libraries", 57, "Update tool dependency group"),
    ("sample-assets", "composer", "asset-libraries", 92, "Update asset dependency group"),
    ("sample-data", "nuget", "data-libraries", 165, "Update data dependency group"),
    ("sample-console", "bundler", "console-libraries", 74, "Update console dependency group"),
    ("sample-search", "gomod", "search-libraries", 103, "Update search dependency group"),
    ("sample-schemas", "npm", "schema-libraries", 45, "Update schema dependency group"),
    ("sample-metrics", "pip", "metric-libraries", 62, "Update metric dependency group"),
    ("sample-fixtures", "cargo", "fixture-libraries", 39, "Update fixture dependency group"),
    ("sample-archive", "maven", "archive-libraries", 128, "Update archive dependency group"),
    ("sample-bridge", "nuget", "bridge-libraries", 97, "Update bridge dependency group"),
    ("sample-experiments", "npm", "experiment-libraries", 53, "Update experiment dependency group"),
]
DISABLED = {18, 20}
BLOCKED = {7, 12}
HISTORY_COMPLETE = {4, 10, 14}
IDLE = {15, 16, 17}
EXECUTION_LIMIT = 3
GLOBAL_PROMPT = ("Read trusted AGENTS.md and applicable testing procedures. Preserve Dependabot groups. "
                 "Use GitHub Actions for CI, fix relevant compatibility errors without weakening tests, "
                 "and squash merge only when scoped authorization and all current-head checks pass.")
MAX_POLICY_BYTES = 65536


def validate_policy_request(value):
    if not isinstance(value, dict) or set(value) != {"global_prompt", "repo_prompts"}:
        raise ValueError("Policy must contain only global_prompt and repo_prompts")
    if not isinstance(value["global_prompt"], str) or len(value["global_prompt"]) > 4096 or "\x00" in value["global_prompt"]:
        raise ValueError("Invalid global prompt; maximum 4096 characters")
    prompts = value["repo_prompts"]
    known = {f"example/{repo[0]}" for repo in REPOS}
    if not isinstance(prompts, dict) or not set(prompts).issubset(known):
        raise ValueError("Unknown repository in policy")
    if any(not isinstance(prompt, str) or len(prompt) > 2048 or "\x00" in prompt for prompt in prompts.values()):
        raise ValueError("Invalid repository prompt; maximum 2048 characters")
    if len(json.dumps(value).encode()) > MAX_POLICY_BYTES:
        raise ValueError("Policy exceeds 65536 bytes")
    return value


def load_policy(root):
    path = root / "policy.json"
    if not path.exists():
        policy = {"revision": 1, "global_prompt": GLOBAL_PROMPT,
                  "repo_prompts": {f"example/{name}": f"Follow AGENTS.md; preserve {group}; apply the {ecosystem} testing recipe."
                                   for name, ecosystem, group, _, _ in REPOS}}
        atomic_json(path, policy)
    policy = json.loads(path.read_text())
    if not isinstance(policy, dict) or set(policy) != {"revision", "global_prompt", "repo_prompts"}:
        raise ValueError("Invalid stored policy")
    if type(policy["revision"]) is not int or policy["revision"] < 1:
        raise ValueError("Invalid stored policy revision")
    validate_policy_request({"global_prompt": policy["global_prompt"], "repo_prompts": policy["repo_prompts"]})
    return policy


def configure(root, value):
    validate_policy_request(value)
    current = load_policy(root)
    current["revision"] += 1
    current["global_prompt"] = value["global_prompt"]
    current["repo_prompts"].update(value["repo_prompts"])
    atomic_json(root / "policy.json", current)
    return {"accepted": True, "configRevision": current["revision"], "synthetic": True}


def seed(root):
    for name, ecosystem, group, number, title in REPOS:
        repo = root / "repos" / name
        (repo / ".github").mkdir(parents=True, exist_ok=True)
        config = {"version": 2, "updates": [{"package-ecosystem": ecosystem,
                  "directory": "/", "schedule": {"interval": "weekly"},
                  "groups": {group: {"patterns": ["*"], "update-types": ["minor", "patch"]}},
                  "ignore": [{"dependency-name": "*", "update-types": ["version-update:semver-major"]}]}]}
        if name == "sample-web":
            config["updates"].append({"package-ecosystem": "github-actions", "directory": "/",
                 "schedule": {"interval": "monthly"}, "groups": {"workflow-tools": {"patterns": ["*"]}}})
        (repo / ".github" / "dependabot.yml").write_text(yaml.safe_dump(config, sort_keys=False))
        recipes = {"sample-web": "synthetic build, unit tests, browser recipe",
                   "sample-service": "synthetic API compatibility, fixtures, package visibility",
                   "sample-desktop": "synthetic compilation, unit tests, native smoke recipe"}
        (repo / "AGENTS.md").write_text(
            "# Synthetic repository instructions\n\n"
            "Preserve Dependabot groups, ignore rules and schedules. Read the trusted base instructions.\n"
            "Require current-head CI, applicable repository recipes, review and resolved feedback.\n"
            "Do not weaken tests. Squash merge only with configured authorization; verify post-merge checks.\n\n"
            "## Demo verification\n\n"
            f"Run `python3 checks.py` for {recipes.get(name, 'synthetic configuration and compatibility assertions')}. "
            "These are fixture checks, not production tests.\n"
        )
        (repo / "api_call.py").write_text("def send(request, cancellation_token):\n    return request == 'ok' and cancellation_token is None\n\ndef exercise():\n    return send('ok')\n" if name == "sample-service" else "def exercise():\n    return True\n")
        (repo / "checks.py").write_text(
            "from pathlib import Path\nimport yaml\nfrom api_call import exercise\n"
            "config = yaml.safe_load(Path('.github/dependabot.yml').read_text())\n"
            "assert config['version'] == 2\nassert all(u.get('groups') for u in config['updates'])\n"
            "assert 'Do not weaken tests.' in Path('AGENTS.md').read_text()\n"
            "assert exercise()\nprint('PASS: explicit synthetic compatibility and recipe assertions')\n")


def scan(root):
    repos, prs = [], []
    for index, (name, ecosystem, group, number, title) in enumerate(REPOS):
        repo = root / "repos" / name
        file = repo / ".github" / "dependabot.yml"
        raw = file.read_text()
        config = yaml.safe_load(raw)
        if config.get("version") != 2 or not isinstance(config.get("updates"), list):
            raise ValueError(f"Invalid fixture Dependabot config: {name}")
        instructions = (repo / "AGENTS.md").read_text()
        repos.append({"repository": f"example/{name}", "dependabot_path": str(file),
            "dependabot_sha256": hashlib.sha256(raw.encode()).hexdigest(),
            "ecosystems": [u["package-ecosystem"] for u in config["updates"]],
            "updates": config["updates"],
            "groups": [g for u in config["updates"] for g in u.get("groups", {})],
            "agents_path": str(repo / "AGENTS.md"), "agents_sha256": hashlib.sha256(instructions.encode()).hexdigest(),
            "instructions": instructions, "instruction_execution": "read; explicit fixture recipe only",
            "enabled": index not in DISABLED, "status": "idle", "last_activity": now(),
            "prompt": f"Follow AGENTS.md; preserve {group}; apply the {ecosystem} testing recipe.",
            "status_reason": "Synthetic repository configuration read successfully"})
        if index in IDLE:
            continue
        pr_status = "Disabled" if index in DISABLED else "Blocked" if index in BLOCKED else "Verified" if index in HISTORY_COMPLETE else "Queued"
        reason = ("Maintenance disabled for this repository (policy preview)" if index in DISABLED else
                  "Required runner unavailable (synthetic blocker)" if index == 7 else
                  "Repository recipe requires a missing fixture (synthetic blocker)" if index == 12 else
                  "Previously completed synthetic history; not executed during this run" if index in HISTORY_COMPLETE else
                  "Synthetic PR queued; execution is bounded to three primary PRs in this demo")
        for offset in range(3 if index in BLOCKED else 2):
            pr_number = number + offset
            prs.append({"repository": f"example/{name}", "number": pr_number,
                "title": title if offset == 0 else f"Update {group} patch set {offset + 1}",
                "url": f"https://github.com/example/{name}/pull/{pr_number}", "status": pr_status,
                "detail": reason, "ecosystem": ecosystem, "group": group, "head": "fixture-base",
                "checks": [], "synthetic": True, "primary": offset == 0,
                "execution_selected": offset == 0 and index < EXECUTION_LIMIT})
    return repos, prs


def refresh(status):
    for repo in status["repos"]:
        prs = [pr for pr in status["prs"] if pr["repository"] == repo["repository"]]
        active = status["active"]
        if not repo["enabled"]:
            repo["status"] = "disabled"
            repo["status_reason"] = "Disabled by repository policy preview"
        elif active and active["repository"] == repo["repository"] and active["status"] not in ("Verified", "Blocked"):
            repo["status"] = "active"
            repo["status_reason"] = active["detail"]
            repo["last_activity"] = status["updated_at"]
        elif any(pr["status"] == "Blocked" for pr in prs):
            repo["status"] = "blocked"
            repo["status_reason"] = next(pr["detail"] for pr in prs if pr["status"] == "Blocked")
        elif any(pr["status"] == "Queued" for pr in prs):
            repo["status"] = "queued"
            repo["status_reason"] = "Pending synthetic PRs remain in the queue"
        elif prs and all(pr["status"] == "Verified" for pr in prs):
            repo["status"] = "complete"
            repo["status_reason"] = "Synthetic completed PR history"
        else:
            repo["status"] = "idle"
            repo["status_reason"] = "No open synthetic dependency PRs"
    status["queued_count"] = sum(p["status"] == "Queued" for p in status["prs"])
    status["completed_count"] = sum(p["status"] == "Verified" for p in status["prs"])
    status["blocked_count"] = sum(p["status"] == "Blocked" for p in status["prs"])
    status["disabled_count"] = sum(not r["enabled"] for r in status["repos"])


def initial(root):
    seed(root)
    repos, prs = scan(root)
    policy = load_policy(root)
    for repo in repos:
        repo["prompt"] = policy["repo_prompts"].get(repo["repository"], "")
    status = {"schema_version": 1, "synthetic": True, "transport": "ssh", "worker_id": "maintenance-worker-01",
              "phase": "ready", "updated_at": now(), "active": None, "prs": prs, "repos": repos,
              "events": [{"at": now(), "message": "Worker ready. Synthetic repositories and task-owned SSH only."}],
              "global_prompt": policy["global_prompt"], "policy_mode": "SSH-synchronized fixture instructions",
              "configured_revision": policy["revision"], "applied_revision": None,
              "worker_health": {"alive": False, "state": "not_started", "heartbeat_at": None, "pid": None, "last_error": None},
              "execution_limit": EXECUTION_LIMIT, "executed_count": 0,
              "run_scope": "Parse all 21 repositories; execute three selected synthetic PR recipes; retain the remaining queue",
              "detail": "21 repositories and 38 synthetic PRs. No cloud or GitHub writes."}
    refresh(status)
    atomic_json(root / "status.json", status)
    return status


def run(root, delay, stay_alive=False):
    status = initial(root)
    status["phase"] = "running"

    def publish():
        policy = load_policy(root)
        if status["applied_revision"] != policy["revision"]:
            status["global_prompt"] = policy["global_prompt"]
            status["configured_revision"] = policy["revision"]
            status["applied_revision"] = policy["revision"]
            for repo in status["repos"]:
                repo["prompt"] = policy["repo_prompts"].get(repo["repository"], "")
            event = {"at": now(), "message": f"Worker process loaded instruction revision {policy['revision']}. Fixture checks remain mandatory."}
            status["events"].append(event)
            with (root / "events.jsonl").open("a") as stream:
                stream.write(json.dumps(event) + "\n")
            print(event["message"], flush=True)
        status["worker_health"] = {"alive": True, "state": "idle" if status["phase"] == "cycle_completed" else "working",
                                   "heartbeat_at": now(), "pid": os.getpid(), "last_error": None}
        status["_heartbeat_monotonic"] = time.monotonic()
        refresh(status)
        atomic_json(root / "status.json", status)

    def emit(message, pr=None, state=None, detail=None):
        if pr is not None:
            if state is not None:
                pr["status"] = state
            if detail is not None:
                pr["detail"] = detail
            status["active"] = pr
        status["updated_at"] = now()
        event = {"at": now(), "message": message}
        status["events"].append(event)
        publish()
        with (root / "events.jsonl").open("a") as stream:
            stream.write(json.dumps(event) + "\n")
        print(message, flush=True)
        deadline = time.monotonic() + delay
        while time.monotonic() < deadline:
            time.sleep(min(2, max(0, deadline - time.monotonic())))
            publish()

    emit(f"Read .github/dependabot.yml and AGENTS.md in all {len(REPOS)} fixture repositories; groups remain unchanged.")
    emit("Portfolio: 21 repositories, 38 PRs, two disabled repositories. Execute only three selected fixture recipes.")
    selected = [pr for pr in status["prs"] if pr["execution_selected"]]
    for index, pr in enumerate(selected):
        repo = root / "repos" / REPOS[index][0]
        baseline = (repo / ".github/dependabot.yml").read_bytes()
        emit(f"{pr['repository']} #{pr['number']}: preserve {pr['group']} group and trusted repository recipes.", pr, "Reviewing")
        emit("Inspect mocked current-head GitHub Actions checks.", pr, "Running CI")
        pr["instruction_revision"] = status["applied_revision"]
        check = subprocess.run([sys.executable, "checks.py"], cwd=repo, text=True, capture_output=True, timeout=10)
        pr["checks"].append({"kind": "explicit synthetic recipe", "passed": check.returncode == 0,
                             "stdout": check.stdout.strip(), "stderr": check.stderr.strip(), "head": pr["head"]})
        if check.returncode != 0:
            emit("Synthetic API check failed: missing cancellation argument. Keep tests intact.", pr, "Fixing CI", "Scoped fixture compatibility repair in progress")
            api = repo / "api_call.py"
            api.write_text(api.read_text().replace("return send('ok')", "return send('ok', None)"))
            pr["head"] = "fixture-repair"
            emit("Applied compatibility repair; new fixture head requires fresh checks.", pr, "Rerunning CI")
            check = subprocess.run([sys.executable, "checks.py"], cwd=repo, text=True, capture_output=True, timeout=10)
            pr["checks"].append({"kind": "explicit synthetic recipe", "passed": check.returncode == 0,
                                 "stdout": check.stdout.strip(), "stderr": check.stderr.strip(), "head": pr["head"]})
        if check.returncode != 0 or baseline != (repo / ".github/dependabot.yml").read_bytes():
            emit("Fixture verification blocked; no simulated merge.", pr, "Blocked")
            continue
        emit("Synthetic CI and applicable fixture recipes passed. Groups and ignore rules unchanged.", pr, "Checking recipes")
        emit("Mock review resolved; scoped mock merge authorization passes.", pr, "Ready to merge")
        emit("Simulate serial squash merge; no GitHub mutation is performed.", pr, "Post-merge checks")
        emit("Synthetic post-merge verification passed.", pr, "Verified", "All explicit fixture checks passed; simulated merge verified")
        status["executed_count"] += 1
    status["active"] = None
    status["phase"] = "cycle_completed"
    status["detail"] = "Three selected fixture recipes finished. Other queued and blocked PRs remain pending; completed history is synthetic."
    emit("Bounded demo cycle finished: three real fixture recipes. Remaining portfolio stays queued, blocked, idle, or disabled.")
    if stay_alive:
        print("Worker is alive and idle. Policy reload and heartbeat continue every two seconds.", flush=True)
        while True:
            time.sleep(2)
            publish()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--delay", type=float, default=2)
    parser.add_argument("--seed", action="store_true")
    parser.add_argument("--stay-alive", action="store_true")
    options = parser.parse_args()
    options.root.mkdir(parents=True, exist_ok=True)
    initial(options.root) if options.seed else run(options.root, options.delay, options.stay_alive)
