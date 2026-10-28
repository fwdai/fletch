#!/usr/bin/env python3 -I
"""Send a shell script over a unix socket to a guest 'sh -s' and print what comes back.
Modes: connect SOCK  (libkrun listen-mode port)  |  accept SOCK  (guest-initiated reverse channel)."""
import socket, sys, time
mode, path = sys.argv[1], sys.argv[2]
script = sys.stdin.read()
t0 = time.time()
if mode == "connect":
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect(path)
else:
    try:
        import os; os.unlink(path)
    except FileNotFoundError: pass
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); srv.bind(path); srv.listen(1); srv.settimeout(15)
    s, _ = srv.accept()
t1 = time.time()
s.sendall(script.encode() + b"\nexit\n")
s.settimeout(float(sys.argv[3]) if len(sys.argv) > 3 else 10)
out = b""
try:
    while True:
        d = s.recv(65536)
        if not d: break
        out += d
except socket.timeout:
    out += b"\n[timeout]"
t2 = time.time()
sys.stdout.write(out.decode(errors="replace"))
print(f"\n[{mode}] connected in {t1-t0:.3f}s, {len(out)} bytes, total {t2-t0:.3f}s", file=sys.stderr)
