#!/usr/bin/env python3
"""Turn the take's event logs into video times (plan.json)."""
import json, os
S = os.environ["DEMO_DIR"]
INTRO, OUTRO = 4.0, 6.0
frames = sorted(int(f[:-4]) for f in os.listdir(S + "/frames") if f.endswith(".png"))
t0, t1 = frames[0], frames[-1]
D = (t1 - t0) / 1000
at = lambda ms: INTRO + (ms - t0) / 1000
events = {"go": [], "done": [], "alert": [], "expand": [], "mic": []}
marks, rel = {}, {}
last_done = 0
for line in open(S + "/tools/demo/cmd.txt.log"):
    ms, rest = line.split(" ", 1)
    ms = int(ms); rest = rest.strip()
    word = rest.split(" ")[0]
    if word in ("go", "key", "mini", "page"): events["go"].append(at(ms))
    elif word in ("expand", "overview", "hub"): events["expand"].append(at(ms))
    elif word == "mark": marks[rest.split()[1]] = (ms - t0) / 1000
    elif word == "say": events["mic"].append(at(ms) - 0.1); rel["say"] = (ms - t0) / 1000
    elif word == "enter": rel["enter"] = (ms - t0) / 1000
    elif word == "answer": events["done"].append(at(ms))
    elif rest.startswith("event plan"):
        done = int(rest.split()[2])
        if done > last_done:
            events["done"].append(at(ms)); last_done = done
    elif rest.startswith("event note"): events["alert"].append(at(ms)); rel["note"] = (ms - t0) / 1000
voice = [json.loads(l) for l in open(S + "/voice-out/events.jsonl")]
user = next(e for e in voice if e["kind"] == "user_audio")
saved = next(e for e in voice if e["kind"] == "saved")
voices = [
    {"file": S + "/audio/request-user.wav", "at": at(user["start"]), "peak": 0.7},
    {"file": S + "/voice-out/assistant.wav", "at": at(saved["t0"]), "peak": 0.72},
]
plan = {
    "intro": INTRO, "outro": OUTRO, "capture": D, "total": INTRO + D + OUTRO - 0.6,
    "events": events, "rel": rel, "voices": voices, "voice_at": at(user["start"]),
    "intro_swell": 0.2, "intro_hit": 1.5, "outro_hit": INTRO + D - 0.2,
    "t0": t0, "marks": marks,
}
json.dump(plan, open(S + "/video/plan.json", "w"), indent=1)
print(json.dumps({k: v for k, v in plan.items() if k != "events"}, indent=1)); print(events)
