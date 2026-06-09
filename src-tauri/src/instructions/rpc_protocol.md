## Quorum app actions (file RPC)

Your writes are confined to your worktree. When a task needs an action the app must run for you, ask for it through the file mailbox at `$QUORUM_RPC_DIR` and wait for the reply. Only the ops listed below are accepted; anything else is rejected.

To make a call:

1. Choose a unique id (a UUID works).
2. Write the request **atomically**: write `$QUORUM_RPC_DIR/requests/<id>.json.tmp`, then rename it to `$QUORUM_RPC_DIR/requests/<id>.json`. Body: `{"id":"<id>","op":"<op>","args":{}}`.
3. Poll for `$QUORUM_RPC_DIR/responses/<id>.json` until it appears (every ~0.2s, with your own timeout), then read it.

A success response is `{"id","ok":true,"exit_code","stdout","stderr"}`; a failure is `{"id","ok":false,"error":"..."}`.

Wait for a reply (shell):

```sh
ID=$(uuidgen)
printf '{"id":"%s","op":"ping"}' "$ID" > "$QUORUM_RPC_DIR/requests/$ID.json.tmp"
mv "$QUORUM_RPC_DIR/requests/$ID.json.tmp" "$QUORUM_RPC_DIR/requests/$ID.json"
until [ -f "$QUORUM_RPC_DIR/responses/$ID.json" ]; do sleep 0.2; done
cat "$QUORUM_RPC_DIR/responses/$ID.json"
```

Available ops:

- `ping` — liveness check; returns `pong`. No args.
- `git_status` — runs `git status` in your worktree and returns its output. No args.
