set -e
R=https://dl-cdn.alpinelinux.org/alpine/v3.24/main/aarch64; mkdir -p /tmp/apk /data/opt/bash; cd /tmp/apk
[ -s index.txt ] || curl -fsSL $R/APKINDEX.tar.gz | tar xzO APKINDEX 2>/dev/null > index.txt
ver() { awk -v p="$1" 'BEGIN{RS="";FS="\n"} { for(i=1;i<=NF;i++){ if($i=="P:"p) found=1; if(found && $i ~ /^V:/){ sub(/^V:/,"",$i); print $i; exit } } }' index.txt; }
for p in bash readline ncurses-libs ncurses-terminfo-base; do v=$(ver $p); echo "$p $v"; curl -fsSL "$R/$p-$v.apk" -o $p.apk; tar xzf $p.apk -C /data/opt/bash 2>/dev/null || true; done
sync
LD_LIBRARY_PATH=/data/opt/bash/usr/lib /data/opt/bash/bin/bash --version | head -1
