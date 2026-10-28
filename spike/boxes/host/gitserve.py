#!/usr/bin/env python3 -I
"""Serve one repository's upload-pack on a unix socket: the host side of 'the guest clones from the host'."""
import os, socket, subprocess, sys
sock_path, repo = sys.argv[1], sys.argv[2]
try: os.unlink(sock_path)
except FileNotFoundError: pass
srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); srv.bind(sock_path); srv.listen(8)
print(f"gitserve: {repo} on {sock_path}", flush=True)
while True:
    conn, _ = srv.accept()
    if os.fork() == 0:
        srv.close()
        fd = conn.fileno()
        rc = subprocess.call(["git", "upload-pack", "--strict", repo], stdin=fd, stdout=fd)
        conn.close(); os._exit(rc)
    conn.close()
