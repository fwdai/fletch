-- Product memory: the brief the PM keeps about a project, and the PM's pending
-- ask to change it.
--
-- Why this exists: the board says what will be built; nothing said *why the
-- product is the way it is*. Vision, the domains the codebase actually has, the
-- constraints that rule out the obvious answer, the directions already rejected —
-- all of that lived in whichever PM chat happened to be open, and died with it.
-- Every new session then re-litigated decisions the user had already made, which
-- is the single most expensive failure mode of a planning agent.
--
-- Why it is deliberately this small: real product memory is a research problem
-- (what to remember, how to keep it true, how to retrieve the relevant slice).
-- This is the *seam* for it, not the answer. One markdown document per project is
-- the most naive implementation that is still honest, and it is behind three
-- stable surfaces (see src-tauri/src/roadmap/memory.rs): load-for-injection, one
-- proposal-gated write, one render payload. A future memory system replaces the
-- rows and the module internals without touching a caller.
--
-- One row per project (PRIMARY KEY), so writing the brief replaces it in place:
-- the document *is* the memory, and versioning it here would invent a history
-- nothing reads (the durable trail of *decisions* is `roadmap_item_events`, and
-- the proposal table below is where an unruled change waits). CASCADE, because a
-- brief about a deleted project is a document nothing can reach.
--
-- The proposal table is the same shape for the same reason the order ask is
-- (`roadmap_order_proposals`, migration 0032): the brief is board-scoped, so a
-- pending change to it belongs to no item and cannot ride the item-scoped
-- `roadmap_proposals` table whose `item_id NOT NULL` is what makes "one ask per
-- item" mean anything. One pending ask per project, replaced by a newer one, and
-- applied only by the user's ruling — the PM may propose its own memory, never
-- commit it. That is invariant 2 (.context/roadmap-pm-plan.md) applied to memory:
-- a brief the agent could rewrite silently is a place for it to talk itself into
-- something the user never agreed to.
--
-- Both tables are deliberately NOT on the generic CRUD allow-list
-- (`database::validate`), like every roadmap table: the accept path has to write
-- the brief, delete the ask, and emit both in one lock scope, and a frontend
-- `db_insert` would skip all three.
CREATE TABLE roadmap_briefs (
    project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    content    TEXT NOT NULL,              -- the brief itself, markdown
    updated_at INTEGER NOT NULL            -- when the user last ruled one in
);

CREATE TABLE roadmap_brief_proposals (
    project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    content    TEXT NOT NULL,              -- the whole proposed brief, markdown
    note       TEXT,                       -- the PM's one line on what changed
    created_at INTEGER NOT NULL
);
