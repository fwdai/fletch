# Runs as: /opt/calina/init /bin/sh /dev/vdc  (attached as a raw disk; no argv quoting)
echo BOOT_OK
uname -a
head -2 /etc/os-release
df -h /data | tail -1
mount | grep -E ' / | /data ' 
echo "node=$(node --version) git=$(git --version) python=$(python3 --version 2>&1)"
echo "cwd=$(pwd) home=$HOME user=$(id -u)"
echo "ldd check:"; ldd /bin/busybox 2>&1 | head -2
echo "cpu=$(nproc) mem=$(free -m | awk '/Mem/{print $2}')MB"
echo PROBE_DONE
