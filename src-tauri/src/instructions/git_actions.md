## Fletch app git actions

When the user clicks a git action in the app, the app sends a one-line request on their behalf:

    [app-action] <name> key="value" ...

Treat it exactly like a user request and follow the matching playbook below — nothing more, nothing less. Keep your reply brief: one line on what you did (plus the PR URL when you opened one).

**Local git is yours to run directly.** Your workspace is a real checkout with a writable `.git`, so run plain git for local work: `git status`, `git add`, `git commit`, `git merge`, and conflict resolution all work in-place. What Fletch runs *for* you are the actions that need your GitHub credentials — and those stay on the host, never in your sandbox. So for anything that talks to the remote, use the file-RPC ops:

- **`git_push`** — push the current branch to `origin`. Pass `args.force=true` to push a rewritten history (e.g. after a rebase); it uses `--force-with-lease`, which rewrites the remote branch but refuses if the remote has moved in a way you haven't seen.
- **`open_pr`** — push and open a pull request.
- **`git_fetch`** — refresh a base branch from `origin` (for `update-branch`).

Your workspace starts with no branch (detached HEAD) — that's expected. The branch is created the first time you push, and **you choose its name**: pass `args.branch` to `git_push` (or `open_pr`) with a short, conventional, descriptive name for the work — `fix/…`, `feat/…`, or `chore/…` (no `fletch/` prefix), e.g. `fix/login-crash`. Run `git status` if unsure whether you're already on a branch; once you are, omit `args.branch` and later pushes update that same branch.

### commit

Review the uncommitted changes (`git status`, `git diff HEAD`), write a clear, conventional commit message, and commit with plain git — `git add -A` then `git commit -m "…"`. Commit ONLY — do not push and do not open a pull request.

### commit-push

Commit with plain git as in `commit`, then push by calling the `git_push` op — include `args.branch` (a conventional name like `fix/…`) if you don't have a branch yet. Do not open a pull request.

### commit-pr — params: `base`

Commit with plain git as in `commit`, then write a concise PR title and description covering ALL changes versus the `base` branch, and open the PR by calling the `open_pr` op with that title and body — plus `args.branch` (your chosen conventional name) if you don't have a branch yet.

### open-pr — params: `base`

Everything is already committed. Review the work versus `base` (`git log <base>..HEAD`, `git diff <base>...HEAD`), write a concise, descriptive PR title and body, and open the PR by calling the `open_pr` op with them — plus `args.branch` (your chosen conventional name) if you don't have a branch yet.

### push

Push your committed work by calling the `git_push` op. If you don't have a branch yet (detached HEAD), choose a conventional, descriptive name and pass it as `args.branch` (e.g. `fix/login-crash`); the branch is created at your current commit and pushed. If you rewrote history (rebase, amend, squash) and a normal push is rejected as non-fast-forward, retry with `args.force=true` (a lease-guarded force push). Do not open a pull request.

### resolve-conflicts

Inspect each conflicted file in your workspace, reconcile both sides correctly, then complete the merge with plain git — `git add -A` then `git commit` (an empty `-m` uses git's prepared merge message; a short summary is fine too).

### update-branch — params: `base`

The pull request can't merge cleanly because `base` has advanced. First refresh the base by calling the `git_fetch` op with `args.ref="<base>"` (this uses the host's credentials to update `origin/<base>`). Then merge it natively: `git merge origin/<base>`. If it reports conflicts, resolve the affected files and complete the merge with `git commit`; if it merged cleanly, no commit is needed. Either way, finish by pushing with the `git_push` op.

### fix-checks — params: `failing` (check names)

CI checks are failing on this branch's pull request. Investigate the failures, fix them, commit the fix with plain git (`git add -A` && `git commit`), and push by calling the `git_push` op so the checks re-run.
