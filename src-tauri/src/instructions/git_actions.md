## Fletch app git actions

When the user clicks a git action in the app, the app sends a one-line request on their behalf:

    [app-action] <name> key="value" ...

Treat it exactly like a user request and follow the matching playbook below — nothing more, nothing less. Use the file-RPC ops (`git_commit`, `git_push`, `git_update_branch`, `open_pr`) for every git mutation; raw `git commit`/`push`/`merge` are blocked by your sandbox. Keep your reply brief: one line on what you did (plus the PR URL when you opened one).

Your worktree starts with no branch (detached HEAD) — that's expected. The branch is created the first time you push, and **you choose its name**: pass `args.branch` to `open_pr` (or `git_push`) with a short, conventional, descriptive name for the work — `fix/…`, `feat/…`, or `chore/…` (no `fletch/` prefix), e.g. `fix/login-crash`. Run `git status` if unsure whether you're already on a branch; once you are, omit `args.branch` and later pushes update that same branch.

### commit

Review the uncommitted changes (`git status`, `git diff HEAD`), write a clear, conventional commit message, and commit by calling `git_commit`. Commit ONLY — do not push and do not open a pull request.

### commit-push

Same as `commit`, then push by calling `git_push` — include `args.branch` (a conventional name like `fix/…`) if you don't have a branch yet. Do not open a pull request.

### commit-pr — params: `base`

Same as `commit`, then write a concise PR title and description covering ALL changes versus the `base` branch, and open the PR by calling `open_pr` with that title and body — plus `args.branch` (your chosen conventional name) if you don't have a branch yet.

### open-pr — params: `base`

Everything is already committed. Review the work versus `base` (`git log <base>..HEAD`, `git diff <base>...HEAD`), write a concise, descriptive PR title and body, and open the PR by calling `open_pr` with them — plus `args.branch` (your chosen conventional name) if you don't have a branch yet.

### push

Push your committed work by calling `git_push`. If you don't have a branch yet (detached HEAD), choose a conventional, descriptive name and pass it as `args.branch` (e.g. `fix/login-crash`); the branch is created at your current commit and pushed. Do not open a pull request.

### resolve-conflicts

Inspect each conflicted file in your worktree, reconcile both sides correctly, then complete the merge by calling `git_commit` with a short merge message — it stages everything for you.

### update-branch — params: `base`

The pull request can't merge cleanly because `base` has advanced. Call `git_update_branch` to merge the latest base into this branch. If it reports conflicts, resolve the affected files, complete the merge with `git_commit`, then push with `git_push`. If it merges cleanly, just push with `git_push`.

### fix-checks — params: `failing` (check names)

CI checks are failing on this branch's pull request. Investigate the failures, fix them, commit the fix with `git_commit`, and push with `git_push` so the checks re-run.
