#!/usr/bin/env python3
"""Land approved pull requests in tested batches, without a rebase race.

GitHub's merge queue needs an organization-owned repository. This script gives the
same guarantee by hand: main plus the queued PRs is tested once on an `mq/*` branch,
and only that tested result lands.

- enqueue: the PR's agent queues one exact head after every merge gate in AGENTS.md
  passed for it. The entry is a PR comment by the repository owner, plus the
  `merge-queue` label.
- build: applies each queued head to a fresh origin/main as one squash commit, in
  queue order, and pushes the result as `mq/<UTC time>`. CI runs on that branch.
- land: when CI on the batch tip passed, main has not moved and no head drifted,
  squash-merges each PR with its head guard, then checks that each merge commit has
  the tested tree.

GitHub cannot guard the base of a squash merge, so the tree check runs after each
merge: a push to main by someone else during `land` is found and stops the batch, but
is not prevented. Only the batch tip is tested; if `land` stops partway, main holds an
untested prefix of the batch until the next batch or a fix lands, and main CI checks it.

Usage: merge-queue.py enqueue PR --head SHA --subject TEXT --body-file FILE
       merge-queue.py dequeue PR --reason TEXT
       merge-queue.py list | status
       merge-queue.py build [--only N,M] [--dry-run]
       merge-queue.py land [--dry-run]

The batch plan is private state under $XDG_STATE_HOME/horizon-merge-queue (default
~/.local/state/horizon-merge-queue), never in the repository.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LABEL = "merge-queue"
MARKER = "<!-- merge-queue v1 -->"
DEQUEUED = "<!-- merge-queue dequeued -->"
# Only peters may give merge permission, and agents act with that account.
TRUSTED = {"OWNER"}
BASE_BRANCH = "main"
CI_WORKFLOW = "CI"
GOOD_CONCLUSIONS = {"success", "skipped", "neutral"}
# Every form that makes GitHub skip workflows for a push.
SKIP_FORMS = re.compile(r"\[(?:skip ci|ci skip|no ci|skip actions|actions skip)\]"
                        r"|^skip-checks:\s*true", re.I | re.M)
CLOSING = re.compile(
    r"\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\b:?\s+"
    r"(?:(?:[\w.-]+/[\w.-]+)?#\d+|GH-\d+|https?://\S+/(?:issues|pull)/\d+)", re.I)
ENTRY_JSON = re.compile(r"^```json\n(\{[^\n]*\})\n```$", re.M)
TIMEOUT = 120


class QueueError(Exception):
    pass


def run(args, cwd=ROOT, timeout=TIMEOUT, check=True, stdin=None):
    try:
        out = subprocess.run(args, cwd=cwd, capture_output=True, text=True, timeout=timeout,
                             input=stdin, check=False)
    except subprocess.TimeoutExpired:
        raise QueueError(f"{' '.join(args[:3])}: timed out after {timeout} s")
    if check and out.returncode != 0:
        raise QueueError(f"{' '.join(args[:4])}: {out.stderr.strip() or out.stdout.strip()}")
    return out


def git(*args, cwd=ROOT, timeout=TIMEOUT, check=True):
    return run(["git", *args], cwd=cwd, timeout=timeout, check=check)


def gh_json(*args):
    out = run(["gh", *args]).stdout
    return json.loads(out) if out.strip() else None


def gh_pages(path):
    """Every item of a paginated REST list."""
    out = run(["gh", "api", "--paginate", "--slurp", path]).stdout
    return [item for page in json.loads(out) for item in page]


def repo_name():
    return os.environ.get("MERGE_QUEUE_REPO") or run(
        ["gh", "repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"]).stdout.strip()


# Pure logic. Each part below takes plain data so the tests need no network.

def message_problems(subject, body):
    """Why a squash subject or body may not land, or [] when it may."""
    problems = []
    text = f"{subject}\n{body}"
    if not subject.strip():
        problems.append("the subject is empty")
    if "\n" in subject:
        problems.append("the subject has more than one line")
    skip = SKIP_FORMS.search(text)
    if skip:
        problems.append(f"it contains {skip.group(0)!r}: main would skip CI for the squash "
                        "commit")
    if re.search(r"co-authored-by", text, re.I):
        problems.append("it contains a Co-authored-by trailer")
    match = CLOSING.search(text)
    if match:
        problems.append(f"it contains the closing keyword {match.group(0)!r}")
    return problems


def entry_comment(head, subject, body):
    payload = json.dumps({"head": head, "subject": subject, "body": body}, ensure_ascii=False)
    return (f"{MARKER}\nQueued for the merge queue at head `{head}`. A later push drops "
            f"this entry.\n\n```json\n{payload}\n```\n")


def parse_entry(body):
    """The head, subject and body of a queue entry comment, or None."""
    if not body.startswith(MARKER):
        return None
    match = ENTRY_JSON.search(body)
    if not match:
        return None
    try:
        data = json.loads(match.group(1))
    except json.JSONDecodeError:
        return None
    if not (isinstance(data, dict) and all(isinstance(data.get(k), str)
                                           for k in ("head", "subject", "body"))):
        return None
    if not re.fullmatch(r"[0-9a-f]{40}", data["head"]):
        return None
    if message_problems(data["subject"], data["body"]):
        return None
    return {"head": data["head"], "subject": data["subject"], "body": data["body"]}


def latest_entry(comments):
    """The newest valid entry by a trusted author, unless a later dequeue note voids it.
    Each comment is a REST issue comment: body, created_at, author_association."""
    found = None
    for comment in sorted(comments, key=lambda c: c["created_at"]):
        if comment.get("author_association") not in TRUSTED:
            continue
        body = comment.get("body") or ""
        if body.startswith(DEQUEUED):
            found = None
            continue
        entry = parse_entry(body)
        if entry:
            found = {**entry, "queued_at": comment["created_at"]}
    return found


def queue_order(entries):
    """Entries oldest first; the PR number breaks a tie."""
    return sorted(entries, key=lambda e: (e["queued_at"], e["number"]))


def ci_verdict(runs, check_runs):
    """(passed, problems) for CI on one commit. Runs are workflow runs, check_runs the
    jobs. Nothing passes before a run exists or while any job is still open."""
    problems = []
    if not any(item["name"] == CI_WORKFLOW for item in runs):
        problems.append(f"no {CI_WORKFLOW} workflow run for this commit yet")
    for item in runs:
        if item["status"] != "completed":
            problems.append(f"workflow {item['name']} is {item['status']}")
        elif item["conclusion"] not in GOOD_CONCLUSIONS:
            problems.append(f"workflow {item['name']} concluded {item['conclusion']}")
    for item in check_runs:
        if item["status"] != "completed":
            problems.append(f"job {item['name']} is {item['status']}")
        elif item["conclusion"] not in GOOD_CONCLUSIONS:
            problems.append(f"job {item['name']} concluded {item['conclusion']}")
    return (not problems, problems)


def drifted(entry, current_head):
    return entry["head"] != current_head


def landed_base(plan):
    """The commit main must be at now: the last merge `land` made, or the batch base. A
    `land` that stopped partway resumes from there."""
    merged = [e["merged"] for e in plan["prs"] if e.get("merged")]
    return merged[-1] if merged else plan["base"]


def transient_merge_refusal(stderr):
    """GitHub refuses a merge for a moment after the base branch changes."""
    return re.search(r"base branch was modified|not mergeable", stderr, re.I) is not None


# Git work on a private detached worktree, so no checkout of the user's changes.

def apply_batch(repo, base, entries, fetch_head):
    """Squash each entry's head onto base in order. Returns (tip, applied, skipped); each
    applied entry gains "commit", its squash commit in the batch.
    fetch_head(entry) makes the head commit available in repo and returns nothing."""
    work = Path(tempfile.mkdtemp(prefix="batch-", dir=state_dir()))
    applied, skipped = [], []
    try:
        git("worktree", "add", "--detach", str(work), base, cwd=repo)
        for entry in entries:
            try:
                fetch_head(entry)
            except QueueError as error:
                skipped.append((entry, f"cannot fetch head: {error}"))
                continue
            merged = git("merge", "--squash", "--no-commit", entry["head"], cwd=work,
                         check=False)
            if merged.returncode != 0:
                git("reset", "--hard", "-q", "HEAD", cwd=work)
                skipped.append((entry, "conflicts with main or an earlier PR in the batch; "
                                       "rebase onto main to resolve"))
                continue
            if git("diff", "--cached", "--quiet", cwd=work, check=False).returncode == 0:
                skipped.append((entry, "adds nothing to main"))
                continue
            message = entry["subject"] + (f"\n\n{entry['body']}" if entry["body"].strip() else "")
            run(["git", "commit", "-q", "--no-verify", "-F", "-"], cwd=work, stdin=message)
            applied.append({**entry, "commit": git("rev-parse", "HEAD", cwd=work).stdout.strip()})
        tip = git("rev-parse", "HEAD", cwd=work).stdout.strip()
    finally:
        git("worktree", "remove", "--force", str(work), cwd=repo, check=False)
        shutil.rmtree(work, ignore_errors=True)
        git("worktree", "prune", cwd=repo, check=False)
    return tip, applied, skipped


def tree(repo, commit):
    return git("rev-parse", f"{commit}^{{tree}}", cwd=repo).stdout.strip()


def trees_match(repo, left, right):
    return tree(repo, left) == tree(repo, right)


# GitHub access.

def state_dir():
    base = Path(os.environ.get("XDG_STATE_HOME") or Path.home() / ".local" / "state")
    path = base / "horizon-merge-queue"
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    return path


def plan_path():
    return state_dir() / "batch.json"


def load_plan():
    path = plan_path()
    if not path.exists():
        raise QueueError("no batch: run build first")
    return json.loads(path.read_text())


def pull(repo, number):
    return gh_json("pr", "view", str(number), "--repo", repo, "--json",
                   "number,state,isDraft,headRefOid,baseRefName,title,labels,reviewDecision")


def queued(repo):
    """Open labeled PRs with a valid entry, oldest entry first."""
    prs = gh_json("pr", "list", "--repo", repo, "--label", LABEL, "--state", "open",
                  "--limit", "100", "--json", "number,headRefOid,baseRefName,title,isDraft")
    entries = []
    for pr in prs:
        entry = latest_entry(gh_pages(f"repos/{repo}/issues/{pr['number']}/comments"))
        if entry is None:
            print(f"#{pr['number']}: labeled but has no valid entry comment; ignored")
            continue
        entries.append({**entry, "number": pr["number"], "title": pr["title"],
                        "current_head": pr["headRefOid"], "draft": pr["isDraft"],
                        "base_branch": pr["baseRefName"]})
    return queue_order(entries)


def unresolved_threads(repo, number):
    owner, name = repo.split("/")
    query = ("query($owner:String!,$name:String!,$number:Int!,$after:String){repository("
             "owner:$owner,name:$name){pullRequest(number:$number){reviewThreads(first:100,"
             "after:$after){nodes{isResolved}pageInfo{hasNextPage endCursor}}}}}")
    count, after = 0, None
    while True:
        args = ["gh", "api", "graphql", "-f", f"query={query}", "-F", f"owner={owner}",
                "-F", f"name={name}", "-F", f"number={number}"]
        if after:
            args += ["-F", f"after={after}"]
        threads = json.loads(run(args).stdout)["data"]["repository"]["pullRequest"][
            "reviewThreads"]
        count += sum(1 for node in threads["nodes"] if not node["isResolved"])
        if not threads["pageInfo"]["hasNextPage"]:
            return count
        after = threads["pageInfo"]["endCursor"]


def ci_on(repo, sha):
    runs = gh_json("api", f"repos/{repo}/actions/runs?head_sha={sha}&per_page=100",
                   "--jq", "[.workflow_runs[] | {name, status, conclusion}]") or []
    checks = [{"name": c["name"], "status": c["status"], "conclusion": c["conclusion"]}
              for page in json.loads(run(["gh", "api", "--paginate", "--slurp",
                                          f"repos/{repo}/commits/{sha}/check-runs?per_page=100"]
                                         ).stdout)
              for c in page["check_runs"]]
    return ci_verdict(runs, checks)


# Commands.

def cmd_enqueue(args):
    repo = repo_name()
    body = Path(args.body_file).read_text()
    problems = message_problems(args.subject, body)
    if problems:
        raise QueueError("squash message refused: " + "; ".join(problems))
    pr = pull(repo, args.pr)
    if pr["state"] != "OPEN" or pr["isDraft"]:
        raise QueueError(f"#{args.pr} is not an open, ready PR")
    if pr["headRefOid"] != args.head:
        raise QueueError(f"#{args.pr} head is {pr['headRefOid']}, not {args.head}")
    if pr["baseRefName"] != BASE_BRANCH:
        raise QueueError(f"#{args.pr} targets {pr['baseRefName']}, not {BASE_BRANCH}")
    created = run(["gh", "label", "create", LABEL, "--repo", repo, "--color", "0e8a16",
                   "--description", "Approved head waiting for a merge-queue batch"],
                  check=False)
    if created.returncode != 0 and "already exists" not in created.stderr:
        raise QueueError(f"cannot create label {LABEL}: {created.stderr.strip()}")
    run(["gh", "pr", "comment", str(args.pr), "--repo", repo, "--body-file", "-"],
        stdin=entry_comment(args.head, args.subject, body))
    run(["gh", "pr", "edit", str(args.pr), "--repo", repo, "--add-label", LABEL])
    entry = latest_entry(gh_pages(f"repos/{repo}/issues/{args.pr}/comments"))
    if entry is None or entry["head"] != args.head:
        run(["gh", "pr", "edit", str(args.pr), "--repo", repo, "--remove-label", LABEL],
            check=False)
        raise QueueError("the entry is not trusted: post it with the repository owner account")
    print(f"#{args.pr} queued at {args.head}")


def cmd_dequeue(args):
    repo = repo_name()
    run(["gh", "pr", "comment", str(args.pr), "--repo", repo, "--body-file", "-"],
        stdin=f"{DEQUEUED}\nRemoved from the merge queue: {args.reason}\n")
    run(["gh", "pr", "edit", str(args.pr), "--repo", repo, "--remove-label", LABEL])
    print(f"#{args.pr} dequeued")


def cmd_list(_args):
    entries = queued(repo_name())
    if not entries:
        print("The merge queue is empty.")
    for entry in entries:
        note = "head drifted: requeue the new head" if drifted(entry, entry["current_head"]) \
            else ("draft" if entry["draft"] else "ready")
        print(f"#{entry['number']} {entry['head'][:9]} {entry['queued_at']} {note}: "
              f"{entry['subject']}")


def cmd_build(args):
    repo = repo_name()
    entries = queued(repo)
    for entry in entries:
        if entry["draft"] or entry["base_branch"] != BASE_BRANCH:
            print(f"#{entry['number']}: draft or not based on {BASE_BRANCH}; skipped")
    entries = [e for e in entries if not e["draft"] and e["base_branch"] == BASE_BRANCH]
    if args.only:
        try:
            wanted = {int(n) for n in args.only.split(",")}
        except ValueError:
            raise QueueError(f"--only takes PR numbers separated by commas, not {args.only!r}")
        entries = [e for e in entries if e["number"] in wanted]
    drift = [e for e in entries if drifted(e, e["current_head"])]
    for entry in drift:
        print(f"#{entry['number']}: head drifted to {entry['current_head'][:9]}; skipped")
    entries = [e for e in entries if e not in drift]
    if not entries:
        print("Nothing to build: the merge queue has no ready entry.")
        return
    git("fetch", "-q", "origin", BASE_BRANCH, timeout=300)
    base = git("rev-parse", f"origin/{BASE_BRANCH}").stdout.strip()

    def fetch_head(entry):
        git("fetch", "-q", "origin", f"pull/{entry['number']}/head", timeout=300)
        if git("rev-parse", "FETCH_HEAD").stdout.strip() != entry["head"]:
            raise QueueError("the PR head moved while building")

    tip, applied, skipped = apply_batch(ROOT, base, entries, fetch_head)
    for entry, reason in skipped:
        print(f"#{entry['number']}: skipped, {reason}")
    if not applied:
        print("Nothing to push: every entry was skipped.")
        return
    branch = "mq/" + datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    print(f"batch {branch}: base {base[:9]}, tip {tip[:9]}, PRs "
          + ", ".join(f"#{e['number']}" for e in applied))
    if args.dry_run:
        print("dry run: nothing pushed")
        return
    git("push", "-q", "origin", f"{tip}:refs/heads/{branch}", timeout=300)
    # A local ref keeps the batch commits from garbage collection until land compares them.
    git("update-ref", f"refs/merge-queue/{branch}", tip)
    plan = {"branch": branch, "base": base, "tip": tip,
            "prs": [{k: e[k] for k in ("number", "head", "subject", "body", "commit")}
                    for e in applied]}
    if plan_path().exists():
        # The earlier batch is replaced; its branch would otherwise stay on origin.
        old = json.loads(plan_path().read_text())["branch"]
        git("push", "-q", "origin", "--delete", old, timeout=300, check=False)
        git("update-ref", "-d", f"refs/merge-queue/{old}", check=False)
    plan_path().write_text(json.dumps(plan, indent=2) + "\n")
    print(f"pushed {branch}; CI runs on {tip}")


def blockers(repo, plan):
    """Every reason the batch may not land now."""
    found = []
    _passed, problems = ci_on(repo, plan["tip"])
    found += [f"CI on the batch tip: {p}" for p in problems]
    git("fetch", "-q", "origin", BASE_BRANCH, timeout=300)
    main = git("rev-parse", f"origin/{BASE_BRANCH}").stdout.strip()
    if main != landed_base(plan):
        found.append(f"main moved from {landed_base(plan)[:9]} to {main[:9]}: build again")
    for entry in plan["prs"]:
        if entry.get("merged"):
            continue
        pr = pull(repo, entry["number"])
        if pr["state"] != "OPEN" or pr["isDraft"]:
            found.append(f"#{entry['number']} is not open and ready")
        if pr["baseRefName"] != BASE_BRANCH:
            found.append(f"#{entry['number']} targets {pr['baseRefName']}, not {BASE_BRANCH}")
        if drifted(entry, pr["headRefOid"]):
            found.append(f"#{entry['number']} head drifted to {pr['headRefOid'][:9]}")
        current = latest_entry(gh_pages(f"repos/{repo}/issues/{entry['number']}/comments"))
        if LABEL not in {label["name"] for label in pr["labels"]} or current is None \
                or current["head"] != entry["head"]:
            found.append(f"#{entry['number']} left the queue or was queued again")
        if pr.get("reviewDecision") == "CHANGES_REQUESTED":
            found.append(f"#{entry['number']} has a review that requests changes")
        threads = unresolved_threads(repo, entry["number"])
        if threads:
            found.append(f"#{entry['number']} has {threads} unresolved review threads")
    return found


def cmd_status(_args):
    repo = repo_name()
    plan = load_plan()
    print(f"batch {plan['branch']}: base {plan['base'][:9]}, tip {plan['tip'][:9]}, PRs "
          + ", ".join(f"#{e['number']}" for e in plan["prs"]))
    found = blockers(repo, plan)
    print("ready to land" if not found else "\n".join(f"- {item}" for item in found))


def cmd_land(args):
    repo = repo_name()
    plan = load_plan()
    found = blockers(repo, plan)
    if found:
        raise QueueError("the batch cannot land:\n" + "\n".join(f"- {i}" for i in found))
    if args.dry_run:
        print("dry run: would land " + ", ".join(f"#{e['number']}" for e in plan["prs"]
                                                 if not e.get("merged")))
        return
    expected = landed_base(plan)
    for entry in plan["prs"]:
        if entry.get("merged"):
            continue
        for attempt in range(5):
            # Each merge must land on the commit the previous one made. Another push to
            # main in between would put this PR on an untested base.
            git("fetch", "-q", "origin", BASE_BRANCH, timeout=300)
            main_now = git("rev-parse", f"origin/{BASE_BRANCH}").stdout.strip()
            if main_now != expected:
                raise QueueError(f"main moved to {main_now[:9]} outside the batch before "
                                 f"#{entry['number']}; it and later PRs were not merged")
            try:
                result = run(["gh", "pr", "merge", str(entry["number"]), "--repo", repo,
                              "--squash", "--match-head-commit", entry["head"],
                              "--subject", entry["subject"], "--body", entry["body"]],
                             timeout=300, check=False)
                error = result.stderr if result.returncode else ""
            except QueueError as timeout:
                # The merge may still have gone through: the state poll below decides.
                error = str(timeout)
                break
            if not error or not transient_merge_refusal(error):
                break
            time.sleep(5 * (attempt + 1))
        for _attempt in range(10):
            # GitHub can report the merge commit a moment after the merge returns.
            merged = gh_json("pr", "view", str(entry["number"]), "--repo", repo, "--json",
                             "state,mergeCommit")
            commit = (merged.get("mergeCommit") or {}).get("oid")
            if merged["state"] == "MERGED" and commit:
                break
            time.sleep(2)
        if merged["state"] != "MERGED" or not commit:
            reason = error.strip() or "GitHub did not merge it"
            raise QueueError(f"#{entry['number']} did not merge ({reason}); later PRs were not "
                             "merged. Run land again to resume.")
        git("fetch", "-q", "origin", BASE_BRANCH, timeout=300)
        if not trees_match(ROOT, commit, entry["commit"]):
            raise QueueError(f"MISMATCH: #{entry['number']} merged as {commit}, but its tree "
                             "is not the tested batch tree. Another merge may have landed "
                             "first. Later PRs were not merged. Check main CI now.")
        print(f"#{entry['number']} merged as {commit}")
        entry["merged"] = commit
        plan_path().write_text(json.dumps(plan, indent=2) + "\n")
        expected = commit
    git("push", "-q", "origin", "--delete", plan["branch"], timeout=300, check=False)
    git("update-ref", "-d", f"refs/merge-queue/{plan['branch']}", check=False)
    plan_path().rename(state_dir() / f"landed-{plan['branch'][3:]}.json")
    print("each merge has its tested tree; batch landed")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    enqueue = sub.add_parser("enqueue", help="queue one approved head")
    enqueue.add_argument("pr", type=int)
    enqueue.add_argument("--head", required=True)
    enqueue.add_argument("--subject", required=True)
    enqueue.add_argument("--body-file", required=True)
    dequeue = sub.add_parser("dequeue", help="remove a PR from the queue")
    dequeue.add_argument("pr", type=int)
    dequeue.add_argument("--reason", required=True)
    sub.add_parser("list", help="show the queue")
    sub.add_parser("status", help="show the current batch and what blocks it")
    build = sub.add_parser("build", help="build and push a batch branch")
    build.add_argument("--only", help="comma-separated PR numbers")
    build.add_argument("--dry-run", action="store_true")
    land = sub.add_parser("land", help="merge the tested batch")
    land.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)
    commands = {"enqueue": cmd_enqueue, "dequeue": cmd_dequeue, "list": cmd_list,
                "status": cmd_status, "build": cmd_build, "land": cmd_land}
    try:
        commands[args.command](args)
    except QueueError as error:
        print(f"merge-queue: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
