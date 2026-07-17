## Fletch app RPC

Your writes are confined to your workspace. When an agent needs the app to do
something it cannot do itself, send a JSON request through the mailbox at
`$FLETCH_RPC_DIR` and wait for the reply.

Protocol shape:

1. Pick a unique id, usually a UUID.
2. Write the request atomically to `$FLETCH_RPC_DIR/requests/<id>.json.tmp`,
   then rename it to `$FLETCH_RPC_DIR/requests/<id>.json`.
3. Poll `$FLETCH_RPC_DIR/responses/<id>.json` until it appears, then read it.

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
printf '{"id":"%s","op":"ping"}' "$ID" > "$FLETCH_RPC_DIR/requests/$ID.json.tmp"
mv "$FLETCH_RPC_DIR/requests/$ID.json.tmp" "$FLETCH_RPC_DIR/requests/$ID.json"
until [ -f "$FLETCH_RPC_DIR/responses/$ID.json" ]; do sleep 0.2; done
cat "$FLETCH_RPC_DIR/responses/$ID.json"
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
  > "$FLETCH_RPC_DIR/requests/$ID.json.tmp"
mv "$FLETCH_RPC_DIR/requests/$ID.json.tmp" "$FLETCH_RPC_DIR/requests/$ID.json"
```

The exact op names are feature-specific. Fletch uses this transport for the git
actions that need host-held GitHub credentials — `git_push`, `open_pr`, and
`git_fetch` (local git you run directly; see the git-actions playbooks). The
dispatcher also exposes `echo` as a simple round-trip check. Future app features
can define their own ops on top of the same mailbox.

In a multi-repo workspace (sibling repository checkouts under the workspace
root), every git op accepts an optional `args.repo` — the sibling checkout's
directory name — and defaults to your starting repository when absent. Commit
in each repo with plain local git; use `args.repo` only for the host-brokered
ops above, e.g. `{"op":"git_push","args":{"repo":"backend","branch":"feat/x"}}`.
