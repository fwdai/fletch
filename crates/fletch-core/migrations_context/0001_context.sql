-- context.db: the project context layer (src/context/).
--
-- Project knowledge that is not in the code: the entities a product is made
-- of, the decisions, constraints and facts asserted about them, and where
-- each came from. Its own file, like transcripts.db, because this is the one
-- store in Fletch that is not regenerable from anything else and the one a
-- second host will later need to sync as a unit.
--
-- `events` is the source of truth and is insert-only: no code path may UPDATE
-- or DELETE a row (a test greps for it). Every other table here is either a
-- projection replayable from `events` (`entities`, `assertions`, the edge
-- tables) or operational state around the pipeline that feeds it
-- (`observations`, `extractor_runs`, `proposals`, `reads`).
--
-- `project_id` everywhere is the *context* project id minted into
-- `project_settings` (`context.id`), never the host-local `projects.id`, so a
-- log can leave this host without rewriting a key.

CREATE TABLE events (
    id          TEXT PRIMARY KEY,        -- UUIDv7, minted by the writer
    project_id  TEXT NOT NULL,
    host_id     TEXT NOT NULL,           -- the writing host (settings `context.host_id`)
    seq         INTEGER NOT NULL,        -- per-host monotonic, never reused
    recorded_at INTEGER NOT NULL,        -- wall clock millis, informational
    author      TEXT NOT NULL,           -- JSON `Author`
    source      TEXT NOT NULL,           -- JSON `Source`
    provenance  TEXT NOT NULL,           -- JSON `Provenance`
    type        TEXT NOT NULL,           -- the `EventPayload` tag
    payload     TEXT NOT NULL,           -- JSON `EventPayload`, carries `v`
    UNIQUE (host_id, seq)
);
-- Replay order: wall clock, then (host, seq) to break ties.
CREATE INDEX idx_events_project_order ON events(project_id, recorded_at, host_id, seq);

-- Projection: the nouns.
CREATE TABLE entities (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL,
    slug        TEXT NOT NULL COLLATE NOCASE,  -- one identity per spelling, whatever the case
    kind        TEXT NOT NULL,           -- vision | goal | capability | feature | module | topic
    name        TEXT NOT NULL,
    summary     TEXT NOT NULL,
    aliases     TEXT NOT NULL DEFAULT '[]',  -- JSON array of strings
    paths       TEXT NOT NULL DEFAULT '[]',  -- JSON array of repo-relative paths
    status      TEXT NOT NULL DEFAULT 'active', -- active | archived | merged
    merged_into TEXT,
    recorded_at INTEGER NOT NULL,        -- of the latest revision
    author      TEXT NOT NULL,           -- JSON, of the latest revision
    source      TEXT NOT NULL,           -- JSON, of the latest revision
    UNIQUE (project_id, slug)
);
CREATE INDEX idx_entities_project ON entities(project_id, kind);

-- Projection: statements about entities. Immutable once recorded; only
-- `status` moves, and only through confirmed / abandoned / retracted events.
CREATE TABLE assertions (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL,
    kind        TEXT NOT NULL,           -- decision | constraint | fact
    domain      TEXT NOT NULL,           -- business | architectural | implementation
    stance      TEXT NOT NULL,           -- adopted | rejected
    statement   TEXT NOT NULL,
    rationale   TEXT NOT NULL,
    valid_from  INTEGER NOT NULL,
    paths       TEXT NOT NULL DEFAULT '[]',
    status      TEXT NOT NULL,           -- provisional | confirmed | abandoned | retracted
    recorded_at INTEGER NOT NULL,
    author      TEXT NOT NULL,           -- JSON
    source      TEXT NOT NULL,           -- JSON
    provenance  TEXT NOT NULL            -- JSON
);
CREATE INDEX idx_assertions_project ON assertions(project_id, status);

-- Edge projections. `about`, `supersedes` and `contradicts` are never removed
-- (assertions are immutable); `relates` rows come and go with linked /
-- unlinked events.
CREATE TABLE about (
    assertion_id TEXT NOT NULL,
    entity_id    TEXT NOT NULL,
    PRIMARY KEY (assertion_id, entity_id)
);
CREATE INDEX idx_about_entity ON about(entity_id);

CREATE TABLE supersedes (
    new_id    TEXT NOT NULL,
    old_id    TEXT NOT NULL,
    reasoning TEXT NOT NULL,
    PRIMARY KEY (new_id, old_id)
);
CREATE INDEX idx_supersedes_old ON supersedes(old_id);

CREATE TABLE contradicts (
    a_id                TEXT NOT NULL,
    b_id                TEXT NOT NULL,
    reasoning           TEXT,
    -- A ruling closes the tension without touching either side; the sides
    -- change through retract / supersede like any other assertion.
    resolved_at         INTEGER,
    resolution          TEXT,            -- the ruling's reasoning
    resolved_by         TEXT,            -- JSON `Author`
    PRIMARY KEY (a_id, b_id)
);

CREATE TABLE relates (
    from_id TEXT NOT NULL,
    to_id   TEXT NOT NULL,
    rel     TEXT NOT NULL,               -- part_of | depends_on | serves
    PRIMARY KEY (from_id, to_id, rel)
);
CREATE INDEX idx_relates_to ON relates(to_id);

-- Pipeline state. Not part of the log: a proposal is a candidate, not truth,
-- and becomes events only when accepted (by rule or by the user).
CREATE TABLE observations (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL,
    source       TEXT NOT NULL,          -- JSON `Source`
    provenance   TEXT NOT NULL,          -- JSON `Provenance`
    input_hash   TEXT NOT NULL,          -- of the text handed to extraction
    plan         TEXT,                   -- the workspace task / roadmap item, for deviation detection
    created_at   INTEGER NOT NULL,
    extracted_at INTEGER
);
CREATE INDEX idx_observations_project ON observations(project_id, created_at);

CREATE TABLE extractor_runs (
    id             TEXT PRIMARY KEY,
    observation_id TEXT NOT NULL,
    model          TEXT NOT NULL,
    prompt_version TEXT NOT NULL,
    output         TEXT,                 -- raw model output
    tokens_in      INTEGER,
    tokens_out     INTEGER,
    duration_ms    INTEGER,
    error          TEXT,
    created_at     INTEGER NOT NULL
);
CREATE INDEX idx_extractor_runs_observation ON extractor_runs(observation_id);

CREATE TABLE proposals (
    id             TEXT PRIMARY KEY,
    project_id     TEXT NOT NULL,
    observation_id TEXT,
    payload        TEXT NOT NULL,        -- JSON `ProposalPayload`
    evidence       TEXT NOT NULL DEFAULT '[]', -- JSON array of `Evidence`
    status         TEXT NOT NULL,        -- pending | auto | accepted | dismissed
    dismiss_reason TEXT,                 -- wrong | trivial | duplicate | already_known
    created_at     INTEGER NOT NULL,
    ruled_at       INTEGER,
    ruled_by       TEXT                  -- JSON `Author`
);
CREATE INDEX idx_proposals_project_status ON proposals(project_id, status);

-- The retrieval log: every `context_get`, what it served and what it missed.
CREATE TABLE reads (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL,
    agent_id     TEXT,
    workspace_id TEXT,
    session_id   TEXT,
    query        TEXT NOT NULL,          -- JSON `CompileQuery`
    served       TEXT NOT NULL,          -- JSON { entities: [...], assertions: [...] }
    misses       TEXT NOT NULL DEFAULT '[]', -- JSON array of requested-but-unknown refs
    chars        INTEGER NOT NULL,
    created_at   INTEGER NOT NULL
);
CREATE INDEX idx_reads_project ON reads(project_id, created_at);
