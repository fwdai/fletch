echo BOOT2_OK
export PATH=/data/home/.local/bin:$PATH
cd /data/workspace/repo && git log --oneline -1 && git status --short | head -3
echo "claude=$(claude --version 2>&1 | head -1)"
echo "codex=$(codex --version 2>&1 | head -1)"
echo PROBE2_DONE
