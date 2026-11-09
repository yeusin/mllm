#!/usr/bin/env python3
"""Minimal wide-char-aware terminal emulator (reference model).

Usage: python3 ref_emu.py <raw_capture_file> <rows> <cols>

Renders a raw terminal byte stream (e.g. a PTY capture or the host-side
emission produced by the REPLAY_MULTIFRAME test) and prints the resulting
screen, one line per row. Used as the ground-truth/verification side of the
pane-fidelity harness described in docs/vendored-vt100.md.

Models the host terminal faithfully for the sequences OpenCode/ratatui emit:
CUP, CUU/CUD/CUF/CUB, EL, ED, CR, LF, BS, IND, RI, DECSC/DECRC (ESC 7/8),
CSI s/u, SGR (ignored), and private modes (ignored). Wide chars (EAW W/F)
occupy two columns and advance the cursor by two — the critical behavior.
"""
import sys, re, unicodedata

def cw(ch):
    if unicodedata.east_asian_width(ch) in ('W', 'F'):
        return 2
    return 1

class Emu:
    def __init__(self, rows, cols):
        self.rows, self.cols = rows, cols
        self.grid = [[' ']*cols for _ in range(rows)]
        self.r = 0; self.c = 0
        self.saved = (0, 0)

    def put(self, ch):
        w = cw(ch)
        if self.c >= self.cols:
            self.c = 0; self.r += 1
        if self.r >= self.rows:
            self.r = self.rows - 1
        if w == 2 and self.c == self.cols - 1:
            # wide char at last column: real terminals wrap or truncate;
            # truncate here (xterm-style varies, rare edge)
            self.c = 0; self.r += 1
            if self.r >= self.rows: self.r = self.rows - 1
        self.grid[self.r][self.c] = ch
        if w == 2:
            if self.c + 1 < self.cols:
                self.grid[self.r][self.c+1] = ''  # continuation
            self.c += 2
        else:
            self.c += 1

    def render(self):
        out = []
        for row in self.grid:
            out.append(''.join(ch if ch else '' for ch in row))
        return out

def feed(emu, data):
    text = data.decode('utf-8', errors='replace')
    i = 0; n = len(text)
    while i < n:
        ch = text[i]
        if ch == '\x1b':
            m = re.match(r'\x1b\[([\?0-9;]*)([A-Za-z@`])', text[i:])
            if m:
                params, final = m.group(1), m.group(2)
                pl = [int(x) if x else 0 for x in params.lstrip('?').split(';') if x != ''] if params.strip('?') else []
                p1 = pl[0] if pl else 1
                p2 = pl[1] if len(pl) > 1 else 1
                if final == 'H' or final == 'f':
                    emu.r = max(0, min(emu.rows-1, p1-1))
                    emu.c = max(0, min(emu.cols-1, p2-1))
                elif final == 'A': emu.r = max(0, emu.r - max(1,p1))
                elif final == 'B': emu.r = min(emu.rows-1, emu.r + max(1,p1))
                elif final == 'C': emu.c = min(emu.cols-1, emu.c + max(1,p1))
                elif final == 'D': emu.c = max(0, emu.c - max(1,p1))
                elif final == 'J':
                    mode = pl[0] if pl else 0
                    if mode == 2 or mode == 3:
                        emu.grid = [[' ']*emu.cols for _ in range(emu.rows)]
                    elif mode == 0:
                        for cc in range(emu.c, emu.cols): emu.grid[emu.r][cc] = ' '
                        for rr in range(emu.r+1, emu.rows): emu.grid[rr] = [' ']*emu.cols
                    elif mode == 1:
                        for rr in range(0, emu.r): emu.grid[rr] = [' ']*emu.cols
                        for cc in range(0, emu.c+1): emu.grid[emu.r][cc] = ' '
                elif final == 'K':
                    mode = pl[0] if pl else 0
                    if mode == 0:
                        for cc in range(emu.c, emu.cols): emu.grid[emu.r][cc] = ' '
                    elif mode == 1:
                        for cc in range(0, min(emu.c+1, emu.cols)): emu.grid[emu.r][cc] = ' '
                    else:
                        emu.grid[emu.r] = [' ']*emu.cols
                elif final == 's': emu.saved = (emu.r, emu.c)
                elif final == 'u': emu.r, emu.c = emu.saved
                i += m.end(); continue
            m = re.match(r'\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)', text[i:])  # OSC
            if m: i += m.end(); continue
            m = re.match(r'\x1bP[^\x1b]*\x1b\\', text[i:])  # DCS
            if m: i += m.end(); continue
            m = re.match(r'\x1b([78MDEc=>])', text[i:])
            if m:
                f = m.group(1)
                if f == '7': emu.saved = (emu.r, emu.c)
                elif f == '8': emu.r, emu.c = emu.saved
                elif f == 'D':
                    emu.r += 1
                    if emu.r >= emu.rows: emu.r = emu.rows - 1
                elif f == 'M':
                    emu.r -= 1
                    if emu.r < 0: emu.r = 0
                i += m.end(); continue
            i += 1; continue
        elif ch == '\r': emu.c = 0; i += 1
        elif ch == '\n':
            emu.r += 1
            if emu.r >= emu.rows: emu.r = emu.rows - 1
            i += 1
        elif ch == '\b': emu.c = max(0, emu.c - 1); i += 1
        elif ch == '\t': emu.c = min(emu.cols-1, (emu.c//8+1)*8); i += 1
        elif ord(ch) < 32: i += 1
        else: emu.put(ch); i += 1
    return emu

if __name__ == '__main__':
    rows, cols = int(sys.argv[2]), int(sys.argv[3])
    emu = Emu(rows, cols)
    feed(emu, open(sys.argv[1],'rb').read())
    for i, line in enumerate(emu.render()):
        print(f"{i:02}|{line}|")
