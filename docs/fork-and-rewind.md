# Fork & rewind: design

One small set of primitives serves two features:

- **Fork** starts a new workspace whose agent knows what was discussed up to a
  message. Its code starts clean, from the parent's current tree, or as of that
  message.
- **Rewind** goes back to a message in place, optionally restoring the code to
  how it was at that message.

Rewind is a fork into the same workspace. Both are built from the pieces below,
and nothing else.

## Concepts

### 1. Anchor (cut point)
An anchor is a user turn (`session_user_turns.turn_id`), not a prompt ordinal.
It is resolved by the backend into `(origin_session_id, cut_seq)`, where
`cut_seq` is *exclusive*: records with `seq < cut_seq` are kept.

- **Before T** (rewind) cuts at the record `seq` of T's own prompt.
- **Through T** (fork) cuts at the record `seq` of the next turn after T in T's
  session that has a prompt record. A later turn without one is still running
  or was never delivered, so none of it is in the history to cut away. If no
  later turn has one, it cuts at `MAX(seq) + 1` as of the request.
- **No anchor** (header fork) cuts at the end of the current session.
- The origin is the session that owns T. That may be an ancestor, when T is
  inherited, and then the new child references the ancestor directly. The cut
  is clamped to what the requesting conversation shows of the ancestor, so
  records the ancestor gained after this branch left it stay out.
- T must be part of the requesting workspace's history (its current session or
  an ancestor, below the cut); any other turn is an error.
- A turn whose prompt has not been matched to a record yet is an explicit error
  ("still syncing"). It never silently means "everything".

App-action turns are ordinary turns here. No prefix filtering is needed,
because anchors are ids.

### 2. Session lineage
`sessions.parent_session_id` and `sessions.parent_cut_seq` define a session's
history: its parent's history below the cut (recursively), followed by its own
records. History is stored once, and children reference it.

- `session_records` and `session_user_turns` are append-only. Rows are only
  ever deleted with their session. That is what makes referencing safe.
- Each session keeps its own `seq` space, ingest offset, positional ids, turn
  matching and usage. Only *display* reads stitch.
- **Stitched read:** `read_history_records(workspace)` and
  `read_history_turns(workspace)` return each ancestor's records (or matched
  turns) below its cut, root first, then the session's own, each tagged
  `inherited: bool`. They are the only reads the existing
  `read_session_records` / `read_user_turns` Tauri commands and remote ops
  serve, so desktop and mobile get them unchanged. They are two reads rather
  than one because each command needs only its half, and records are the
  expensive half. Every internal read (ingestion, turn matching,
  `last_activity`, record counts, usage, workflow ledger) stays
  own-session-only.
- **Usage** counts only records with `inherited == false`. A workspace's
  recorded spend also counts the own records of the sessions it superseded
  (a rewind's abandoned branches), so it never drops, but not those of a
  session `detach_children` handed it, whose spend was the deleted
  workspace's.
- **Deleting an ancestor:** every path that deletes sessions (discard,
  recycled-name eviction, project delete) first calls
  `detach_children(tx, doomed_workspaces)`. A doomed session that a surviving
  session inherits from is handed over to the survivor's workspace as a
  superseded session (which is all an ancestor is), and this repeats until no
  survivor points into the doomed set. Nothing is copied or renumbered, so the
  child's own seqs, positional ids and grandchild cuts don't shift, and
  inherited turns keep their ids as anchors. What no child shows still goes:
  the moved session's records at or past every child's cut, its turns left
  without a record, and its queued messages. A missed path fails loudly through
  the FK, not silently.

### 3. Current session
A workspace has exactly one *current* session (`superseded_at IS NULL`,
enforced by a partial unique index). Older sessions stay only as ancestors.

One helper resolves the current session. No query may use
`ORDER BY created_at DESC LIMIT 1`, and no update may hit every session of a
workspace through `WHERE workspace_id`. SQL that has to say it inline (a join,
a single-statement update) uses the same predicate, `superseded_at IS NULL`.

### 4. Handoff context: what a new session's agent knows

| Mode | Used by | What the agent gets |
|---|---|---|
| `None` | fork "fresh" | nothing carried |
| `Summary` | fork; rewind fallback | an LLM-written summary of the history up to the anchor, stored on the session and injected through the normal instruction path (bounded, so it fits argv on every OS) |
| `Exact` | rewind (supported providers) | a native lossless continuation: the provider resumes its own session truncated at the anchor (Claude: `--resume <src> --fork-session --session-id <new> --resume-session-at <msg>`) |

`Summary` is provider-neutral prose, so it also enables cross-provider forks
later.

`Exact` (Claude) facts, from the 2.1.287 binary:
- `--resume-session-at` takes the uuid of the last chain entry to *keep*. To
  rewind before prompt T, pass T's `parentUuid` (`agent::claude_branch_before`).
- It is honored only in print mode, so `Exact` works in the chat view only.
  Switching to the native view is refused until the branch has its first
  message.
- A resume can't cut before the last compaction. Such cuts fall back to
  `Summary`.
- The branched transcript repeats the kept history under the same `uuid`s.
  Ingestion for a branched session skips records whose `native_id` is already
  in its inherited history.
- The branch point is persisted on the session (`branch_from_session`,
  `branch_at_message`, migration 0042). A launch branches while the session's
  own transcript has no message yet, so it survives restarts with no in-memory
  state.

The summarizer runs once at fork time, as a provisioning stage
(`Summarizing`). It uses the parent's provider and model in a one-shot,
no-tools, text-only mode, with the input on stdin, never argv. Its input is the
rendered history up to the anchor, starting from the last compaction summary
before the cut, with tool output truncated.

If summarizing fails, or the provider has no safe one-shot mode, the session
gets the bounded raw transcript tail plus a notice. It never fails the fork.

### 5. Checkpoints (code as of a message)
Before Fletch delivers a turn to the agent (`deliver_as_turn`), it snapshots
every checkout of the workspace. The snapshot is committed plus uncommitted
work, excluding gitignored files, as a commit whose parent is HEAD. It is
pinned at `refs/fletch/checkpoints/<turn_id>` in that checkout.

- The code **before T** is checkpoint(T). The code **through T** is
  checkpoint(the next turn in T's session), taken in the checkout of the
  workspace that ran it, or the live tree if T is the latest turn.
- Capture is best-effort and never fails or delays a send beyond the snapshot
  itself. The snapshot seeds a temporary index from the real one, so only
  changed files are re-hashed.
- No checkpoint is taken for: live-injected mid-turn messages (the agent is
  mid-edit), native PTY typing (no turn row), or workflow step agents
  (`owner_run_id`, which share a tree).
- Checkpoints live in the checkout, so they disappear when the checkout is
  deleted on archive. "Code as of this message" is then unavailable.
- A child receives a checkpoint by fetching its ref from the checkout that
  holds it and restoring it, in its checkout of the same repo: the tree is
  the snapshot's and HEAD the snapshot's parent, so commits stay commits and
  uncommitted work stays uncommitted. A fork of the current code pins the
  parent's live tree the same way, under a key of its own. The child drops
  its fetched copy once restored, and the spawn drops an on-demand pin from
  the parent however it ends; a turn's checkpoint is never dropped.
- A fork takes the code of every checkout of the source workspace or none of
  it: a checkout without the checkpoint, or a repo the child doesn't check
  out, fails the fork and names the repos. A repo the child checks out but
  the source never had keeps its clean base.

## Flows

### Fork `(parent, anchor?, code, context)`
1. Resolve the anchor to `(origin_session, cut_seq)`.
2. Create the child workspace and session. If `context != None`, set
   `parent_session_id = origin_session` and `parent_cut_seq = cut_seq`.
3. Code:
   - `Clean`: the parent's base branch.
   - `Current`: snapshot the parent now.
   - `AtMessage`: the checkpoint for "through T".
4. Context `Summary`: run the summarizer stage and store the summary on the
   child session.
5. Spawn the agent (fresh provider session).

### Rewind `(agent, turn T, conversation | code | both)`
- **Code:** in each checkout, restore checkpoint(T): files and HEAD. Confirm
  first if commits made after T would leave the branch, or were already pushed.
  The checked-out branch moves back with HEAD, and each checkout it changes is
  first pinned at `refs/fletch/undo/latest`, which keeps those commits
  reachable and makes the restore undoable; `refs/fletch/undo/restored` pins
  what it was restored to. The backend owns that one undo point per checkout:
  a new restore replaces it, an undo or a discard ends it, and otherwise it
  lasts until the next delivered turn, or until the code changes in any way,
  whichever comes first. An undo applies only while every restored checkout is
  exactly as the restore left it (HEAD and the working tree, ignored files
  aside), so it never discards newer work, whether a turn, the native view,
  the terminal or an editor made it.
- **Conversation:**
  1. Stop the agent and drop its queued messages.
  2. Create a new session in the same workspace with lineage
     `(session owning T, cut before T)`, and mark the old session superseded.
  3. Launch with `Exact` if the provider supports branching at a message,
     otherwise with `Summary`.
  4. Prefill the composer with T's text.
- **Both:** do both.

## Module map

**Backend** (`crates/fletch-core/src`)

| Module | Owns |
|---|---|
| `workspace/lineage.rs` | the `SessionLineage` a child session is created with, anchor → cut resolution, the stitched `read_history_records` / `read_history_turns`, `detach_children` |
| `workspace/sessions.rs` | the single current-session helper |
| `git/checkpoint.rs` | capture, fetch-into and restore, built on the snapshot primitive |
| `supervisor/checkpoints.rs` | capture for every checkout of an agent at turn delivery |
| `supervisor/fork.rs` | fork orchestration only |
| `supervisor/rewind.rs` | rewind orchestration only |
| `handoff/` | the summarizer: the one-shot runner (per-provider flags are `agent::OneShot` descriptors), the fallback tail |
| `agent` | `SessionStart { Fresh, Resume, Branch(BranchPoint) }` replaces `fresh: bool`; the `branch` capability per provider |

**Frontend** (`src`)

| Module | Owns |
|---|---|
| `adapters/handoff.ts` | the handoff transcript: the cut, the compaction start, tool caps and the input budget (was `store/forkDigest.ts`) |
| fork UI | sends `turn_id` anchors |
| rewind UI | a per-message action |

## Removed by this design
- Copying parent records into the child, and `snapshot_max_seq` /
  `carried_records`.
- The raw transcript digest in the system prompt.
- Prompt-ordinal anchors, and the `APP_ACTION_PREFIX` copy in Rust.
- Stamping the parent's task on the child to unlock the chat-history load.
- Every "latest session by `created_at`" query.

## Known limits (v1)
- Native PTY turns can't be anchors or checkpoints.
- `Exact` is Claude-only. Other providers rewind with `Summary`.
- Side effects outside the worktree (database changes, installs, pushes) are
  not rolled back.

## PR sequence

| PR | Scope | Depends on |
|---|---|---|
| 1 | Cap the current digest (hotfix) | none |
| 2 | Lineage, the current-session helper, fork by reference, `turn_id` anchors, `detach_children` | none |
| 3 | Checkpoint capture and the git/checkpoint API, no UI | none |
| 5 | `SessionStart::Branch` and the Claude branch capability | none |
| 4 | Handoff summarizer, the "code as of this message" fork option, fork UI cleanup | 1, 2, 3 |
| 6 | Rewind (conversation, code, both) | 2, 3, 4, 5 |
