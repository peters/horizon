#!/usr/bin/env python3
"""Grab frames of the private desktop as fast as the shell allows until frames/STOP appears."""
import os, subprocess, sys, time
S = os.environ["DEMO_DIR"]
out = S + "/frames"
os.makedirs(out, exist_ok=True)
for name in os.listdir(out):
    os.remove(os.path.join(out, name))
env = dict(os.environ, DBUS_SESSION_BUS_ADDRESS=open(os.environ["DEMO_DIR"] + "/gnd/bus.addr").read().strip(),
           HOME=os.environ["DEMO_DIR"] + "/gnd/home", XDG_RUNTIME_DIR=os.environ["DEMO_DIR"] + "/gnd/rt")
count = 0
while not os.path.exists(out + "/STOP"):
    stamp = int(time.time() * 1000)
    subprocess.run(["gdbus", "call", "--session", "--dest", "org.gnome.Shell.Screenshot",
                    "--object-path", "/org/gnome/Shell/Screenshot", "--method",
                    "org.gnome.Shell.Screenshot.Screenshot", "false", "false", f"{out}/{stamp}.png"],
                   env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    count += 1
print("frames", count)
