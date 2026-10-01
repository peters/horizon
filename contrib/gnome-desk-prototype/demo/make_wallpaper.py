#!/usr/bin/env python3
"""Writes the demo wallpaper: python3 make_wallpaper.py out.png (needs numpy and Pillow)."""
import sys
import numpy as np
from PIL import Image
W, H = 1920, 1080
y = np.linspace(0, 1, H)[:, None, None]
bg = np.array([8, 12, 24.0]) * (1 - y) + np.array([16, 26, 52.0]) * y
bg = np.broadcast_to(bg, (H, W, 3)).copy()
xx, yy = np.meshgrid(np.linspace(0, W, W), np.linspace(0, H, H))
for cx, cy, r, col, k in [(300, 200, 700, (106, 144, 255), 0.20), (1650, 900, 800, (140, 90, 240), 0.16), (1000, 500, 900, (60, 120, 200), 0.07)]:
    g = np.exp(-(((xx - cx) ** 2 + (yy - cy) ** 2) / (2 * r * r)))[..., None] * k
    bg = bg * (1 - g) + np.array(col, float) * g
Image.fromarray(bg.clip(0, 255).astype(np.uint8)).save(sys.argv[1])
