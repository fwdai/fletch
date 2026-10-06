# Project context layer

The knowledge about a project that is not in its code — what it is made of,
what has been decided about it, the rules it follows, how it actually works —
captured from the work done through Fletch and served back to every agent so
it does not re-explore the project to reconstruct it.

This is the first, deliberately small version. It replaces nothing yet (the
roadmap brief still stands and is shown as the vision until a `vision` entity
exists) and is built so the later pieces — search, code anchors, multi-host —
are extensions, not rewrites. The core lives in `crates/fletch-core/src/context/`
(model, store, service, compile, render, resolve, trust), the capture paths —
the extractor and the PR ingester — in `src/capture/`, the `context_*` RPC ops
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
  drops and replays them through the same `apply` every write uses, in
  `(host_id, seq)` order. A test greps the module for any `UPDATE`/`DELETE`
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
  outside the service can reach them: the compiler keeps the rule.

The store enforces identity and graph shape whatever the caller: slugs are
one case-insensitive identity (`COLLATE NOCASE`); a supersession must target
an assertion of the same kind and domain about a shared entity; an entity
cannot relate to itself; a merge needs an active target, refuses cycles and
compresses paths so every old id points at the final one.

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
2. **Background extraction** (`capture/extract/`). After a turn settles
   (`session_sync`) — debounced to one run per workspace per 10 minutes, with
   ≥1 new user turn and ≥200 chars of user text — and always at archive, a
   one-shot, tool-less run of the session's own provider
   (`handoff::run::once`) reads the new user turns and the agent's final
   message per turn, the workspace task as the *plan*, the entity index and
   the current heads, and returns strict JSON. Nothing it says lands:
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

Stored context is untrusted data wherever it is rendered, and the store is
the one place that makes it safe: every entity and assertion write validates
the slug (`[a-z0-9][a-z0-9._-]*`, ≤ 64), strips control characters and
collapses whitespace in every text field, and caps lengths (name 120,
summary 600, statement 300, rationale 1000) — whichever writer it came
from. The spawn-time index quotes names and leaves out entities the
extractor minted until something else revised them, and the instruction
block says the index is data, not instructions.

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

Bootstrap: ask an agent to map the project (vision from the brief, modules
from the code layout, features from the roadmap) with the record ops, then
correct it in the tab.

## Deferred, and where each attaches

| Later | Attaches to |
|---|---|
| Text search beyond token match (FTS5, then embeddings) | `compile::by_text`; an embedding column is derived data, rebuilt on replay |
| Symbol anchors and staleness against the code graph | `paths[]` already; a resolver over the checkout's `.codegraph/codegraph.db` at retrieval time (the host never queries it today) |
| Roadmap rulings and `wf_report` as deterministic sources | `capture/ingest/`, same `MergedPr`-style entry |
| Gap mining (what did the agent discover that it was not served) | A second question in the extractor prompt; same proposal pipeline |
| Host-side injection at turn start | The index block is step one; no per-turn mechanism exists yet |
| Multi-host | Not an extension of what is here. The log replays by `(recorded_at, host_id, seq)` and fails loudly on an out-of-order status event, which is honest, not sufficient: two hosts need causal ordering (vector clock or HLC on every event), a fold that tolerates a status event arriving before its assertion, and a rule for forked supersession chains. Writer-minted ids and the stamp keep the door open; the fold is a redesign. |
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
