set -e
R=https://dl-cdn.alpinelinux.org/alpine/v3.24/main/aarch64; mkdir -p /tmp/apk; cd /tmp/apk
echo "--- damaged files on the data disk (sizes):"; ls -la /data/opt/ssh/usr/lib/ | awk '{print $5, $9}' | tr '\n' ';'; echo
curl -fsSL $R/APKINDEX.tar.gz | tar xzO APKINDEX 2>/dev/null > index.txt
ver() { awk -v p="$1" 'BEGIN{RS="";FS="\n"} { for(i=1;i<=NF;i++){ if($i=="P:"p) found=1; if(found && $i ~ /^V:/){ sub(/^V:/,"",$i); print $i; exit } } }' index.txt; }
rm -rf /data/opt/ssh/usr/lib/libutmps* /data/opt/ssh/usr/lib/libskarnet*
for p in utmps-libs skalibs-libs; do v=$(ver $p); curl -fsSL "$R/$p-$v.apk" -o $p.apk; tar xzf $p.apk -C /data/opt/ssh 2>/dev/null || true; done
sync
export LD_LIBRARY_PATH=/data/opt/ssh/usr/lib
/data/opt/ssh/usr/sbin/dropbear -V 2>&1 | head -1
(nohup /data/opt/ssh/usr/sbin/dropbear -F -E -R -r /data/opt/ssh/host_key -p 127.0.0.1:2223 >/tmp/dropbear.log 2>&1 &)
sleep 0.4; netstat -tln 2>/dev/null | grep -q 2223 && echo SSHD_UP || { cat /tmp/dropbear.log; exit 1; }
