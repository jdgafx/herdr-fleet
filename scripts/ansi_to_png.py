#!/usr/bin/env python3
"""Render a tmux `capture-pane -e` dump (ANSI SGR colours) into a PNG.

Usage: ansi_to_png.py <capture.txt> <out.png>

Used by scripts/screenshot.sh to turn a real herdr-fleet run into the README
image. It understands the 16 base colours, bright variants, and 256/true-colour
SGR sequences that crossterm emits.
"""
import re
import sys

from PIL import Image, ImageDraw, ImageFont

CELL_W, CELL_H, PAD, FONT_SIZE = 9, 19, 12, 15
FONT_PATH = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"
DEFAULT_BG = (24, 24, 37)
DEFAULT_FG = (205, 214, 244)

BASE = [
    (30, 30, 46), (243, 139, 168), (166, 227, 161), (249, 226, 175),
    (137, 180, 250), (203, 166, 247), (148, 226, 213), (205, 214, 244),
]
BRIGHT = [
    (88, 91, 112), (243, 139, 168), (166, 227, 161), (249, 226, 175),
    (137, 180, 250), (203, 166, 247), (148, 226, 213), (205, 214, 244),
]
SGR = re.compile(r"\x1b\[([0-9;]*)m")

# Decorative glyphs the TUI uses that DejaVu Sans Mono lacks.
SUBST = str.maketrans({
    "⏵": ">", "✳": "*", "✻": "*", "◐": "o",
    "❯": ">", "▶": ">", "█": "#",
})


def xterm256(n):
    if n < 8:
        return BASE[n]
    if n < 16:
        return BRIGHT[n - 8]
    if n < 232:
        n -= 16
        conv = lambda c: 0 if c == 0 else 55 + c * 40
        return (conv(n // 36), conv((n % 36) // 6), conv(n % 6))
    c = 8 + (n - 232) * 10
    return (c, c, c)


def apply_sgr(params, fg, bg):
    vals = [int(p) for p in params.split(";") if p != ""] if params else [0]
    i = 0
    while i < len(vals):
        v = vals[i]
        if v == 0:
            fg, bg = DEFAULT_FG, DEFAULT_BG
        elif v == 1:
            fg = BRIGHT[7]
        elif 30 <= v <= 37:
            fg = BASE[v - 30]
        elif 90 <= v <= 97:
            fg = BRIGHT[v - 90]
        elif v == 39:
            fg = DEFAULT_FG
        elif 40 <= v <= 47:
            bg = BASE[v - 40]
        elif 100 <= v <= 107:
            bg = BRIGHT[v - 100]
        elif v == 49:
            bg = DEFAULT_BG
        elif v in (38, 48):
            if vals[i + 1 : i + 2] == [5] and i + 2 < len(vals):
                col = xterm256(vals[i + 2])
                fg, bg = (col, bg) if v == 38 else (fg, col)
                i += 2
            elif vals[i + 1 : i + 2] == [2] and i + 4 < len(vals):
                col = tuple(vals[i + 2 : i + 5])
                fg, bg = (col, bg) if v == 38 else (fg, col)
                i += 4
        i += 1
    return fg, bg


def spans(line):
    out, pos, fg, bg = [], 0, DEFAULT_FG, DEFAULT_BG
    for m in SGR.finditer(line):
        if m.start() > pos:
            out.append((line[pos : m.start()], fg, bg))
        fg, bg = apply_sgr(m.group(1), fg, bg)
        pos = m.end()
    out.append((line[pos:], fg, bg))
    return out


def main():
    src, dst = sys.argv[1], sys.argv[2]
    with open(src, encoding="utf-8") as fh:
        lines = fh.read().split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    lines = [ln.translate(SUBST) for ln in lines]

    width = max((len(SGR.sub("", ln)) for ln in lines), default=1)
    img = Image.new("RGB", (PAD * 2 + width * CELL_W, PAD * 2 + len(lines) * CELL_H), DEFAULT_BG)
    draw = ImageDraw.Draw(img)
    font = ImageFont.truetype(FONT_PATH, FONT_SIZE)

    for row, line in enumerate(lines):
        x = PAD
        y = PAD + row * CELL_H
        for text, fg, bg in spans(line):
            if not text:
                continue
            cw = len(text) * CELL_W
            if bg != DEFAULT_BG:
                draw.rectangle([x, y, x + cw - 1, y + CELL_H - 1], fill=bg)
            draw.text((x, y), text, font=font, fill=fg)
            x += cw

    img.save(dst)
    print(f"wrote {dst} ({img.width}x{img.height})")


if __name__ == "__main__":
    main()
