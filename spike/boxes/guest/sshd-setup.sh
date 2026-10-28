set -e
export HOME=/data/home PATH=/data/home/.local/bin:$PATH
R=https://dl-cdn.alpinelinux.org/alpine/v3.24/main/aarch64
mkdir -p /data/opt/ssh /data/root/.ssh /tmp/apk && cd /tmp/apk
curl -fsSL $R/APKINDEX.tar.gz | tar xzO APKINDEX 2>/dev/null > index.txt
ver() { awk -v p="$1" 'BEGIN{RS="";FS="\n"} { for(i=1;i<=NF;i++){ if($i=="P:"p) found=1; if(found && $i ~ /^V:/){ sub(/^V:/,"",$i); print $i; exit } } }' index.txt; }
for p in dropbear openssh-sftp-server; do v=$(ver $p); echo "$p $v"; curl -fsSL "$R/$p-$v.apk" -o $p.apk; tar xzf $p.apk -C /data/opt/ssh 2>/dev/null || true; done
ls /data/opt/ssh/usr/sbin /data/opt/ssh/usr/lib/ssh 2>/dev/null
echo "deps:"; ls /usr/lib | grep -E '^libcrypto|^libz\.' | tr '\n' ' '; echo
mount --bind /data/root /root
printf '%s\n' 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHIUUDof7i7Vnho+0Hsly0XuWHegRyGMiMZDrU7E8eEk fletch-box-spike' > /root/.ssh/authorized_keys; chmod 700 /root/.ssh; chmod 600 /root/.ssh/authorized_keys
[ -f /data/opt/ssh/host_key ] || /data/opt/ssh/usr/bin/dropbearkey -t ed25519 -f /data/opt/ssh/host_key >/dev/null
mkdir -p /tmp/www && echo '<h1>hello over ssh -L</h1>' > /tmp/www/index.html && (cd /tmp/www && nohup python3 -m http.server 5173 --bind 127.0.0.1 >/tmp/http.log 2>&1 &)
(nohup socat VSOCK-LISTEN:2222,reuseaddr,fork EXEC:"/data/opt/ssh/usr/sbin/dropbear -i -r /data/opt/ssh/host_key -E" >/tmp/dropbear.log 2>&1 &)
sleep 0.3; echo SSHD_UP
