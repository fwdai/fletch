set -e
R=https://dl-cdn.alpinelinux.org/alpine/v3.24/main/aarch64; cd /tmp/apk
ver() { awk -v p="$1" 'BEGIN{RS="";FS="\n"} { for(i=1;i<=NF;i++){ if($i=="P:"p) found=1; if(found && $i ~ /^V:/){ sub(/^V:/,"",$i); print $i; exit } } }' index.txt; }
for p in utmps-libs skalibs-libs; do v=$(ver $p); echo "$p $v"; curl -fsSL "$R/$p-$v.apk" -o $p.apk; tar xzf $p.apk -C /data/opt/ssh 2>/dev/null || true; done
ls /data/opt/ssh/usr/lib | tr '\n' ' '; echo
pkill -f "VSOCK-LISTEN:2222" || true; pkill dropbear || true; sleep 0.2
export LD_LIBRARY_PATH=/data/opt/ssh/usr/lib
/data/opt/ssh/usr/sbin/dropbear -V 2>&1 | head -1
(nohup /data/opt/ssh/usr/sbin/dropbear -F -E -R -r /data/opt/ssh/host_key -p 127.0.0.1:2223 >/tmp/dropbear3.log 2>&1 &)
(nohup socat VSOCK-LISTEN:2222,reuseaddr,fork TCP:127.0.0.1:2223 >/tmp/relay22.log 2>&1 &)
sleep 0.5; netstat -tln 2>/dev/null | grep 2223; echo SSHD_UP
