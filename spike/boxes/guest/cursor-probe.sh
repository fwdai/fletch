export HOME=/data/home PATH=/data/opt/bash/bin:/data/home/.local/bin:$PATH LD_LIBRARY_PATH=/data/opt/bash/usr/lib
export HTTPS_PROXY=http://127.0.0.1:8899 https_proxy=http://127.0.0.1:8899 HTTP_PROXY=http://127.0.0.1:8899 http_proxy=http://127.0.0.1:8899 NO_PROXY=127.0.0.1,localhost
echo "--- installer"; start=$(date +%s); curl -fsSL https://cursor.com/install -o /tmp/cursor-install.sh && wc -c < /tmp/cursor-install.sh && grep -n -i -E 'musl|glibc|alpine|uname|arch|download' /tmp/cursor-install.sh | head -12
bash /tmp/cursor-install.sh 2>&1 | tail -4; echo "install wall=$(( $(date +%s) - start ))s"
echo "--- what landed"; ls -la $HOME/.local/bin/ | grep -i cursor; d=$(readlink -f $HOME/.local/bin/cursor-agent 2>/dev/null); echo "cursor-agent -> $d"; ls $(dirname "$d") 2>/dev/null | head -12; du -sh $(dirname "$d") 2>/dev/null | tail -1
echo "--- binaries and their interpreters"; for f in $(find $(dirname "$d") -maxdepth 1 -type f 2>/dev/null); do if head -c 4 "$f" | grep -q ELF; then printf '%s: ' "$(basename $f)"; strings "$f" | grep -m1 -E 'ld-linux|ld-musl' || echo "(static or no PT_INTERP string)"; fi; done
echo "--- run it"; cursor-agent --version 2>&1 | head -3
echo "--- gcompat attempt"; R=https://dl-cdn.alpinelinux.org/alpine/v3.24/community/aarch64; M=https://dl-cdn.alpinelinux.org/alpine/v3.24/main/aarch64; mkdir -p /tmp/apk /data/opt/gcompat; cd /tmp/apk
for repo in $M $R; do curl -fsSL $repo/APKINDEX.tar.gz | tar xzO APKINDEX 2>/dev/null >> index2.txt; done
ver() { awk -v p="$1" 'BEGIN{RS="";FS="\n"} { for(i=1;i<=NF;i++){ if($i=="P:"p) found=1; if(found && $i ~ /^V:/){ sub(/^V:/,"",$i); print $i; exit } } }' index2.txt; }
for p in gcompat libucontext libobstack musl-obstack; do v=$(ver $p); [ -n "$v" ] || continue; for repo in $M $R; do curl -fsSL "$repo/$p-$v.apk" -o $p.apk 2>/dev/null && break; done; tar xzf $p.apk -C /data/opt/gcompat 2>/dev/null || true; echo "$p $v"; done
ls /data/opt/gcompat/lib /data/opt/gcompat/usr/lib 2>/dev/null | tr '\n' ' '; echo
loader=$(ls /data/opt/gcompat/lib/ld-linux-aarch64.so.1 2>/dev/null); echo "loader: $loader"
[ -n "$loader" ] && bin=$(find $(dirname "$d") -maxdepth 1 -type f -name 'cursor-agent*' | head -1) && echo "running: $loader $bin --version" && LD_LIBRARY_PATH=/data/opt/gcompat/usr/lib:/data/opt/gcompat/lib "$loader" "$bin" --version 2>&1 | head -5
