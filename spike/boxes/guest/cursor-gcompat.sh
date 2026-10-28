export HOME=/data/home PATH=/data/opt/bash/bin:/data/home/.local/bin:$PATH LD_LIBRARY_PATH=/data/opt/bash/usr/lib
echo "--- overlayfs available: $(grep -c overlay /proc/filesystems)"
mkdir -p /data/ov/lib /data/ov/work && mount -t overlay overlay -o lowerdir=/lib,upperdir=/data/ov/lib,workdir=/data/ov/work /lib && echo "overlay mounted on /lib"
cp -a /data/opt/gcompat/lib/* /lib/ && cp -a /data/opt/gcompat/usr/lib/* /lib/ && ls /lib | grep -E 'ld-linux|libc.so.6|gcompat' | tr '\n' ' '; echo
V=/data/home/.local/share/cursor-agent/versions/2026.10.22-27dcf5f
echo "--- bundled node under gcompat:"; $V/node --version 2>&1 | head -3
echo "--- cursor-agent under gcompat:"; cursor-agent --version 2>&1 | head -5
echo "--- cursor-agent-sea directly:"; $V/cursor-agent-sea --version 2>&1 | head -3
umount /lib 2>/dev/null && echo "overlay removed"
