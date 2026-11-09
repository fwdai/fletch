## Fletch app RPC

Your writes are confined to your workspace. When an agent needs the app to do
something it cannot do itself, send a JSON request through the mailbox at
`$FLETCH_RPC_DIR` and wait for the reply.

Protocol shape:

1. Pick a unique id of letters, digits, `-` and `_`, usually a UUID. Not every
   sandbox has `uuidgen`, so the examples fall back to the kernel's.
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
ID=$(uuidgen 2>/dev/null || cat /proc/sys/kernel/random/uuid)
printf '{"id":"%s","op":"ping"}' "$ID" > "$FLETCH_RPC_DIR/requests/$ID.json.tmp"
mv "$FLETCH_RPC_DIR/requests/$ID.json.tmp" "$FLETCH_RPC_DIR/requests/$ID.json"
until [ -f "$FLETCH_RPC_DIR/responses/$ID.json" ]; do sleep 0.2; done
cat "$FLETCH_RPC_DIR/responses/$ID.json"
```

For free-text args, build the JSON with `jq -n --arg` so quotes and newlines
are escaped correctly. The current dispatcher exposes a harmless `echo` op for
round-trip testing:

```sh
ID=$(uuidgen 2>/dev/null || cat /proc/sys/kernel/random/uuid)
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
review threads; optional `args.number` for another PR). Each returns JSON in `stdout`; an error names the reason (no
PR yet or none visible to the host's account, not a GitHub origin, host not
signed in, or a rate-limit pause to retry after). Do not call `gh` or the
GitHub API directly: your sandbox has no GitHub credentials.

`repo_view` describes the repository behind origin: visibility, default
branch, the merge methods it allows and whether merged branches are deleted.

GitHub Actions beyond a pull request — a deploy on `main` after a merge, a
scheduled job — is read with `run_list` (the repository's workflow runs,
newest first: `id`, `name`, `workflow` file, `title`, `branch`, `event`,
`status`, `conclusion`, URL; `args.branch`, `args.workflow` (a file name like
`ci.yml`), `args.status` (a status or conclusion such as `in_progress` or
`failure`) and `args.event` narrow them, `args.limit` default 20, max 100) and
`run_log` (`args.run` is a run's `id`; returns the log tail and annotations
of its first failing job, or of `args.job` by id or name, with `args.lines`
as for `pr_check_log`, plus the run's `jobs`). `workflow_dispatch` triggers
a workflow that has a `workflow_dispatch` trigger: `args.workflow` (the file
name, as `run_list` spells it), `args.ref` (a branch or tag on origin; your
checked-out branch when absent) and `args.inputs` (an object of up to ten
string values). It asks the user first when publish confirmation is on, and
GitHub reports no run id: find the run with `run_list` afterwards. Only
dispatch a workflow when the user asked for it.

Releases: `release_list` (newest first: `tag`, `name`, `draft`,
`prerelease`, URL, author, `published_at`, assets; `args.limit` default 20,
max 100) and `release_view` (one release with its notes and assets:
`args.tag`, or the latest published release without it).

Issues of the repository: `issue_view` (`args.number`; title, state, body,
labels, assignees, author and the newest `args.comments` comments, default
50) and `issue_list` (`args.state` open (default), closed or all,
`args.labels` as a comma-separated string or a list, `args.limit` default 20,
max 100).

To change your pull request after opening it, use `pr_edit` with any of
`args.title`, `args.body` (replaces the whole description — keep the
`## Decisions` section), `args.base` (retarget; must exist on origin) and
`args.draft` (true/false); `args.number` edits another PR of the repository
(its text and base only: the closing trailer and this workspace's recorded
base follow your own PR).
Refresh the description with it whenever follow-up commits make the one you
wrote at `open_pr` stale. `pr_comment` (`args.body`; `args.number` for another
PR) posts on the PR's conversation — say what a follow-up push changed there —
and `issue_comment` (`args.number`, `args.body`) on an issue. Review threads
are answered with `reply_thread`, not a comment. `pr_close` closes a PR
without merging (`args.number`; `args.comment` says why), for a superseded PR
of a stack. `pr_merge` merges the branch's PR (or `args.number`) the way the
Git panel does — auto-merge while checks run, never past a merge queue — and
asks the user first when publish confirmation is on; only do it when the user
asked you to merge.

### Running the app

The Run panel's process — the project's setup and run command, in your
checkout, under the host's sandbox — is yours to drive when you need to see a
change working: `app_start` (starts it, or reports it already running or
already finished when the command is a short one; setup runs first the one
time it has never completed), `app_status` (`phase` idle,
setup, running or stopped, the `port` the dev server launched on, and the last
error), `app_log` (the last `args.lines` lines of its output, default 200, max
2000, escapes stripped; `args.raw: true` keeps them) and `app_stop`. The start
returns at once: poll `app_status` until the phase is `running` and the port is
known, then talk to the app on that port. Stop what you started when you are
done with it. (`run_list` and `run_log` are GitHub Actions runs, above; the
`app_*` ops are this project's process on this machine.)

### Notify

`notify` (`args.title`, up to 120 characters; optional `args.body`, up to 500)
raises a native notification for the user, under their own mute. Use it when
the user is likely away and something needs them — a long task finished, a
question is blocking you — and keep the substance in your reply; it is not
a log. Fire-and-forget, like `set_title`.

### Workspace title

`set_title` names the work for the sidebar: 3–6 words, max 80 chars, no
double quotes. Fire-and-forget — do not wait for the response:

```sh
ID=$(uuidgen 2>/dev/null || cat /proc/sys/kernel/random/uuid); printf '{"id":"%s","op":"set_title","args":{"title":"Swap sidebar title and prompt"}}' "$ID" > "$FLETCH_RPC_DIR/requests/$ID.json.tmp" && mv "$FLETCH_RPC_DIR/requests/$ID.json.tmp" "$FLETCH_RPC_DIR/requests/$ID.json"
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

### Walkthrough

`tour_set` gives the user a guided reading of your changes: the Code panel
shows it beside the diff, so they can step through the change in the order it
makes sense instead of file by file. Args: `{"steps":[{"title":"…","note":"…",
"anchors":[{"path":"src/a.ts","start":10,"end":24}]}]}` — `title` optional (max
80 chars), `note` required (max 600), at most 12 `anchors` (may be empty).
`path` is relative to the repository root, prefixed with the checkout's
directory name in a multi-repo workspace (`backend/src/a.rs`); `start` and
`end` are 1-indexed, inclusive lines of the file's current content.

- Call it at the end of every turn that changed code, just before your final
  reply, covering that turn's changes; and again before `open_pr`, covering
  the whole PR. Fire-and-forget, like `set_title`.
- Order the steps the way you would explain the change, roughly the order it
  was built: contract and data, then logic, call sites, UI, tests.
- 3–12 steps, one idea each. A step groups that idea's hunks across files: a
  button, its handler and its style are one step with three anchors.
- A note is one or two sentences on what the step does and why. Do not restate
  the diff; it is right beside the note.
- A repeat call replaces the whole tour.

Put the explanation of a change in the tour, not in code comments. Comments in
code are for its future readers and must describe the code as it is, never the
change that produced it.

```sh
ID=$(uuidgen 2>/dev/null || cat /proc/sys/kernel/random/uuid)
jq -n --arg id "$ID" '{id:$id,op:"tour_set",args:{steps:[
  {title:"Limit setting",note:"Per-deployment request limit instead of a constant.",anchors:[{path:"src/config.ts",start:12,end:18}]},
  {note:"Counts requests per key and answers 429 past the limit.",anchors:[{path:"src/limit.ts",start:1,end:42},{path:"src/server.ts",start:30,end:31}]}
]}}' > "$FLETCH_RPC_DIR/requests/$ID.json.tmp" && mv "$FLETCH_RPC_DIR/requests/$ID.json.tmp" "$FLETCH_RPC_DIR/requests/$ID.json"
```
