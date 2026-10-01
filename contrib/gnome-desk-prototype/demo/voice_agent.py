#!/usr/bin/env python3
"""A live voice assistant for the Horizon assistant panel (demo).

It runs as the panel's process, so it carries the panel's Horizon identity. It connects to the
OpenAI Realtime API, hears a spoken request, and decides for itself which Horizon MCP tools to call
(through the same stdio MCP server an agent would use). Everything it does is real tool use against
the live panels; only the person's voice is a recording.

Environment:
  OPENAI_KEY_FILE            file holding the API key (never the key itself)
  HORIZON_MCP_BIN            the Horizon executable (run with --browser-mcp)
  HORIZON_DESK_SCRIPT        the command file the Horizon bar reads (for the live bar: say/level/speaking)
  HORIZON_VOICE_REQUEST_PCM  the person's spoken request, 24 kHz mono 16-bit PCM
  HORIZON_VOICE_REQUEST_TEXT file with what the recording says (shown in the bar while it plays)
  HORIZON_VOICE_OUT          directory for the assistant's audio and the event log
"""
import asyncio, base64, heapq, json, math, os, struct, subprocess, sys, time, wave

import websockets

KEY = open(os.environ["OPENAI_KEY_FILE"]).read().strip()
MODEL = os.environ.get("HORIZON_VOICE_MODEL", "gpt-realtime-2.1")
VOICE = os.environ.get("HORIZON_VOICE_NAME", "marin")
CMD = os.environ.get("HORIZON_DESK_SCRIPT", "")
OUT = os.environ["HORIZON_VOICE_OUT"]
REQUEST_PCM = os.environ["HORIZON_VOICE_REQUEST_PCM"]
REQUEST_TEXT = open(os.environ["HORIZON_VOICE_REQUEST_TEXT"]).read().strip()
RATE = 24000
os.makedirs(OUT, exist_ok=True)

# What the assistant may use. Read-only helpers and the coordination tool; nothing that spends money or
# changes the host (no cloud tools, no creating or closing panels).
ALLOWED = {"agent_panels": None, "browser_list": None, "device_panel": {"list", "inspect"}}

# The MCP schemas use tagged unions that the Realtime API does not accept, so these two are written out by
# hand from the same operations; the server still validates every call.
HAND = {
    "agent_panels": {"type": "object", "properties": {
        "operation": {"type": "string", "enum": ["list", "send", "read", "note", "plan", "approvals"]},
        "panel_id": {"type": "string", "description": "send and read: a panel_id from list"},
        "text": {"type": "string", "description": "send: the complete message to type into the agent"},
        "submit": {"type": "boolean", "description": "send: press Enter after typing (default true)"},
        "lines": {"type": "integer", "description": "read: how many of the last lines (default 40)"},
        "title": {"type": "string", "description": "note: a short heading"},
        "markdown": {"type": "string", "description": "note: the body, as markdown"},
        "steps": {"type": "array", "description": "plan: the whole plan, replacing the previous one", "items": {
            "type": "object", "properties": {
                "title": {"type": "string"}, "detail": {"type": "string", "description": "short, such as the agent's name"},
                "status": {"type": "string", "enum": ["pending", "running", "done", "failed"]}},
            "required": ["title", "status"]}}},
        "required": ["operation"]},
    "device_panel": {"type": "object", "properties": {
        "operation": {"type": "string", "enum": ["list", "inspect"]},
        "panel_id": {"type": "string"}}, "required": ["operation"]},
}

INSTRUCTIONS = """You are Horizon's voice assistant. Horizon is a workspace for developers: terminals, coding agents, browser panels and native app viewers, spread over several workspaces on one desktop.
You can see and coordinate every agent in every workspace through your tools. You speak with the person; keep every spoken reply to one or two short, natural sentences, like a capable colleague on a call. Never read out ids or JSON.

How you work:
1. When the person asks for work, first call agent_panels with operation list to see the agents, their workspace and state.
2. Post the steps you intend to take with agent_panels operation plan (a short title per step, a detail naming the agent, status pending or running). Keep it up to date: call plan again whenever something changes, with each step pending, running, done or failed.
3. Send each task to the right agent with agent_panels operation send. Write the task completely, as the person would. Only send to an agent whose state is idle; never to one that is working or needs_input.
4. Agents take several seconds. After sending, say briefly what you started, then use the wait tool (5 to 8 seconds) and call list again. Read an agent's output with operation read when you need to know what it did. Do not poll more than once per wait.
5. If an agent's state is needs_input, it is asking the person something. You cannot answer for them: tell the person which agent is asking and what, in one sentence.
6. When everything that can finish has finished, post a recap with agent_panels operation note (title and a short markdown body), update the plan, and tell the person where things stand and what, if anything, needs them.
Never end your turn while an agent you started is still working: call wait, then list again, and only stop after you have posted the recap with note.
Be accurate: only say something is done if the agent's state and output show it."""


def ms():
    return int(time.time() * 1000)


def bar(line):
    if CMD:
        with open(CMD, "a") as f:
            f.write(line + "\n")


def event(kind, **data):
    with open(os.path.join(OUT, "events.jsonl"), "a") as f:
        f.write(json.dumps({"t": ms(), "kind": kind, **data}) + "\n")


def say(text, style="0"):
    print(f"\033[{style}m{text}\033[0m", flush=True)


class Mcp:
    def __init__(self, binary):
        self.p = subprocess.Popen([binary, "--browser-mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, text=True)
        self.n = 0
        self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                    "clientInfo": {"name": "horizon-voice", "version": "0"}})
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        self.p.stdin.flush()

    def request(self, method, params):
        self.n += 1
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params}) + "\n")
        self.p.stdin.flush()
        while True:
            line = self.p.stdout.readline()
            if not line:
                return {"error": "the Horizon MCP server ended"}
            message = json.loads(line)
            if message.get("id") == self.n:
                return message

    def tools(self):
        return self.request("tools/list", {})["result"]["tools"]

    def call(self, name, arguments):
        reply = self.request("tools/call", {"name": name, "arguments": arguments})
        result = reply.get("result") or {}
        value = result.get("structuredContent")
        if value is None:
            value = [c.get("text") for c in result.get("content", []) if c.get("type") == "text"] or reply.get("error")
        return json.dumps(value)[:6000]


def inline_refs(schema, defs, depth=0):
    """OpenAI function schemas do not take $ref well; inline them."""
    if isinstance(schema, dict):
        if depth > 12:
            return {}
        if "$ref" in schema:
            target = defs.get(schema["$ref"].split("/")[-1], {})
            return inline_refs(target, defs, depth + 1)
        drop = () if "type" not in schema else ("$schema", "$defs")
        return {k: inline_refs(v, defs, depth + 1) for k, v in schema.items() if k not in drop}
    if isinstance(schema, list):
        return [inline_refs(v, defs, depth + 1) for v in schema]
    return schema


def realtime_tools(mcp_tools):
    out, names = [], []
    for tool in mcp_tools:
        if tool["name"] not in ALLOWED:
            continue
        schema = HAND.get(tool["name"]) or tool.get("inputSchema") or {"type": "object", "properties": {}}
        parameters = inline_refs(schema, schema.get("$defs", {})) or {"type": "object", "properties": {}}
        out.append({"type": "function", "name": tool["name"], "description": tool["description"][:1500],
                    "parameters": parameters})
        names.append(tool["name"])
    out.append({"type": "function", "name": "wait", "description": "Pause for a few seconds so agents can work, then continue.",
                "parameters": {"type": "object", "properties": {"seconds": {"type": "number", "description": "1 to 10"}},
                               "required": ["seconds"]}})
    return out, names


def rms_levels(pcm, step_ms=50):
    step = RATE * 2 * step_ms // 1000
    levels = []
    for i in range(0, len(pcm) - step + 1, step):
        samples = struct.unpack("<%dh" % (step // 2), pcm[i:i + step])
        levels.append(math.sqrt(sum(s * s for s in samples) / len(samples)) / 32768)
    peak = max(levels) or 1
    return [min(1.0, (v / peak) ** 0.7) for v in levels]


class Timeline:
    """Everything the assistant says, placed on a clock, so it can be heard and shown in step."""

    def __init__(self):
        self.queue = []
        self.clock = 0.0
        self.chunks = []  # (start epoch seconds, pcm bytes)
        self.count = 0

    def add_audio(self, pcm):
        start = max(self.clock, time.time())
        self.chunks.append((start, pcm))
        for index, level in enumerate(rms_levels(pcm)):
            self.schedule(start + index * 0.05, f"level {level:.2f}")
        self.clock = start + len(pcm) / (RATE * 2)
        return start

    def schedule(self, at, line):
        self.count += 1
        heapq.heappush(self.queue, (at, self.count, line))

    async def pump(self):
        while True:
            if self.queue and self.queue[0][0] <= time.time():
                _, _, line = heapq.heappop(self.queue)
                bar(line)
            else:
                await asyncio.sleep(0.01)

    def save(self, t0):
        if not self.chunks:
            return
        end = max(s + len(p) / (RATE * 2) for s, p in self.chunks)
        total = bytearray(int((end - t0) * RATE) * 2)
        for start, pcm in self.chunks:
            offset = int((start - t0) * RATE) * 2
            total[offset:offset + len(pcm)] = pcm
        with wave.open(os.path.join(OUT, "assistant.wav"), "wb") as w:
            w.setnchannels(1); w.setsampwidth(2); w.setframerate(RATE); w.writeframes(bytes(total))


async def wait_for_go():
    """Listen until the story says go, so a recording can start at a known moment."""
    start = os.path.getsize(CMD) if CMD and os.path.exists(CMD) else 0
    while True:
        await asyncio.sleep(0.2)
        if CMD and os.path.getsize(CMD) > start:
            with open(CMD) as f:
                f.seek(start)
                if "voice_go" in f.read():
                    return


async def main():
    mcp = Mcp(os.environ["HORIZON_MCP_BIN"])
    tools, names = realtime_tools(mcp.tools())
    say("\033[1;36m Assistant\033[0m  live voice (" + MODEL + ")", "0")
    say(f" MCP: horizon-browser, {len(mcp.tools())} tools; this session may use: " + ", ".join(names + ["wait"]), "2")
    say(" Listening for the person...\n", "2")
    await wait_for_go()

    timeline = Timeline()
    pump = asyncio.create_task(timeline.pump())
    url = f"wss://api.openai.com/v1/realtime?model={MODEL}"
    async with websockets.connect(url, additional_headers={"Authorization": f"Bearer {KEY}"}, max_size=None) as ws:
        await ws.recv()
        await ws.send(json.dumps({"type": "session.update", "session": {
            "type": "realtime", "model": MODEL, "output_modalities": ["audio"], "instructions": INSTRUCTIONS,
            "tools": tools, "tool_choice": "auto",
            "audio": {"input": {"format": {"type": "audio/pcm", "rate": RATE},
                                "transcription": {"model": "gpt-4o-mini-transcribe"}, "turn_detection": None},
                      "output": {"format": {"type": "audio/pcm", "rate": RATE}, "voice": VOICE}}}}))
        t0 = time.time()

        # The person speaks: the recording streams in real time, and the bar shows it as dictation.
        pcm = open(REQUEST_PCM, "rb").read()
        seconds = len(pcm) / (RATE * 2)
        user_start = time.time()
        event("user_audio", start=int(user_start * 1000), seconds=seconds)
        bar(f"say {int(seconds * 1000)} {REQUEST_TEXT}")
        say("You", "1;37")
        say(" " + REQUEST_TEXT + "\n", "0")
        levels = rms_levels(pcm)
        chunk = RATE * 2 * 50 // 1000
        for index in range(0, len(pcm), chunk):
            await ws.send(json.dumps({"type": "input_audio_buffer.append",
                                      "audio": base64.b64encode(pcm[index:index + chunk]).decode()}))
            bar(f"level {levels[min(index // chunk, len(levels) - 1)]:.2f}")
            await asyncio.sleep(0.05)
        await ws.send(json.dumps({"type": "input_audio_buffer.commit"}))
        await ws.send(json.dumps({"type": "response.create"}))
        bar("enter")

        pending, texts, started = [], {}, {}
        noted, nudges = False, 0
        deadline = time.time() + 240
        quiet_since = None
        while time.time() < deadline:
            try:
                message = json.loads(await asyncio.wait_for(ws.recv(), 20))
            except asyncio.TimeoutError:
                if timeline.clock < time.time() and not pending:
                    break
                continue
            kind = message["type"]
            if kind == "response.output_audio.delta":
                start = timeline.add_audio(base64.b64decode(message["delta"]))
                started.setdefault(message["response_id"], start)
            elif kind == "response.output_audio_transcript.done":
                text = message.get("transcript", "").strip()
                when = started.get(message["response_id"], time.time())
                timeline.schedule(when, "speaking " + text)
                say("Assistant", "1;36")
                say(" " + text + "\n", "0")
                event("assistant_said", start=int(when * 1000), text=text)
            elif kind == "response.output_item.done" and message["item"].get("type") == "function_call":
                pending.append(message["item"])
            elif kind == "response.done":
                if pending:
                    for call in pending:
                        name, raw = call["name"], call.get("arguments") or "{}"
                        args = json.loads(raw)
                        loop = asyncio.get_running_loop()
                        if name == "wait":
                            say(f" ⏳ waiting {args.get('seconds', 5)}s", "2")
                            await asyncio.sleep(max(1, min(10, float(args.get("seconds", 5)))))
                            output = "ok"
                        elif name in ALLOWED and (ALLOWED[name] is None or args.get("operation") in ALLOWED[name]):
                            say(f" ● {name} {args.get('operation', '')}", "33")
                            event("tool", name=name, args=args)
                            noted = noted or (name == "agent_panels" and args.get("operation") == "note")
                            output = await loop.run_in_executor(None, mcp.call, name, args)
                        else:
                            output = json.dumps({"error": "not available in this session"})
                        await ws.send(json.dumps({"type": "conversation.item.create", "item": {
                            "type": "function_call_output", "call_id": call["call_id"], "output": output}}))
                    pending = []
                    await ws.send(json.dumps({"type": "response.create"}))
                elif not noted and nudges < 5 and time.time() < deadline - 20:
                    # Stopped before the recap: ask it to carry on, as a person would.
                    nudges += 1
                    event("nudge", n=nudges)
                    await asyncio.sleep(max(0, timeline.clock - time.time()))
                    await ws.send(json.dumps({"type": "conversation.item.create", "item": {
                        "type": "message", "role": "user", "content": [{"type": "input_text", "text":
                        "Keep going: wait for the agents to finish, check on them, and post the recap when they have."}]}}))
                    await ws.send(json.dumps({"type": "response.create"}))
                else:
                    event("response_done", t=ms())
                    # Done when nothing more is coming and what was said has been played.
                    if timeline.clock < time.time() + 0.5:
                        quiet_since = time.time()
                        await asyncio.sleep(3)
                        if timeline.clock < time.time():
                            break
            elif kind == "error":
                say(f" error: {message}", "31")
                event("error", message=message)
                break
        await asyncio.sleep(max(0, timeline.clock - time.time()) + 1)
        timeline.save(user_start)
        event("saved", t0=int(user_start * 1000))
    pump.cancel()
    say("\n Session ended.", "2")
    while True:
        await asyncio.sleep(3600)


asyncio.run(main())
