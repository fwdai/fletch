## Quorum app RPC

Your writes are confined to your worktree. When an agent needs the app to do
something it cannot do itself, send a JSON request through the mailbox at
`$QUORUM_RPC_DIR` and wait for the reply.

Protocol shape:

1. Pick a unique id, usually a UUID.
2. Write the request atomically to `$QUORUM_RPC_DIR/requests/<id>.json.tmp`,
   then rename it to `$QUORUM_RPC_DIR/requests/<id>.json`.
3. Poll `$QUORUM_RPC_DIR/responses/<id>.json` until it appears, then read it.

Request body:

```json
{"id":"<id>","op":"<op>","args":{}}
```

`op` is an application-defined message name. `args` is an arbitrary JSON object
for that operation's payload. The app replies with one final response payload;
there is no streaming.

A success response is:

```json
{"id":"<id>","ok":true,"exit_code":0,"stdout":"","stderr":""}
```

A failure response is:

```json
{"id":"<id>","ok":false,"error":"..."}
```

Example:

```sh
ID=$(uuidgen)
printf '{"id":"%s","op":"ping"}' "$ID" > "$QUORUM_RPC_DIR/requests/$ID.json.tmp"
mv "$QUORUM_RPC_DIR/requests/$ID.json.tmp" "$QUORUM_RPC_DIR/requests/$ID.json"
until [ -f "$QUORUM_RPC_DIR/responses/$ID.json" ]; do sleep 0.2; done
cat "$QUORUM_RPC_DIR/responses/$ID.json"
```

For free-text args, build the JSON with `jq -n --arg` so quotes and newlines
are escaped correctly. The current dispatcher exposes a harmless `echo` op for
round-trip testing:

```sh
ID=$(uuidgen)
MSG=$(cat <<'EOF'
hello from the agent
EOF
)
jq -n --arg id "$ID" --arg msg "$MSG" '{id:$id,op:"echo",args:{message:$msg}}' \
  > "$QUORUM_RPC_DIR/requests/$ID.json.tmp"
mv "$QUORUM_RPC_DIR/requests/$ID.json.tmp" "$QUORUM_RPC_DIR/requests/$ID.json"
```

The exact op names are feature-specific. Quorum currently uses this same
transport for Git actions, and the Git dispatcher also exposes `echo` as a
simple round-trip check. Future app features can define their own ops on top of
the same mailbox.
