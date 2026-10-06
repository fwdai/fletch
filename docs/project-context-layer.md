# Project context layer

The knowledge about a project that is not in its code — what it is made of,
what has been decided about it, the rules it follows, how it actually works —
captured from the work done through Fletch and served back to every agent so
it does not re-explore the project to reconstruct it.

This is the first, deliberately small version. It replaces nothing yet (the
roadmap brief still stands and is shown as the vision until a `vision` entity
exists) and is built so the later pieces — search, code anchors, multi-host —
are extensions, not rewrites. Everything lives in `crates/fletch-core/src/context/`,
the `context_*` RPC ops in `src/rpc/context/`, the host commands in
`src/commands/context.rs`, and the UI in `src/components/ProjectContext/`.

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
  drops and replays them through the same `apply` every write uses, in
  `(host_id, seq)` order. A test greps the module for any `UPDATE`/`DELETE`
  against `events`.
- **Compile** (`compile.rs`) is a pure function from one project's `Graph` to
  the `Bundle` an agent or the UI reads. Actuality is decided there, not in
  storage: heads, provisional flags, contradictions, misses, budget.
- **Proposals** are candidates, not truth. They live outside the log and
  become events only when accepted — by rule (`resolve::auto_rule`) or by the
  user in the review queue.

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
brief · workflow · ui` + reference), `provenance` (workspace, branch, commit,
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
- **Provisional vs confirmed.** Anything an agent records from a workspace is
  provisional until that workspace's PR merges (`confirmed`) or it is
  archived without merging (`abandoned`). What the user says is confirmed on
  arrival. Branch drift is solved by construction.

## Capture

Three paths, one pipeline, rising cost:

1. **Deterministic** (`ingest/`). The PR body's `## Decisions` section
   (format in `instructions/git_actions.md`) is parsed when the watcher sees
   the PR merge (`supervisor::pr_watch`) or at archive for a PR that merged
   while the host was down. Lines land confirmed with source `pr`, next to
   whatever is already said about those entities (a restatement is skipped
   as a duplicate). The merge also confirms the workspace's provisionals;
   archive-without-merge abandons them.
2. **Background extraction** (`extract/`). After a turn settles
   (`session_sync`) — debounced to one run per workspace per 10 minutes, with
   ≥1 new user turn and ≥200 chars of user text — and always at archive, a
   one-shot, tool-less run of the session's own provider
   (`handoff::run::once`) reads the new user turns and the agent's final
   message per turn, the workspace task as the *plan*, the entity index and
   the current heads, and returns strict JSON. Each item is resolved against
   existing entities; the model's own `relation` (supersedes / contradicts a
   named head) is kept when its target is a live head, a restatement is
   dropped as a duplicate (`resolve::classify`), and the result either lands
   (`auto_rule`) or waits in the review queue. The
   observation, the run (model, prompt version, raw output, timing, error)
   and every proposal are kept, so a run can be audited and re-extracted later.
3. **Explicit** (`rpc/context`). `context_record_decision` /
   `context_record_entity` / `context_link` for "the user asked you to
   remember this" and for the agent's own deviations from the plan. When
   current decisions already exist about the same entities in that domain,
   nothing is written: the reply carries those heads and the agent resubmits
   with `supersedes`, `contradicts` or `coexists`.

Whether a statement *replaces* or *contradicts* a head is never decided from
text alone — two decisions about one entity are usually both true — so
`resolve::classify` only detects restatements; supersession and
contradiction come from the agent's or the model's explicit relation. The
rule table (`resolve::auto_rule`): `new` / `confirms` / `duplicate` land
without review (duplicates are dropped); `supersedes` lands unless the target
is user-stated (source `user_turn` or `ui`) and the candidate is not;
`contradicts` never lands without a ruling. A user's acceptance in the review
queue confirms what it lands.

## Reading

- `context_get` (any agent, any provider, any engine — it is a mailbox op):
  no args returns the map (vision, every active entity, their heads); with
  `entities` (slugs), `paths` and/or `query` it returns the named entities,
  their one-hop neighbours, the current assertions grouped by kind and
  domain (adopted before rejected), and warnings for anything provisional,
  contradicted or missing. `include_history` adds the supersession chain.
  Budgeted at 12k chars; the vision and warnings are never dropped.
- The spawn-time **index** (`render_index`, ≤1500 chars) — every entity by
  kind — rides in the instruction block so an agent knows what to ask for.
- Every read is logged (`reads`: query, what was served, what was asked for
  and not found). Misses are the coverage signal.

## Human side

The whole layer sits behind a developer gate while it is piloted: the
host-owned `context_layer_enabled` setting (Settings › Developer › Project
context layer, opt-in; `set_context_layer_enabled` over the wire). Off, every
entry — ops, instruction block, ingesters, extractor — reads the setting and
does nothing, and the project page shows no Context tab.

A **Context** tab on the project page: entities by kind with search; per
entity its relations and assertions with status, author/source and
contradiction badges and the actions *change* (supersede with reasoning),
*retract*, *history*, *edit* (revision), *archive*, *merge into*; a **review
queue** of pending proposals with their evidence and relation; a **preview**
of exactly what an agent would be served for a query. Two project toggles:
`context.enabled` (everything) and `context.extract` (the background run).
All of it goes through host ops (`context_*` in `remote::dispatch`), so a
remote-controlled host behaves the same.

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

Bootstrap: ask an agent to map the project (vision from the brief, modules
from the code layout, features from the roadmap) with the record ops, then
correct it in the tab.

## Deferred, and where each attaches

| Later | Attaches to |
|---|---|
| Text search beyond token match (FTS5, then embeddings) | `compile::by_text`; an embedding column is derived data, rebuilt on replay |
| Symbol anchors and staleness against the code graph | `paths[]` already; a resolver over the checkout's `.codegraph/codegraph.db` at retrieval time (the host never queries it today) |
| Roadmap rulings and `wf_report` as deterministic sources | `ingest/`, same `MergedPr`-style entry |
| Gap mining (what did the agent discover that it was not served) | A second question in the extractor prompt; same proposal pipeline |
| Host-side injection at turn start | The index block is step one; no per-turn mechanism exists yet |
| Multi-host | Exchange `events` rows by `(host_id, seq)` watermark; no shared database needed |
| Replacing the roadmap brief | A `vision` entity takes over the moment one exists |

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
