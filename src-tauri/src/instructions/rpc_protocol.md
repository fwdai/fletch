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
- `git_commit` — stages all changes in your worktree and commits them. `args.message` (required) is the commit message.
- `open_pr` — pushes your branch and opens a pull request against your base branch. `args.title` and `args.body` set the PR title and description; omit `title` to auto-fill from your commits.
- `git_push` — pushes your current branch to origin (sets the upstream on first push). No args. Use it to update an existing pull request after committing fixes.
- `git_update_branch` — fetches your base branch from origin and merges it into your current branch. No args. A non-zero `exit_code` with `CONFLICT` in `stdout` means the merge stopped on conflicts: resolve the listed files in your worktree, then complete the merge with `git_commit` and push with `git_push`.

Use these ops for git: your worktree's git database lives outside the sandbox, so running `git add`/`commit`/`merge`/`push` yourself fails with a permission error. Commit through `git_commit`, push through `git_push`, sync your base branch through `git_update_branch`, and open PRs through `open_pr` instead.
