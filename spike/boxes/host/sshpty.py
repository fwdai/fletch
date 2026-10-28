#!/usr/bin/env python3 -I
"""Run a command over ssh with a real PTY on the host side, watch what it draws for a while,
optionally type something, and report. Usage: sshpty.py SSH_CONFIG SECONDS [KEYS] -- cmd..."""
import os, pty, re, select, sys, time
cfg, secs = sys.argv[1], float(sys.argv[2])
rest = sys.argv[3:]
keys = None
if "--" in rest:
    i = rest.index("--")
    keys, cmd = (rest[0] if i == 1 else None), rest[i + 1:]
else:
    cmd = rest
pid, fd = pty.fork()
if pid == 0:
    os.environ["TERM"] = "xterm-256color"
    os.execvp("ssh", ["ssh", "-F", cfg, "-tt", "box"] + cmd)
ansi = re.compile(rb"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*(\x07|\x1b\\)|\x1b[()>=][A-Z0-9]?|\x1b[78]")
def drain(seconds):
    out, end = b"", time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.2)
        if r:
            try:
                d = os.read(fd, 65536)
            except OSError:
                break
            if not d:
                break
            out += d
    return out
t0 = time.time()
first = drain(secs)
lines = [l.strip() for l in ansi.sub(b"", first).decode(errors="replace").splitlines() if l.strip()]
print(f"[pty] {len(first)} bytes in {time.time()-t0:.1f}s, {len(lines)} lines:")
for l in lines[:25]:
    print("  |", l[:120])
if keys:
    os.write(fd, keys.encode().replace(b"\\r", b"\r").replace(b"\\n", b"\n"))
    tail = [l.strip() for l in ansi.sub(b"", drain(4)).decode(errors="replace").splitlines() if l.strip()]
    print(f"[pty] after keys {keys!r}:")
    for l in tail[-8:]:
        print("  |", l[:120])
try:
    os.kill(pid, 15)
except OSError:
    pass
