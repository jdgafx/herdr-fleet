"""Redact the LAST OUTPUT column of fleet rows in an ANSI capture.

Reads the raw `tmux capture-pane -e` text, finds the column where the
LAST OUTPUT header starts, and replaces each agent row's text from that
column on with [redacted]. Keeps colours on everything before it.
"""
import re, sys

ESC = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")

def visible(line):
    return ESC.sub("", line)

def col_of(plain, word):
    return plain.find(word)

def scrub(lines):
    header = next((l for l in lines if "LAST OUTPUT" in visible(l)), None)
    if header is None:
        return lines
    col = col_of(visible(header), "LAST OUTPUT") - 6  # spill from the FLAGS cell, if any
    out = []
    for line in lines:
        if not re.search(r"\bw\w+:p\w+", visible(line)):
            out.append(line)
            continue
        kept, vis, i = [], 0, 0
        redacted = False
        while i < len(line):
            m = ESC.match(line, i)
            if m:
                if vis < col:
                    kept.append(m.group(0))
                i = m.end()
                continue
            ch = line[i]
            if vis >= col:
                if not redacted:
                    kept.append("[redacted]")
                    redacted = True
            else:
                kept.append(ch)
            vis += 1
            i += 1
        out.append("".join(kept) + "\x1b[0m")
    return out

if __name__ == "__main__":
    src, dst = sys.argv[1], sys.argv[2]
    with open(src, encoding="utf-8") as f:
        lines = f.read().split("\n")
    with open(dst, "w", encoding="utf-8") as f:
        f.write("\n".join(scrub(lines)))
