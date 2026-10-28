#!/usr/bin/env python3 -I
"""TCP 127.0.0.1:PORT <-> unix socket, so a browser or curl reaches a guest service forwarded by libkrun."""
import socket, sys, threading
port, sock_path = int(sys.argv[1]), sys.argv[2]
def pipe(a, b):
    try:
        while True:
            d = a.recv(65536)
            if not d: break
            b.sendall(d)
    except OSError: pass
    finally:
        try: b.shutdown(socket.SHUT_WR)
        except OSError: pass
def handle(c):
    u = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); u.connect(sock_path)
    threading.Thread(target=pipe, args=(u, c), daemon=True).start(); pipe(c, u); c.close()
srv = socket.socket(); srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); srv.bind(("127.0.0.1", port)); srv.listen(16)
print(f"bridge: 127.0.0.1:{port} -> {sock_path}", flush=True)
while True:
    c, _ = srv.accept(); threading.Thread(target=handle, args=(c,), daemon=True).start()
