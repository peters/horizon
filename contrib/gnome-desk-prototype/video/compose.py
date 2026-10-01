#!/usr/bin/env python3
"""Compose the marketing video: intro, the recorded desktop with camera moves and captions, outro."""
import bisect, json, math, os, subprocess, sys
import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

S = os.environ["DEMO_DIR"]
plan = json.load(open(S + "/video/plan.json"))
FPS, W, H = 30, 1920, 1080
INTRO, D, TOTAL = plan["intro"], plan["capture"], plan["total"]
OUTRO_AT = INTRO + D - 0.6
rel = plan["rel"]
FONT = "/usr/share/fonts/truetype/lato/Lato-%s.ttf"
font = lambda weight, size: ImageFont.truetype(FONT % weight, size)
ACCENT, INK, SOFT = (106, 144, 255), (232, 238, 250), (160, 174, 204)

frames = sorted(int(f[:-4]) for f in os.listdir(S + "/frames") if f.endswith(".png"))
t0 = frames[0]
frame_times = [(f - t0) / 1000 for f in frames]
cache = {}
def capture_at(tc):
    index = max(0, bisect.bisect_right(frame_times, tc) - 1)
    if index not in cache:
        if len(cache) > 6:
            cache.pop(next(iter(cache)))
        cache[index] = Image.open(f"{S}/frames/{frames[index]}.png").convert("RGB")
    return cache[index]

def ease(x): x = min(1.0, max(0.0, x)); return x * x * (3 - 2 * x)
def lerp(a, b, x): return a + (b - a) * x

# ---- backdrop -----------------------------------------------------------------------
def make_backdrop():
    y = np.linspace(0, 1, H)[:, None, None]
    top, bottom = np.array([6, 9, 17.0]), np.array([13, 21, 44.0])
    bg = top * (1 - y) + bottom * y
    bg = np.broadcast_to(bg, (H, W, 3)).copy()
    xx, yy = np.meshgrid(np.linspace(0, W, W), np.linspace(0, H, H))
    for cx, cy, r, color, k in [(0.12 * W, 0.15 * H, 780, ACCENT, 0.16), (0.9 * W, 0.92 * H, 900, (150, 90, 255), 0.11)]:
        g = np.exp(-(((xx - cx) ** 2 + (yy - cy) ** 2) / (2 * r * r)))[..., None] * k
        bg = bg * (1 - g) + np.array(color, float) * g
    return Image.fromarray(bg.clip(0, 255).astype(np.uint8))
BACK = make_backdrop()

AREA = (1920, 1080)
AX, AY = 0, 0
BACK_WITH_SHADOW = BACK

# ---- the Horizon assistant mark, drawn the way the app draws it ------------------------
def mark(size):
    k = 4; s = size * k
    im = Image.new("RGBA", (s, s), (0, 0, 0, 0)); d = ImageDraw.Draw(im)
    base = (12, 17, 28)
    tint = tuple(int(base[i] * 0.8 + ACCENT[i] * 0.2) for i in range(3))
    d.rounded_rectangle((0, 0, s - 1, s - 1), int(s * 0.3), fill=tint + (255,), outline=ACCENT + (110,), width=max(2, s // 80))
    at = lambda x, y: (x * s, y * s)
    hy = 0.66
    d.line([at(0.2, hy), at(0.8, hy)], fill=ACCENT + (180,), width=max(3, int(s * 0.045)))
    r = s * 0.2; cx, cy = at(0.5, hy)
    d.pieslice((cx - r, cy - r, cx + r, cy + r), 180, 360, fill=ACCENT + (255,))
    arc = s * 0.37
    for deg in (215, 270, 325):
        a = math.radians(deg)
        n = (cx + math.cos(a) * arc, cy + math.sin(a) * arc)
        d.line([(cx + math.cos(a) * (r + s * 0.03), cy + math.sin(a) * (r + s * 0.03)), n], fill=ACCENT + (80,), width=max(2, s // 90))
        rr = s * 0.055
        d.ellipse((n[0] - rr, n[1] - rr, n[0] + rr, n[1] + rr), fill=(240, 244, 255, 255))
    return im.resize((size, size), Image.LANCZOS)

def glow(im, radius, strength):
    g = Image.new("RGBA", im.size, (0, 0, 0, 0))
    g.paste(Image.new("RGBA", im.size, ACCENT + (int(255 * strength),)), mask=im.split()[3])
    return g.filter(ImageFilter.GaussianBlur(radius))

def text_layer(text, fnt, color, anchor_center=True):
    box = fnt.getbbox(text)
    w, h = box[2] + 8, box[3] + 12
    im = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    ImageDraw.Draw(im).text((4, 2), text, font=fnt, fill=color + (255,))
    return im

def paste_alpha(base, layer, cx, cy, alpha=1.0, scale=1.0):
    if alpha <= 0.003:
        return
    if scale != 1.0:
        layer = layer.resize((max(1, int(layer.width * scale)), max(1, int(layer.height * scale))), Image.LANCZOS)
    if alpha < 1.0:
        a = layer.split()[3].point(lambda v: int(v * alpha))
        layer = layer.copy(); layer.putalpha(a)
    base.paste(layer, (int(cx - layer.width / 2), int(cy - layer.height / 2)), layer)

BIG_MARK = mark(220); BIG_GLOW = glow(BIG_MARK, 40, 0.5)
SMALL_MARK = mark(96)
T_TITLE = text_layer("Horizon", font("Bold", 120), INK)
T_SUB = text_layer("The infinite canvas, on every desktop.", font("Light", 42), SOFT)
T_OUT1 = text_layer("One assistant.", font("Bold", 100), INK)
T_OUT2 = text_layer("Every workspace.", font("Bold", 100), ACCENT)
T_OUT3 = text_layer("Prototype running on Ubuntu 26.04  ·  GNOME on Wayland", font("Regular", 30), SOFT)
T_OUT4 = text_layer("Scripted stand-in agents and a synthetic voice. No real model calls.", font("Regular", 24), (110, 124, 156))

def intro_frame(t):
    img = BACK.copy()
    a = ease((t - 0.25) / 0.9)
    paste_alpha(img, BIG_GLOW, W / 2, 400, a * 0.8, 1.0 + 0.04 * math.sin(t * 1.6))
    paste_alpha(img, BIG_MARK, W / 2, 400, a, lerp(0.88, 1.0, a))
    paste_alpha(img, T_TITLE, W / 2, 640 + 14 * (1 - ease((t - 0.9) / 0.7)), ease((t - 0.9) / 0.7))
    paste_alpha(img, T_SUB, W / 2, 760 + 14 * (1 - ease((t - 1.5) / 0.7)), ease((t - 1.5) / 0.7))
    return img

def outro_frame(t):
    img = BACK.copy()
    paste_alpha(img, SMALL_MARK, W / 2, 250, ease(t / 0.6))
    paste_alpha(img, T_OUT1, W / 2, 440 + 16 * (1 - ease((t - 0.3) / 0.7)), ease((t - 0.3) / 0.7))
    paste_alpha(img, T_OUT2, W / 2, 560 + 16 * (1 - ease((t - 0.8) / 0.7)), ease((t - 0.8) / 0.7))
    paste_alpha(img, T_OUT3, W / 2, 730, ease((t - 1.6) / 0.8))
    paste_alpha(img, T_OUT4, W / 2, 985, ease((t - 2.2) / 0.8) * 0.9)
    return img

# ---- captions and camera --------------------------------------------------------------
m = plan["marks"]
g = rel["go"]
CAPTIONS = [
    (m["start"] + 0.3, m["tour"] - 0.2, "Panels are normal windows on your desktop.", "No canvas. No frame. Just your apps."),
    (m["tour"] + 0.1, m["native"] - 0.2, "A browser, an agent and a shell, side by side.", None),
    (m["native"] + 0.1, m["keys"] - 0.2, "Native apps in a VNC viewer panel.", None),
    (m["keys"] + 0.1, m["overview"] - 0.3, "Ctrl + Alt + arrows work too.", "A real GNOME desktop."),
    (m["overview"] + 0.2, m["overview_close"] - 0.1, "Twenty real workspaces.", "GNOME's own overview shows every one."),
    (m["overview_close"] + 0.2, m["voice"] - 0.1, "Twenty workspaces, one small bar.", None),
    (m["voice"] + 0.1, m["enter"] + 0.3, "Just say what you want.", None),
    (m["enter"] + 0.9, m["work2"] - 0.4, "One request. Three agents. Three workspaces.", None),
    (m["work2"] + 0.1, m["work3"] - 0.3, "Each agent works in its own workspace.", None),
    (m["work3"] + 0.2, m["work1"] - 0.2, "Watch the minimap, not the noise.", None),
    (m["work1"] + 0.3, rel["note"] - 0.4, "The bar follows you to every workspace.", None),
    (rel["note"] - 0.1, m["expA"] - 0.3, "A recap, not noise: two done, one needs you.", None),
    (m["expA"] + 0.1, m["expB"] - 0.1, "Expand into the whole conversation.", "A  ·  Sheet"),
    (m["expB"] + 0.1, m["expC"] - 0.1, "Expand into the whole conversation.", "B  ·  Split"),
    (m["expC"] + 0.1, m["collapse"] - 0.1, "Expand into the whole conversation.", "C  ·  Stage"),
    (m["move"] + 0.2, m["moved"] - 0.2, "Drag a window to another desktop.", "The panel follows."),
    (m["moved"] + 0.1, m["scope"] - 0.3, "Same panel, new workspace.", None),
    (m["scope"] - 0.2, m["end"], "Scope it to one workspace, or all of them.", None),
]
caption_cache = {}
def caption_layer(main, sub):
    key = (main, sub)
    if key in caption_cache:
        return caption_cache[key]
    fm, fs = font("Semibold", 46), font("Medium", 28)
    mw = fm.getbbox(main)[2]
    sw = fs.getbbox(sub)[2] if sub else 0
    width = max(mw, sw) + 90
    height = 92 + (46 if sub else 0)
    im = Image.new("RGBA", (width + 40, height + 40), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    pill = Image.new("RGBA", im.size, (0, 0, 0, 0))
    ImageDraw.Draw(pill).rounded_rectangle((20, 20, 20 + width, 20 + height), 34, fill=(8, 12, 22, 238), outline=(120, 150, 230, 90), width=2)
    pill = pill.filter(ImageFilter.GaussianBlur(0.6))
    im.alpha_composite(pill)
    d = ImageDraw.Draw(im)
    d.text((20 + width / 2, 20 + 46), main, font=fm, fill=INK + (255,), anchor="mm")
    if sub:
        d.text((20 + width / 2, 20 + 46 + 44), sub, font=fs, fill=ACCENT + (255,), anchor="mm")
    caption_cache[key] = im
    return im

def caption_alpha(tc, start, end):
    return ease((tc - start) / 0.35) * ease((end - tc) / 0.35)

# camera keyframes: (capture time, zoom, centre x, centre y) in desktop pixels
CAMERA = [
    (0.0, 1.0, 960, 540), (m["voice"] - 0.8, 1.0, 960, 540), (m["voice"] + 0.0, 1.5, 960, 900),
    (m["enter"] + 0.6, 1.5, 960, 900), (m["enter"] + 1.8, 1.0, 960, 540),
    (rel["note"] - 1.2, 1.0, 960, 540), (rel["note"] - 0.2, 1.45, 960, 860), (m["expA"] - 0.8, 1.45, 960, 860),
    (m["expA"] - 0.1, 1.0, 960, 540), (m["scope"] - 0.4, 1.0, 960, 540), (m["scope"] + 0.8, 1.45, 960, 900),
    (m["end"] - 0.6, 1.45, 960, 900), (m["end"] + 0.6, 1.0, 960, 540), (D + 5, 1.0, 960, 540),
]
def camera(tc):
    for i in range(len(CAMERA) - 1):
        a, b = CAMERA[i], CAMERA[i + 1]
        if a[0] <= tc <= b[0]:
            x = ease((tc - a[0]) / max(b[0] - a[0], 1e-6))
            return tuple(lerp(a[k], b[k], x) for k in (1, 2, 3))
    return CAMERA[-1][1:]

def capture_frame(tc, t):
    src = capture_at(tc)
    z, cx, cy = camera(tc)
    z = max(1.0, z * (1 + 0.008 * math.sin(t * 0.7)) + 0.008)
    bw, bh = 1920 / z, 1080 / z
    x0 = min(max(cx - bw / 2, 0), 1920 - bw); y0 = min(max(cy - bh / 2, 0), 1080 - bh)
    view = src.resize(AREA, Image.BICUBIC, box=(x0, y0, x0 + bw, y0 + bh))
    img = view.copy()
    for start, end, main, sub in CAPTIONS:
        a = caption_alpha(tc, start, end)
        if a > 0.003:
            layer = caption_layer(main, sub)
            lift = 14 * (1 - ease((tc - start) / 0.4))
            paste_alpha(img, layer, W / 2, 170 + lift, a)
    return img

def frame_at(t):
    if t < INTRO - 0.6:
        return intro_frame(t)
    if t >= OUTRO_AT + 0.6:
        return outro_frame(t - OUTRO_AT)
    tc = t - INTRO
    cap = capture_frame(max(0.0, tc), t)
    if t < INTRO:
        return Image.blend(intro_frame(t), cap, ease((t - (INTRO - 0.6)) / 0.6))
    if t > OUTRO_AT:
        return Image.blend(cap, outro_frame(t - OUTRO_AT), ease((t - OUTRO_AT) / 0.6))
    return cap

if __name__ == "__main__":
    out = sys.argv[1]
    limit = float(sys.argv[2]) if len(sys.argv) > 2 else TOTAL
    start = float(sys.argv[3]) if len(sys.argv) > 3 else 0.0
    ff = subprocess.Popen(
        ["ffmpeg", "-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H}", "-r", str(FPS),
         "-i", "-", "-ss", str(start), "-i", S + "/video/mix.wav", "-t", str(limit - start),
         "-c:v", "libx264", "-preset", "medium", "-crf", "19", "-pix_fmt", "yuv420p",
         "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart", "-shortest", out],
        stdin=subprocess.PIPE)
    total_frames = int((limit - start) * FPS)
    for i in range(total_frames):
        t = start + i / FPS
        ff.stdin.write(np.asarray(frame_at(t)).tobytes())
        if i % 150 == 0:
            print(f"{i}/{total_frames}", flush=True)
    ff.stdin.close(); ff.wait()
    print("done", out)
