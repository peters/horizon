---
name: horizon-speech
description: Check that a microphone is usable for Horizon dictation and report which layer is at fault — no signal, bad level, or a genuine model/accent limit. Use when dictation produces wrong, unrelated, or empty text.
---

# Horizon speech input check

Run this when a user says dictation "types the wrong thing", produces text they
never said, or produces nothing.

## Read this first

Whisper-family models **hallucinate fluent, plausible text when fed silence**.
Given a digitally silent recording, NB-Whisper will confidently emit something
like `Esther Smith, forfatter` — grammatical, idiomatic, and completely
unrelated to the user. The output looks like a model, accent, or dialect
problem. It is almost always a capture problem.

So: **measure the capture level before touching models, languages, or config.**
Never conclude "the model is bad at your accent" until step 3 shows real signal.

A user cannot tell these apart from the transcript alone. That is the whole
reason this skill exists.

## 1. Compare configured device against reality

The Horizon config lives at `~/.horizon/config.yaml` (Linux/macOS) or
`%USERPROFILE%\.horizon\config.yaml` (Windows). Read `features.speech`:

- `input_device: ""` means the system default input is used.
- A non-empty name is matched case-insensitively: exact, then substring, then a
  normalized identity key.

**If the configured name matches no present device, Horizon falls back to the
system default and only logs a warning.** The user never sees it. A config
naming a microphone that is no longer plugged in therefore looks like it works.
Check the name against the live device list before anything else.

List input devices:

- Linux (PipeWire/PulseAudio): `wpctl status`, or `pactl list short sources`
- Linux (ALSA only): `arecord -l`
- macOS: `system_profiler SPAudioDataType`, or
  `ffmpeg -f avfoundation -list_devices true -i ""`
- Windows: `ffmpeg -list_devices true -f dshow -i dummy`, or
  `Get-PnpDevice -Class AudioEndpoint` in PowerShell

Also confirm the device is not a **Bluetooth headset in HSP/HFP mode**. That
profile is narrowband and heavily compressed; it degrades accuracy badly. Prefer
a wired or USB microphone, or force the A2DP-era high-quality input if the stack
offers one.

## 2. Record a short sample

Ask the user to speak normally for about six seconds. Record mono at 16 kHz —
that is what the models consume.

- Linux: `arecord -f S16_LE -r 16000 -c 1 -d 6 sample.wav`
  (or `pw-record --rate 16000 --channels 1 sample.wav`)
- macOS: `ffmpeg -f avfoundation -i ":<device-index>" -ar 16000 -ac 1 -t 6 sample.wav`
- Windows: `ffmpeg -f dshow -i audio="<device name>" -ar 16000 -ac 1 -t 6 sample.wav`

Tell the user **when recording starts**. If you launch the recorder as a
background task, add a visible countdown first — otherwise the window elapses
while they are still reading your message, and you will measure an empty room
and misdiagnose it as a dead microphone.

## 3. Measure the level

Run the bundled analyzer (Python 3, standard library only):

```
python3 level.py sample.wav
```

It prints peak, RMS, clipped-sample count and a verdict:

| Reading | Meaning | Fix |
| --- | --- | --- |
| `peak` ~0 | Capture muted or no device | Unmute capture; check the device is selected and present |
| `peak` < 200 | Effectively no signal | Wrong input selected, or nothing connected |
| clipped samples > 20 | Too hot — distortion | Lower gain; turn microphone boost **off** first |
| RMS < 2% | Too quiet | Raise gain, or move closer |
| RMS 5–25% | Ideal for ASR | Nothing to do |

## 4. Transcribe the same file

Run the profile's model against `sample.wav` and compare with what the user
actually said. Horizon's models are transcribe.cpp GGUFs; if a `transcribe-cli`
build is available:

```
transcribe-cli -m <model>.gguf -l <lang> sample.wav
```

Otherwise have the user dictate the same sentence in Horizon and compare.

## 5. Verdict

Combine steps 3 and 4 — this is the part the user cannot do alone:

- **No signal + fluent unrelated text** → capture is dead. The text is
  hallucinated. Fix the microphone; the model is fine.
- **Good level + empty transcript** → real audio, no speech detected. Usually
  room noise only, or the user did not speak inside the window.
- **Clipping + roughly correct text** → working but distorted. Reduce gain;
  accuracy will improve, especially on consonants.
- **Good level + mostly correct text with wrong proper nouns or English
  technical terms** → this is the model's genuine limit, not the microphone.
  Suggest an initial prompt to bias vocabulary, or a profile whose model suits
  the language better. Only at this point is accent or dialect worth discussing.

Report which of these it is explicitly. "Your microphone is fine, the model
mis-heard a term" and "your microphone captured nothing" are opposite fixes and
look identical in the transcript.

## Adjusting gain

Turn microphone **boost** off before raising capture volume — boost amplifies
the microphone's own noise floor as much as the voice.

- Linux (PipeWire): `wpctl set-volume <source-id> <0.0-1.0>`. This maps onto the
  ALSA hardware control. Boost lives in `amixer -c <card>`, e.g.
  `amixer -c <card> sset 'Mic Boost' 0`.
- macOS: System Settings → Sound → Input → Input volume.
- Windows: Settings → System → Sound → Input → device properties. Also disable
  "Audio Enhancements", which can gate or suppress speech.

Re-run steps 2–3 after each change. Gain that sounds fine to a human can still
be clipping.
