#!/usr/bin/env python3
"""Generate the person's spoken request with OpenAI text-to-speech (outside Horizon): 24 kHz mono 16-bit PCM + wav."""
import json, os, sys, urllib.request, wave
S = os.environ.get("DEMO_DIR") or os.path.dirname(os.path.abspath(__file__))
key = open(os.environ.get("OPENAI_KEY_FILE") or os.path.expanduser("~/.cache/horizon-tools/openai.key")).read().strip()
voice = sys.argv[1] if len(sys.argv) > 1 else "coral"
text = open(S + "/audio/request-user.txt").read().strip()
body = {"model": "gpt-4o-mini-tts", "voice": voice, "input": text, "response_format": "pcm",
        "instructions": "A cheerful, warm, upbeat young woman starting her morning, speaking naturally and conversationally, "
                        "with a smile in her voice, a lively pace and natural pauses. Not robotic, not announcer-like."}
req = urllib.request.Request("https://api.openai.com/v1/audio/speech", data=json.dumps(body).encode(),
                             headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"})
pcm = urllib.request.urlopen(req, timeout=120).read()
open(S + "/audio/request-user.pcm", "wb").write(pcm)
with wave.open(S + "/audio/request-user.wav", "wb") as w:
    w.setnchannels(1); w.setsampwidth(2); w.setframerate(24000); w.writeframes(pcm)
print(voice, len(pcm) / 48000, "s")
