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
actions that need host-held GitHub credentials — `git_push`, `open_pr`,
`git_fetch` (`args.ref` or `args.refs`, `pull/<n>/head` for a PR's head)
and `git_ls_remote` (origin's branches, `args.prefix`); local git
you run directly (see the git-actions playbooks). The dispatcher also exposes
`echo` as a simple round-trip check. Future app features can define their own
ops on top of the same mailbox.

To read your branch's pull request, use `pr_view` (number, URL, state, draft
flag, title, body, head `branch` and `head_sha`, `base`, `mergeable`,
`merge_state`, author, commit and file counts; optional `args.number` for
another PR), `pr_list` (the repository's pull requests, newest-updated first: number,
title, state, draft, head `branch`, `base`, URL, author; `args.state` open
(default), closed or all, `args.head` / `args.base` for one branch,
`args.limit` default 20, max 100 — how to find a branch's PR number or walk a
stack), `pr_checks` (CI results: each run's `id`, name, status, conclusion and URL;
optional `args.number` for another PR), `pr_check_log` (one failing check's
job log tail and annotations: `args.check` is a run's `id` or `name` from
`pr_checks`, `args.lines` how many trailing lines, default 200, max 2000), `pr_comments`
(the newest conversation comments, oldest first, with author, body, time and
URL; optional `args.limit`, default 50, max 100) and `pr_threads` (unresolved
review threads). Each returns JSON in `stdout`; an error names the reason (no
PR yet or none visible to the host's account, not a GitHub origin, host not
signed in, or a rate-limit pause to retry after). Do not call `gh` or the
GitHub API directly: your sandbox has no GitHub credentials.

Issues of the repository: `issue_view` (`args.number`; title, state, body,
labels, assignees, author and the newest `args.comments` comments, default
50) and `issue_list` (`args.state` open (default), closed or all,
`args.labels` as a comma-separated string or a list, `args.limit` default 20,
max 100).

To change your pull request after opening it, use `pr_edit` with any of
`args.title`, `args.body` (replaces the whole description — keep the
`## Decisions` section), `args.base` (retarget; must exist on origin) and
`args.draft` (true/false); `args.number` edits another PR of the repository.
Refresh the description with it whenever follow-up commits make the one you
wrote at `open_pr` stale.

### Workspace title

`set_title` names the work for the sidebar: 3–6 words, max 80 chars, no
double quotes. Fire-and-forget — do not wait for the response:

```sh
ID=$(uuidgen); printf '{"id":"%s","op":"set_title","args":{"title":"Swap sidebar title and prompt"}}' "$ID" > "$FLETCH_RPC_DIR/requests/$ID.json.tmp" && mv "$FLETCH_RPC_DIR/requests/$ID.json.tmp" "$FLETCH_RPC_DIR/requests/$ID.json"
```

- First tool call of the first turn, if the user has asked for a change,
  investigation, or deliverable. Not on greetings, small talk, or clarifying
  questions.
- Each later turn: if untitled and the purpose is now clear, call it first.
- Untitled after the user's third message: call it with your best inference.
- Repeat calls overwrite. Re-call only when the purpose changes materially.
- Do it silently. Do not mention it in your replies.

In a multi-repo workspace (sibling repository checkouts under the workspace
root), every git op accepts an optional `args.repo` — the sibling checkout's
directory name — and defaults to your starting repository when absent. Commit
in each repo with plain local git; use `args.repo` only for the host-brokered
ops above, e.g. `{"op":"git_push","args":{"repo":"backend","branch":"feat/x"}}`.
