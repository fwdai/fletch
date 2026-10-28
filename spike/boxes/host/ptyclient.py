#!/usr/bin/env python3 -I
"""Attach to a guest PTY over a libkrun listen socket: read for a while, send keys, read again, report."""
import re, socket, sys, time
path, wait = sys.argv[1], float(sys.argv[2])
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); t0 = time.time(); s.connect(path); s.settimeout(0.5)
def drain(seconds):
    out, end = b"", time.time() + seconds
    while time.time() < end:
        try: out += s.recv(65536)
        except socket.timeout: pass
        except OSError: break
    return out
first = drain(wait)
t_first = time.time() - t0
ansi = re.compile(rb"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07|\x1b[()][A-Z0-9]")
text = ansi.sub(b"", first).decode(errors="replace")
lines = [l.strip() for l in text.splitlines() if l.strip()]
print(f"[pty] {len(first)} bytes in {t_first:.2f}s, {len(lines)} non-empty lines; first lines:")
for l in lines[:14]: print("  |", l[:110])
s.sendall(b"\x03"); time.sleep(0.3); s.sendall(b"\x03")
tail = ansi.sub(b"", drain(1.5)).decode(errors="replace").strip().splitlines()
print("[pty] after Ctrl-C x2:", [l.strip()[:80] for l in tail if l.strip()][-3:])
