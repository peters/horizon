---
procedure: native-lifecycle-causes
feature: native host lifecycle causes
platforms: [linux]
cost: none
destructive: yes
secrets: [existing provider credential reference in the OS credential store]
owner: peters
---

# Native lifecycle causes test procedure

## 1. Purpose

This procedure tests finite host causes, lane stop behavior, and the first error after a cleanup failure.
It uses local fixtures with synthetic provider responses.

## 2. Applicability

- Candidate: a Horizon source checkout with the native lifecycle cause changes.
- Platforms: Linux with local process fixtures.
- This procedure does not test real devices, provider capacity, backend diagnostic retention, or a 20-minute run.
- The negative stdin smoke tests startup refusal before an MCP handshake.
- The successful packaged stdin smoke reads the provider device catalogue.
  It does not call a tool, upload an app, allocate a session, or start a tunnel.
- The local duplex fixtures test the MCP protocol with synthetic provider responses.

## 3. Safety

> **CAUTION:** USE ONLY TASK-OWNED FIXTURE STATE. These tests start local child processes and delete their private temporary resources.

## 4. Equipment and preconditions

- The repository build requirements.
- A source checkout that passed independent review.
- A private temporary directory without a Git repository ancestor.
- A task-owned build cache without another compiler or test process.
- A working session D-Bus connection for the existing credential fixtures.
- An unlocked OS credential store with the approved provider reference.
- An approved private client and its declared native contract.
  Do not use its original owner or state for this smoke.

## 5. Setup

1. Create a private temporary directory.

   ```bash
   task_tmp=$(python3 -c 'import os,tempfile; p=tempfile.mkdtemp(prefix="horizon-lifecycle-",dir="/dev/shm"); os.chmod(p,0o700); print(p)')
   ```

   Result: The directory has mode `0700` and no Git repository ancestor.

2. Set the test environment.

   ```bash
   export TMPDIR="$task_tmp" TEMP="$task_tmp" TMP="$task_tmp"
   export CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 CARGO_INCREMENTAL=0
   export RUSTFLAGS="-D warnings"
   export XDG_RUNTIME_DIR="/run/user/$(id -u)"
   export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
   ```

   Result: Fixtures use private state and the current user's session D-Bus connection.

3. Set `CARGO_TARGET_DIR` to your task-owned build cache.

   ```bash
   export CARGO_TARGET_DIR="/path/to/task-owned-cache"
   ```

   Result: The source checkout uses the selected cache without another compiler or test process.
   After a cache handoff, use a new validation checkout with identical inputs.
   Its file times must follow the handoff. Do not change existing source file times.

4. Select the absolute paths of the frozen candidate executables.

   ```bash
   export candidate_native="/absolute/path/to/frozen/horizon-native"
   export candidate_horizon="/absolute/path/to/frozen/horizon"
   sha256sum "$candidate_native" "$candidate_horizon"
   ```

   Result: The private manifest identifies both exact executables.

## 6. Tasks

### 6.1 LC-01 — Finite causes and private data

1. Run the lifecycle tests.

   ```bash
   cargo test -p horizon-app-host lifecycle::tests::
   ```

   Result: I/O kinds, task failures, protocol failures, and lock failures retain finite causes without private error text.

### 6.2 LC-02 — Stop the lane after host loss

> **CAUTION:** USE ONLY TASK-OWNED FIXTURE STATE.
> These tests stop synthetic resources and delete their temporary state.

1. Run the runner tests.

   ```bash
   cargo test -p horizon-app-host runner::tests::
   ```

   Result: The first step, blocked progress, and terminal report keep the same cause.
   Later actions do not run. Provider diagnostics remain available when the provider supports them.
   A later cleanup failure does not replace the first cause.

### 6.3 LC-03 — Preserve ownership after failed cleanup

> **CAUTION:** USE ONLY TASK-OWNED FIXTURE STATE.
> These tests stop synthetic resources and delete their temporary state.

1. Run the actor tests.

   ```bash
   cargo test -p horizon-app-host actor::tests::
   ```

   Result: Failed reservation and replacement cleanup keep the first error and report unconfirmed cleanup.
   Ambiguous records remain pending. No replacement allocation repeats after unconfirmed cleanup.

### 6.4 LC-04 — Audit completion failure

> **CAUTION:** USE ONLY TASK-OWNED FIXTURE STATE.
> These tests delete their temporary audit records.

1. Run the audit tests.

   ```bash
   cargo test -p horizon-app-host audit::tests::
   ```

   Result: A failed operation keeps its original cause if its completion receipt also fails.
   A successful operation refuses a failed completion receipt.

### 6.5 LC-05 — Required validation

> **CAUTION:** USE ONLY TASK-OWNED FIXTURE STATE.
> These tests stop synthetic resources and delete their temporary state.

1. Run the repository pre-push commands from `AGENTS.md` in the exact candidate checkout.

   Result: Required tests and Clippy tiers pass.
   The report identifies any unchanged advisory warnings separately.

### 6.6 LC-06 — Frozen CLI and stdin startup refusal

1. Run the frozen executable checks with a missing synthetic client.

   ```bash
   python3 - <<'PY'
   import json, os, pathlib, subprocess
   missing = pathlib.Path(os.environ["TMPDIR"]) / "private-client-does-not-exist.json"
   assert not missing.exists()
   frame = json.dumps({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
       "protocolVersion":"2025-06-18","capabilities":{},
       "clientInfo":{"name":"synthetic-fixture","version":"1"}}}) + "\n"
   for variable, modes in [("candidate_native", ["--run", "--mcp"]),
                           ("candidate_horizon", ["--native-run", "--native-mcp"])]:
       binary = pathlib.Path(os.environ[variable])
       assert binary.is_absolute() and binary.is_file()
       for mode in modes:
           result = subprocess.run([str(binary), mode, "--client", str(missing)],
               input=frame if "mcp" in mode else "", text=True, capture_output=True, timeout=15)
           assert result.returncode == 2 and result.stdout == ""
           expected = "app_host_unavailable: Project: I/O NotFound"
           if "mcp" in mode:
               assert result.stderr.strip() == expected
           else:
               event = json.loads(result.stderr)
               assert event["phase"] == "error" and event["message"] == expected
           assert str(missing) not in result.stderr
           print(mode, "pass")
   PY
   ```

   Result: Both CLI and stdin modes retain the same typed cause without the private path.
   No credential lookup, provider allocation, or MCP handshake occurs.

### 6.7 LC-07 — Synthetic MCP protocol

> **CAUTION:** USE ONLY TASK-OWNED FIXTURE STATE.
> These tests stop synthetic resources and delete their temporary state.

1. Run the existing MCP duplex fixtures.

   ```bash
   cargo test -p horizon-app-host mcp::tests::
   ```

   Result: The synthetic controller uses the MCP protocol and retains owned cleanup.
   These fixtures do not qualify a successful packaged stdin handshake.

2. Run the stdio shutdown regressions.

   ```bash
   cargo test -p horizon-app-host mcp::shutdown_tests::
   ```

   Result: Shutdown runs after a failed MCP operation.
   A shutdown failure does not replace the original MCP cause.
   A failed or panicking shutdown appends `app_resource_cleanup_uncertain` once after the original cause.
   After a successful MCP operation, shutdown failures remain visible.

### 6.8 LC-08 — Packaged stdin initialization and tool discovery

> **CAUTION:** USE ONLY THE APPROVED CREDENTIAL REFERENCE AND NEW TASK-OWNED STATE.
> Startup reads the provider device catalogue and writes a private local journal.
> Do not call a tool or change the original client, owner, state, or configuration.

1. Set `approved_client` to the existing approved private client.

   ```bash
   export approved_client="/absolute/path/to/approved/private-client.json"
   ```

   Result: Horizon can resolve the existing provider reference in its credential store.
   No credential value appears in the fixture client.

2. Create a new fixture client and copy only the declared contract.

   ```bash
   export mcp_fixture_client=$(python3 - <<'PY'
   import json, os, pathlib, re, tempfile, uuid
   source = pathlib.Path(os.environ["approved_client"])
   assert source.is_absolute() and source.is_file() and not source.is_symlink()
   assert source.stat().st_uid == os.getuid() and source.stat().st_mode & 0o077 == 0
   client = json.loads(source.read_text())
   declared = (pathlib.Path(client["project"]) / "AGENTS.md").read_text()
   blocks = re.findall(r'```(?:yaml|yml)\s*\n(.*?)```', declared, re.S)
   blocks = [block for block in blocks if "remote-device-testing:" in block]
   assert len(blocks) == 1
   root = pathlib.Path(tempfile.mkdtemp(prefix="horizon-lifecycle-mcp-", dir="/var/tmp"))
   os.chmod(root, 0o700)
   project, state = root / "project", root / "state"
   project.mkdir(mode=0o700); state.mkdir(mode=0o700)
   agents = project / "AGENTS.md"
   with os.fdopen(os.open(agents, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as file:
       file.write("# Synthetic discovery fixture\n\n```yaml\n" + blocks[0] + "```\n")
   client.update(owner=str(uuid.uuid4()), project=str(project), state=str(state))
   target = root / "client.json"
   with os.fdopen(os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as file:
       json.dump(client, file)
   print(target)
   PY
   )
   ```

   Result: The new owner has a separate project and state.
   The approved provider name and pinned tunnel reference remain unchanged.
   The fixture contains no app, backend, recipe, or executable build input.

3. Run the exact frozen packaged executable with the protocol messages.

   ```bash
   python3 - <<'PY'
   import hashlib, json, os, pathlib, selectors, stat, subprocess, time
   client = pathlib.Path(os.environ["mcp_fixture_client"])
   binary = pathlib.Path(os.environ["candidate_horizon"])
   assert binary.is_absolute() and binary.is_file()
   original = pathlib.Path(os.environ["approved_client"])
   approved = json.loads(original.read_text())
   candidates = []
   if os.environ.get("HOME"):
       candidates += [pathlib.Path(os.environ["HOME"]) / ".horizon" / name
           for name in ["config.yaml", "config.yml"]]
   if os.environ.get("XDG_CONFIG_HOME"):
       candidates += [pathlib.Path(os.environ["XDG_CONFIG_HOME"]) / "horizon" / name
           for name in ["config.yaml", "config.yml"]]
   candidates += [pathlib.Path("horizon.yaml"), pathlib.Path("horizon.yml")]
   config = next(path for path in candidates if path.exists())
   def snapshot():
       state = pathlib.Path(approved["state"])
       paths = [original, config, state, *sorted(state.rglob("*"))]
       assert len(paths) <= 8192
       entries, total = [], 0
       for path in paths:
           metadata = path.lstat()
           assert stat.S_ISREG(metadata.st_mode) or stat.S_ISDIR(metadata.st_mode)
           digest = None
           if stat.S_ISREG(metadata.st_mode):
               total += metadata.st_size
               assert total <= 32 * 1024 * 1024 * 1024
               def identity(info):
                   return (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_gid,
                       info.st_size, info.st_mtime_ns, info.st_ctime_ns, info.st_nlink)
               with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW), "rb") as file:
                   assert identity(os.fstat(file.fileno())) == identity(metadata)
                   hasher = hashlib.sha256()
                   for block in iter(lambda: file.read(1048576), b""):
                       hasher.update(block)
                   assert identity(os.fstat(file.fileno())) == identity(metadata)
               assert identity(path.lstat()) == identity(metadata)
               digest = hasher.hexdigest()
           entries.append([str(path), metadata.st_dev, metadata.st_ino,
               metadata.st_mode, metadata.st_uid, metadata.st_gid, metadata.st_size,
               metadata.st_mtime_ns, metadata.st_ctime_ns, metadata.st_nlink, digest])
       return entries
   before = snapshot()
   def save(name, value):
       with os.fdopen(os.open(client.parent / name,
           os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as file:
           json.dump(value, file)
   save("original-before.json", before)
   stderr_path = client.parent / "startup-stderr.log"
   with os.fdopen(os.open(stderr_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as stderr:
       process = subprocess.Popen([str(binary), "--native-mcp", "--client", str(client)],
           stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr)
       selector = selectors.DefaultSelector()
       selector.register(process.stdout, selectors.EVENT_READ)
       os.set_blocking(process.stdout.fileno(), False)
       pending = bytearray()
       deadline = time.monotonic() + 60
       allowed = {"initialize", "notifications/initialized", "tools/list"}
       def send(message):
           assert message["method"] in allowed
           process.stdin.write((json.dumps(message) + "\n").encode()); process.stdin.flush()
       def response(identifier):
           while time.monotonic() < deadline:
               while b"\n" in pending:
                   line, _, rest = pending.partition(b"\n"); pending[:] = rest
                   message = json.loads(line)
                   if message.get("id") == identifier:
                       assert "error" not in message
                       return message["result"]
               assert process.poll() is None
               for key, _ in selector.select(min(1, max(0, deadline - time.monotonic()))):
                   data = os.read(key.fd, 65536)
                   assert data
                   pending.extend(data)
                   assert len(pending) <= 1048576
           raise TimeoutError("MCP discovery deadline")
       try:
           send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
               "protocolVersion":"2025-06-18","capabilities":{},
               "clientInfo":{"name":"synthetic-fixture","version":"1"}}})
           initialized = response(1)
           assert initialized["capabilities"].get("tools") is not None
           send({"jsonrpc":"2.0","method":"notifications/initialized"})
           send({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}})
           tools = response(2)["tools"]
           names = {tool["name"] for tool in tools}
           assert {"device_test_run", "app_snapshot", "app_session_create"} <= names
           proof = {"initialize":initialized, "tools":tools, "tool_calls":0}
           save("discovery.json", proof)
           process.stdin.close()
           assert process.wait(timeout=15) == 0
           after = snapshot()
           save("original-after.json", after)
           assert before == after
           print("initialize pass; tools/list pass; tool calls 0")
       finally:
           selector.close()
           if process.poll() is None:
               process.terminate()
               try:
                   process.wait(timeout=5)
               except subprocess.TimeoutExpired:
                   process.kill(); process.wait(timeout=5)
   PY
   ```

   Result: Initialization and tool discovery succeed without a tool call.
   The process exits normally after stdin closes.
   The original configuration, client, and state match their initial private snapshot.
   Each regular file has a streamed hash and stable metadata during the read.
   The snapshot stops at 8,192 entries or 32 GiB. It does not omit large retained evidence.
   Keep the bounded responses, executable hash, and private stderr in the evidence record.

4. Inspect only the new fixture owner's pending operations.

   ```bash
   "$candidate_horizon" --native-reconcile-status --client "$mcp_fixture_client" |
     python3 -c 'import json,sys; result=json.load(sys.stdin); assert result["operations"] == []; print("pending operations 0")'
   ```

   Result: `operations` is empty.
   No session, upload, backend, or tunnel was allocated.

## 7. Pass criteria

- LC-01 through LC-08 pass for the same frozen candidate source.
- The first error remains visible after a later cleanup failure.
- No action resumes after a fatal host failure.
- Private error payloads do not appear in public host causes.
- Unconfirmed cleanup does not release ambiguous ownership.

## 8. Cleanup

1. Examine the fixture cleanup results.

   Result: Local fixture resources close, or the result identifies retained uncertainty.

2. Preserve private test logs and source hashes.

   Result: The run remains available for review.

3. Keep the packaged discovery fixture and its journal.

   Result: The original private owner and its state remain unchanged.
   Do not run resource reconciliation or delete a fixture with unresolved work.

## 9. Record of results

Use [the report template](../reports/TEMPLATE.md) for a final report.
Record the exact candidate commit, source hash, command results, and cleanup results.
Keep private evidence out of the repository.
