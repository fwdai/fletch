#!/usr/bin/env python3 -I
"""stdio <-> unix socket, for git's ext:: transport on the host: git fetch "ext::python3 -I sockpipe.py SOCK"."""
import os, socket, sys, threading
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(sys.argv[1])
def to_sock():
    try:
        while True:
            d = os.read(0, 65536)
            if not d:
                break
            s.sendall(d)
    finally:
        try:
            s.shutdown(socket.SHUT_WR)
        except OSError:
            pass
t = threading.Thread(target=to_sock, daemon=True)
t.start()
while True:
    d = s.recv(65536)
    if not d:
        break
    os.write(1, d)
