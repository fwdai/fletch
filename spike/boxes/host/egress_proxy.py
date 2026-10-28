#!/usr/bin/env python3 -I
"""Minimal HTTP CONNECT proxy on a unix socket with a host allow-list and a log. Spike stand-in for the real proxy."""
import os, socket, sys, threading, time
sock_path, log_path = sys.argv[1], sys.argv[2]
allow = set(sys.argv[3:])
def allowed(host):
    return host in allow or any(host.endswith("." + a) for a in allow)
log = open(log_path, "a", buffering=1)
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
def handle(conn):
    try:
        head = b""
        while b"\r\n\r\n" not in head:
            d = conn.recv(4096)
            if not d: return
            head += d
        line = head.split(b"\r\n", 1)[0].decode(errors="replace")
        method, target = line.split(" ")[:2]
        if method != "CONNECT":
            conn.sendall(b"HTTP/1.1 405 only CONNECT\r\n\r\n"); log.write(f"{time.time():.3f} DENY {line}\n"); return
        host, port = target.rsplit(":", 1)
        if not allowed(host):
            conn.sendall(b"HTTP/1.1 403 not allowed\r\n\r\n"); log.write(f"{time.time():.3f} DENY {host}:{port}\n"); return
        up = socket.create_connection((host, int(port)), timeout=20)
        conn.sendall(b"HTTP/1.1 200 OK\r\n\r\n")
        log.write(f"{time.time():.3f} ALLOW {host}:{port}\n")
        t = threading.Thread(target=pipe, args=(up, conn), daemon=True); t.start()
        pipe(conn, up); t.join(5)
    except Exception as e:
        log.write(f"{time.time():.3f} ERR {e}\n")
    finally:
        conn.close()
try: os.unlink(sock_path)
except FileNotFoundError: pass
srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); srv.bind(sock_path); srv.listen(64)
print(f"egress: {sock_path} allow={sorted(allow)}", flush=True)
while True:
    c, _ = srv.accept(); threading.Thread(target=handle, args=(c,), daemon=True).start()
