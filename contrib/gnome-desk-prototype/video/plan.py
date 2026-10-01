#!/usr/bin/env python3
"""Turn the take's event log into video times (plan.json)."""
import json, os
S = os.environ["DEMO_DIR"]
INTRO, OUTRO = 4.0, 6.0
frames = sorted(int(f[:-4]) for f in os.listdir(S + "/frames") if f.endswith(".png"))
t0, t1 = frames[0], frames[-1]
D = (t1 - t0) / 1000
at = lambda ms: INTRO + (ms - t0) / 1000
events = {"go": [], "done": [], "alert": [], "expand": [], "mic": []}
rel = {}
last_done = 0
for line in open(S + "/tools/demo/cmd.txt.log"):
    ms, rest = line.split(" ", 1)
    ms = int(ms); rest = rest.strip()
    if rest.startswith("go "): events["go"].append(at(ms)); rel.setdefault("go", []).append((ms - t0) / 1000)
    elif rest.startswith("expand "): events["expand"].append(at(ms)); rel.setdefault("expand", []).append((ms - t0) / 1000)
    elif rest.startswith("say "): events["mic"].append(at(ms) - 0.15); rel["say"] = (ms - t0) / 1000
    elif rest == "enter": rel["enter"] = (ms - t0) / 1000
    elif rest.startswith("collapse"): rel["collapse"] = (ms - t0) / 1000
    elif rest.startswith("scope Cloud"): rel["scope_cloud"] = (ms - t0) / 1000
    elif rest.startswith("scope all"): rel["scope_all"] = (ms - t0) / 1000
    elif rest.startswith("event plan"):
        done = int(rest.split()[2])
        if done > last_done:
            events["done"].append(at(ms)); last_done = done
    elif rest.startswith("event note"): events["alert"].append(at(ms)); rel["note"] = (ms - t0) / 1000
plan = {
    "intro": INTRO, "outro": OUTRO, "capture": D, "total": INTRO + D + OUTRO - 0.6,
    "events": events, "rel": rel,
    "voice_at": INTRO + rel["say"] + 0.12,
    "intro_swell": 0.2, "intro_hit": 1.5, "outro_hit": INTRO + D - 0.2,
    "t0": t0,
}
json.dump(plan, open(S + "/video/plan.json", "w"), indent=1)
print(json.dumps({k: v for k, v in plan.items() if k != "events"}, indent=1)); print(events)
