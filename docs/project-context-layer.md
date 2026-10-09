# Project context layer

The knowledge about a project that is not in its code — what it is made of,
what has been decided about it, the rules it follows, how it actually works —
captured from the work done through Fletch and served back to every agent so
it does not re-explore the project to reconstruct it.

This is the first, deliberately small version. It is the single source of
truth for project knowledge — it replaced the roadmap's product brief, and
every agent (coding agents, reviewers, the PM chat) is served the same
overview from it at spawn — and is built so the later pieces — search, code
anchors, multi-host — are extensions, not rewrites. The core lives in `crates/fletch-core/src/context/`
(model, store, service, compile, render, resolve, trust), the capture paths —
the bootstrap, the extractor and the PR ingester — in `src/capture/`, the `context_*` RPC ops
in `src/rpc/context/`, the host commands in `src/commands/context.rs`, and the
UI in `src/components/ProjectContext/`.

## Shape

```
sources ─► observation ─► extraction ─► proposal ─► confirmation ─► event ─► projection ─► compile ─► agent / UI
```

- **`context.db`**, attached to the main connection as schema `context`
  (`database::connection`, same pattern as `transcripts.db`; migrations in
  `migrations_context/`). Its own file because it is the one store in Fletch
  that is not regenerable from anything else.
- **`events`** is the source of truth and is insert-only. `entities`,
  `assertions` and the edge tables are projections; `ContextStore::rebuild_projection`
  drops and replays them through the same `apply` every write uses. Replay
  order is each host's `seq` order — always, whatever the wall clock did
  between two writes — with the per-host streams merged by `recorded_at`
  (`load_events`). A test greps the module for any `UPDATE`/`DELETE`
  against `events`.
- **Compile** (`compile.rs`) is a pure function from one project's `Graph` to
  the `Bundle` an agent or the UI reads. Actuality is decided there, not in
  storage: `compile::is_current` is the one definition of a current head
  (live, and nothing live further down its supersession chain) and is what
  the conflict check, the stats and the UI (`Graph.current`) all use.
  Queries are about *now*; an `as_of` view would need status intervals and
  is not offered.
- **Proposals** are candidates, not truth. They live outside the log and
  become events only when accepted — by rule (`resolve::auto_rule`) or by the
  user in the review queue.
- **One front door, one write policy.** Everything outside the module —
  agent ops, host commands, ingesters, the extractor — goes through
  `ContextService` (`service.rs`): it owns the gate (`open`), trust
  (`verify_user_quote`), settlement and proposal rulings, and hands every
  assertion to `ContextStore::land`, which classifies the candidate against
  what stands now and applies the rule for the writer's author kind *inside
  one transaction*: a user write lands; an agent write lands unless current
  heads exist about the same entities and domain and it named no relation
  (then the heads come back and nothing is written); an ingester write (a
  merged PR's lines, reviewed by a person before merging) lands next to what
  is there; an extractor write is always held. A restatement is a duplicate
  for everyone — except the very head a candidate explicitly revises. The
  store's write methods are visible only inside `context`, so nothing
  outside the service can reach them: the compiler keeps the rule. Every
  write that changes what a project's context says ends in one
  `context:changed` pulse from the service, whichever writer made it, and
  the event is on the remote whitelist — so a Context tab, local or paired,
  reloads on an agent's op, a merged PR or the extractor as it does on its
  own write.

The store enforces identity and graph shape whatever the caller: slugs are
one case-insensitive identity (`COLLATE NOCASE`); a record attaches to
active entities only — a merged subject stands for the entity it was merged
into, an archived one is refused, for an assertion's subjects, a relation's
ends and an entity revision alike (compile serves active entities, so
anything else would be invisible); a supersession must target an assertion
of the same kind and domain about a shared entity; an entity cannot relate
to itself; a merge needs an active target, refuses cycles and compresses
paths so every old id points at the final one.

## Data model (`model.rs`)

**Entity** — `id` (UUIDv7, writer-minted), `slug` (unique per project; what
agents reference), `kind` (`vision · goal · capability · feature · module · topic`),
`name`, `summary`, `aliases[]` (entity resolution), `paths[]` (file anchors),
`status` (`active · archived · merged`). Exactly one active `vision`.

**Assertion** — `id`, `kind` (`decision · constraint · fact`), `domain`
(`business · architectural · implementation`), `stance` (`adopted · rejected`),
`statement`, `rationale`, `valid_from`, `paths[]`, `status`
(`provisional · confirmed · abandoned · retracted`). Immutable once recorded;
only `status` moves, and only through `confirmed` / `abandoned` / `retracted`
events.

**Edges** — `about` (assertion → entity, ≥1), `supersedes` (new → old, with
required reasoning), `contradicts` (assertion ↔ assertion), `relates`
(entity → entity: `part_of · depends_on · serves`).

**Write stamp** on every event — `host_id` (minted once per data dir into
`settings`), `seq` (per-host monotonic, computed in the write transaction),
`recorded_at`, `author` (`user · agent · extractor · ingester` + agent id and
provider), `source` (`user_turn · agent_turn · pr · review_thread · roadmap ·
workflow · ui · repo` + reference), `provenance` (workspace, branch, commit,
session, turn).

**Project identity** — events are keyed by a *context* project id minted into
`project_settings` (`context.id`), never the host-local `projects.id`, so a
log can leave this host without rewriting a key.

### Why these distinctions

- **Supersede vs retract.** Changing a decision records a new assertion that
  supersedes the old one, with reasoning; history stays. Retract is for
  something that was never right; it is hidden from compile, kept in the log.
- **Decision vs fact.** A user-stated decision ("X should work like A") and an
  observed fact ("X works like B") disagreeing is *drift*, not supersession.
  Only a decision supersedes a decision and a fact a fact; across kinds the
  pipeline records a `contradicts` edge and the user's ruling closes it.
- **Provisional vs confirmed.** Anything an agent records from a checkout is
  provisional until that checkout's PR merges (`confirmed`) or the workspace
  is archived without it merging (`abandoned`). The unit is the checkout:
  `provenance.repo` names it, and `settle` touches only the records made in
  it, so one merged repo of a multi-repo workspace never confirms another's
  work. An agent in a multi-checkout workspace must say which checkout a
  decision is about (`repo`); with one checkout it is implied, and the
  extractor's records (about the conversation, not a repo) follow the
  primary. What the user says is confirmed on arrival.
- **Contradictions have a lifecycle.** A `contradicts` edge carries the
  reasoning it was recorded with and, once a person rules on it
  (`contradiction_resolved`), the ruling. Compile flags only unresolved
  edges whose other side still stands; the sides themselves change through
  retract or supersede like any other assertion.

## Capture

Three paths, one pipeline, rising cost:

1. **Deterministic** (`capture/ingest/`). The PR body's `## Decisions` section
   (format in `instructions/git_actions.md`) is parsed when the watcher sees
   the PR merge (`supervisor::pr_watch`) or at archive for a PR that merged
   while the host was down. Lines land confirmed with source `pr`, next to
   whatever is already said about those entities (a restatement is skipped
   as a duplicate). The merge also confirms the workspace's provisionals;
   archive-without-merge abandons them.

   The same merge appends its **structural delta** (`capture/ingest/structure.rs`),
   before the lines so one can name a module the PR added. The GitHub fetch
   that reads the body also reads the merge commit, and the PR's changed
   files (`pulls/{n}/files`). "After" is the merge commit's tree, listed in
   the project's primary repo (the one the bootstrap mapped; the commit is
   fetched into it when not local); "before" is that list with the PR's own
   changes reversed (`unapply`: added files out, removed files back, renames
   undone), so it is the PR alone for a merge commit, a squash and a rebase
   merge alike. The bootstrap's `rules::modules` over each gives the modules
   before and after, and `delta`, pure, matches them by slug: *added*,
   *removed*, and *moved* (same slug, new directory). Landed as the ingester
   with source `pr` and the merge SHA, against what the project has now: an
   added module is recorded like the bootstrap records one (paths,
   `part_of` from nesting, empty summary; a slug the project has ever had is
   skipped, so a directory that returns after its module was archived
   leaves it archived); a removed module's active `module` entity is
   archived; a moved one is revised with its path anchors rebased onto the
   new directory, every curated field kept. A PR that only touches files
   inside existing modules writes nothing, and the per-reference check makes
   a second announcement a no-op. A fetch or read that fails fails the
   PR's ingest before it is marked, so the next announcement (the watcher's
   tick, the archive) retries it; what did land is skipped. No model is
   involved, and a repo other than the primary is not read (its slugs would
   collide).
2. **Background extraction** (`capture/extract/`). After a turn settles
   (`session_sync`) — debounced to one run per workspace per 4 hours, with
   ≥1 new user turn and ≥200 chars of user text — and always at archive (the
   primary run, over the completed conversation), a
   one-shot, tool-less run of the session's own provider
   (`handoff::run::once`) reads the new user turns and the agent's final
   message per turn, the workspace task as the *plan*, the entity index and
   the current heads, and returns strict JSON. The pipeline keeps at most
   one entity and three assertions per run, and drops an
   implementation-domain assertion or one with no evidence quote found in
   the turns. Nothing it says lands:
   entities become pending entity proposals, assertions are held by
   `land` (a restatement is dropped as a duplicate), and an assertion about
   a proposed entity carries its slug as `about_pending` until that entity
   is accepted. The pilot gives model output no durable authority before its
   precision is known; the user's acceptance is what lands it, confirmed. The
   observation, the run (model, prompt version, raw output, timing, error)
   and every proposal are kept, so a run can be audited and re-extracted later.
3. **Explicit** (`rpc/context`). `context_record_decision` /
   `context_record_entity` / `context_link` for "the user asked you to
   remember this" and for the agent's own deviations from the plan. A
   record is user-stated only when Fletch itself finds the agent's
   `user_quote` (or the extractor's evidence quote) verbatim in a user turn
   of that workspace; a model's say-so never promotes its own words. When
   current decisions already exist about the same entities in that domain,
   nothing is written: the reply carries those heads and the agent resubmits
   with `supersedes`, `contradicts` or `coexists`.

Whether a statement *replaces* or *contradicts* a head is never decided from
text alone — two decisions about one entity are usually both true — so
`resolve::classify` only detects restatements; supersession and
contradiction come from the agent's or the model's explicit relation, and
`land` honours one only against a head that stands now. Who may land what
is the policy table on `Landing`; a user's acceptance in the review queue
confirms what it lands, and a ruling is scoped to the project the gate
opened.

## Reading

- `context_get` (any agent, any provider, any engine — it is a mailbox op):
  no args returns the map (vision, every active entity, their heads); with
  `entities` (slugs), `paths` and/or `query` it returns the named entities,
  their one-hop neighbours (each with its path anchors), the current
  assertions grouped by kind and domain (adopted before rejected), and
  warnings for anything provisional, contradicted or missing.
  `include_history` adds the supersession chain. Budgeted at 12k chars; the
  vision and warnings are never dropped.
- **Staleness** is computed at read time, never stored: `compile` takes the
  reader's checkout (the workspace's primary checkout for `context_get`, the
  project's primary repo for the Preview tab) and warns once per served path
  anchor — of an entity or an assertion — that the checkout does not have.
  The log is not touched; whoever reads the warning fixes the record.
- The spawn-time **overview** (`compile::overview`, ≤3000 chars,
  `rpc::context::spawn_overview`) rides in the instruction block of every
  session on a project with the layer on, whatever its purpose — a coding
  agent and the PM chat get the identical section. In order: the vision (or
  nothing), the adopted current `constraint` assertions in the business and
  architectural domains (statement only), a one-line legend per `module`
  (slug, clipped summary, first path), then every other active entity by kind
  as `slug ("name")`. It goes through `compile`'s ranking and budget loop and
  `render_markdown`; the vision and the constraints are never dropped, and a
  truncation warning tells the agent `context_get` has more. The Context
  tab's preview shows it verbatim (`CompileQuery.overview`). The extractor
  still reads the plain index (`render_index`).
- Every read is logged (`reads`: query, what was served, what was asked for
  and not found). Misses are the coverage signal.

## Human side

The whole layer sits behind a developer gate while it is piloted: the
host-owned `context_layer_enabled` setting (Settings › Developer › Project
context layer, opt-in; `set_context_layer_enabled` over the wire). Off, every
entry — ops, instruction block, ingesters, extractor — reads the setting and
does nothing, and the project page shows no Context tab. The gate is read
per operation, not per session: a `Project` the service resolved is a name,
and every agent op asks again (`ContextService::check`) while every write
reads the gate inside its own transaction (`ContextStore::write`), so an
agent spawned while the layer was on, or a PR ingestion that opened the
project before fetching the body, is refused the moment either switch is
turned off. The gate is keyed by the context id and requires an owner: an
id no project maps to is refused, never let through. Deleting a project
purges its whole context — every table, the log included — inside the
deletion's transaction (`context::purge_project`, from both deletion paths
in `workspace::repos`), and the mapping cascades with the project row, so
nothing still holding the project can read or write what was there. The
pipeline's bookkeeping (observations, extractor runs, the reads log) is
gated the same way, in its own transaction, the observation-keyed writes
through the observation's project — an extraction whose model call outlives
its project's deletion writes nothing — and an extractor run cascades from
its observation by foreign key.

Stored context is untrusted data wherever it is rendered, and the store is
the one place that makes it safe: every entity and assertion write validates
the slug (`[a-z0-9][a-z0-9._-]*`, ≤ 64), strips control characters and
collapses whitespace in every text field, and caps lengths (name 120,
summary 600, statement 300, rationale 1000) — whichever writer it came
from. The spawn-time overview quotes names and leaves out entities the
extractor minted until something else revised them, and the instruction
block fences it and says it is data, not instructions.

Trust is decided in one place too: `context::trust` finds the quoted words
verbatim in a user turn of that workspace (`find_user_quote`), and the
service turns that proof into the record — *the quote is the statement*,
the source is the user's turn, the status confirmed. A writer's own
sentence cannot ride on a user's quote: if it differs it is refused, and a
reading of the quote belongs in the rationale. Only the service can mint a
`user_turn` source; a record from an agent's or a model's turn is provisional
whatever the writer asked for. Tests
grep the crate to keep both rules: no other `user_turn` construction, and no
current-view decision on `Assertion::is_head` instead of
`compile::is_current`.

A **Context** tab on the project page: entities by kind with search; per
entity its relations and assertions with status, author/source and
contradiction badges and the actions *change* (supersede with reasoning),
*retract*, *history*, *edit* (revision), *archive*, *merge into*; a **review
queue** of pending proposals with their evidence and relation; a **preview**
of exactly what an agent would be served for a query. Two project toggles:
`context.enabled` (everything) and `context.extract` (the background run).
All of it goes through host ops (`context_*` in `remote::dispatch`), so a
remote-controlled host behaves the same.

A note for anyone who ran the gated feature before this branch merges:
`migrations_context/0001_context.sql` was edited in place (it had never
shipped), so a `context.db` created earlier must be deleted; it is recreated
empty on the next start.

## Pilot

Dogfood on this repo with the layer on. What to look at, all from SQL over
`context.*`:

- **Coverage**: `reads.misses` — what agents asked for that did not exist.
- **Exploration before first edit** per session, from the transcripts: does
  it fall for sessions that called `context_get`?
- **Pipeline health**: proposals by source and status, dismissal reasons
  (extractor precision), contradictions raised vs resolved, provisional →
  abandoned ratio.
- **Usage**: share of sessions with ≥1 `context_get`; `Stats` from
  `context_overview`.

## Bootstrap

The log is built once from the code at a commit and only appended to after
that; nothing rebuilds or re-syncs the base. Two tiers, both from the
Context tab's **Map project**:

1. **Structure, no model** (`capture/bootstrap/`, host op
   `context_bootstrap`). `derive` reads the tree of the project's primary
   repo at `HEAD` (`git ls-tree`, so the committed code, not the working
   tree); `rules::modules`, pure over a file list, turns it into module
   candidates: every package (a directory with a `Cargo.toml`,
   `package.json`, `pyproject.toml` or `go.mod`; the root one stands in as
   its `src/`), and every code directory directly under a package's `src/`,
   each anchored at its directory, nested ones `part_of` their parent.
   `apply` lands them through the service as the ingester, source `repo`
   with the commit SHA — entities and `part_of` relations only, never an
   assertion. A slug the project has ever had (archived and merged ones
   included) is skipped, not revised, so a second run writes nothing and
   never undoes a curation. After that, structure changes only through
   merged PRs (see Capture).
2. **Meaning, an ordinary agent session.** The op answers the canned task
   (`instructions/context_mapping.md`), which the tab opens as a new
   workspace draft: with codegraph and the record ops the agent writes the
   `vision` (≤ 600 chars), a one-line summary, a name and aliases for every
   module (a revision names the entity by its slug in `id`), and the missing
   `part_of` / `depends_on` relations. It records no decisions or facts —
   those come from the people who made them — with one exception: a project
   that still has a row in `roadmap_briefs` (the user-reviewed product brief,
   read with plain SQL so a dropped table is simply none) gets it appended to
   the task, fenced, and the session folds it into the vision and records the
   constraints it states, quoting the brief so they land as the user's.

## Deferred, and where each attaches

| Later | Attaches to |
|---|---|
| Text search beyond token match (FTS5, then embeddings) | `compile::by_text`; an embedding column is derived data, rebuilt on replay |
| Symbol anchors | `paths[]` already; a resolver over the checkout's `.codegraph/codegraph.db` at retrieval time (the host never queries it today), next to the path check in `compile` |
| Roadmap rulings and `wf_report` as deterministic sources | `capture/ingest/`, same `MergedPr`-style entry |
| Gap mining (what did the agent discover that it was not served) | A second question in the extractor prompt; same proposal pipeline |
| Host-side injection at turn start | The overview block is step one; no per-turn mechanism exists yet |
| Multi-host | Not an extension of what is here. The log replays each host in `seq` order and merges hosts by `recorded_at`, failing loudly on an out-of-order status event — honest for one host, not sufficient for two: they need causal ordering (vector clock or HLC on every event), a fold that tolerates a status event arriving before its assertion, and a rule for forked supersession chains. Writer-minted ids and the stamp keep the door open; the fold is a redesign. |

## Decisions made while building

- **SQLite, not SurrealDB.** Same `ContextStore` contract, no new engine, no
  BSL question, no RocksDB build on three release targets; shared mode later
  is log shipping, which the event log already supports.
- **UUIDv7, not ULID.** Same properties; the `uuid` crate is already in the tree.
- **Compute staleness at retrieval, store nothing per working tree.** The
  index lives in each checkout and the host is never told it rebuilt.
- **Background extraction over an in-line "remember" duty.** The main agent
  is not asked to remember; the extractor reads what it and the user wrote.
- **Archive-time extraction stamps the settled outcome** (`confirmed` /
  `abandoned`) rather than `provisional`, because the archive settles the
  workspace's provisionals before that run lands.
- **The extractor reuses the handoff one-shot runner** (tool-less, empty
  cwd, timeout) rather than a sandboxed agent run.
- **One overview for every agent, and no brief.** The roadmap's product brief
  (a PM-maintained markdown page, `roadmap_briefs`) was a stand-in for this
  layer; its RPC ops, commands, events and board tab are gone. The PM keeps
  only the board's "Not doing" digest in its roadmap block — the board's
  decision log, not project knowledge. The `roadmap_briefs` and
  `roadmap_brief_proposals` tables are retained as a read-only archive
  (nothing writes them; a test greps the crate) so an accepted brief can be
  imported into the vision and constraints under the user's review; a later
  release drops them after that import.
