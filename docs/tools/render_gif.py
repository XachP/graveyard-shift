#!/usr/bin/env python3
"""Renders the ANSI output of `cargo run --example haunted_payroll` into docs/img/demo.gif.

    GS_FAST=1 cargo run -q --example haunted_payroll 2>/dev/null > /tmp/demo_out.txt
    python3 docs/tools/render_gif.py /tmp/demo_out.txt docs/img/demo.gif
"""
import re, sys
from PIL import Image, ImageDraw, ImageFont

src, dst = sys.argv[1], sys.argv[2]
EMOJI = {"🦇": "◆", "🕯": "†", "👁": "◉", "🔑": "»", "📜": "§", "🌅": "☼", "✔": "✔", "✘": "✘"}
COLORS = {"0": (233, 230, 242), "2": (143, 137, 166), "1": None, "32": (87, 214, 141), "31": (255, 95, 109),
          "38;5;208": (255, 138, 31), "38;5;141": (169, 139, 255)}
font = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", 15)
bold = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf", 15)
W, LH, PAD = 1000, 21, 18

lines = open(src, encoding="utf-8").read().splitlines()
def parse(line):
    for k, v in EMOJI.items():
        line = line.replace(k, v)
    out, color, b = [], COLORS["0"], False
    for part in re.split(r"(\x1b\[[0-9;]*m)", line):
        m = re.fullmatch(r"\x1b\[([0-9;]*)m", part)
        if m:
            code = m.group(1)
            if code in ("0", ""):
                color, b = COLORS["0"], False
            elif code == "1":
                b = True
            elif code in COLORS:
                color = COLORS[code]
        elif part:
            out.append((part, color, b))
    return out

parsed = [parse(l) for l in lines]
rows = 30
H = PAD * 2 + LH * rows + 26

def frame(n):
    img = Image.new("RGB", (W, H), (11, 10, 16))
    d = ImageDraw.Draw(img)
    for i, c in enumerate([(255, 95, 86), (255, 189, 46), (39, 201, 63)]):
        d.ellipse((14 + i * 20, 9, 26 + i * 20, 21), fill=c)
    d.text((80, 7), "cargo run --example haunted_payroll", font=font, fill=(143, 137, 166))
    visible = parsed[max(0, n - rows):n]
    for r, segs in enumerate(visible):
        x = PAD
        for text, color, b in segs:
            f = bold if b else font
            d.text((x, 30 + PAD + r * LH), text, font=f, fill=color)
            x += d.textlength(text, font=f)
    return img

frames, durations = [], []
for n in range(1, len(parsed) + 1):
    frames.append(frame(n))
    durations.append(120 if not lines[n - 1].strip() else 380)
durations[-1] = 4000
frames[0].save(dst, save_all=True, append_images=frames[1:], duration=durations, loop=0, optimize=True)
print("wrote", dst, len(frames), "frames")
