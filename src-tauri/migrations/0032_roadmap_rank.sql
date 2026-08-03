-- Explicit priority order for roadmap items, and the PM's pending ask to change it.
--
-- Until now the order was implicit: `(created_at, rowid)` was both the order the
-- board drew a horizon group in and the order the drainer dispatched in (see
-- `roadmap::drainer`). That made "build this one first" inexpressible — the only
-- way to move an item up the queue was to have created it earlier. `rank` makes
-- dispatch order an explicit, jointly owned fact: the user drags a card, the PM
-- proposes a whole new sequence, and both write the one column the board draws
-- and the drainer reads.
--
-- REAL rather than INTEGER because the order is maintained by *fractional
-- indexing*: a card dropped between two neighbours stores the midpoint of their
-- ranks, so one row is written and no other row moves. Integers would force a
-- renumbering of everything below every drop.
--
-- The backfill preserves exactly what the board showed before this migration:
-- 1.0, 2.0, … per project in `(created_at, rowid)` order, which is the order the
-- pre-0032 list query used. `DEFAULT 0` only covers the ALTER itself — the
-- UPDATE below rewrites every existing row, and new rows are allocated
-- `MAX(rank) + 1` per project by `roadmap::store::next_rank`.
ALTER TABLE roadmap_items ADD COLUMN rank REAL NOT NULL DEFAULT 0;

UPDATE roadmap_items
SET rank = (
    SELECT COUNT(*)
    FROM roadmap_items AS earlier
    WHERE earlier.project_id = roadmap_items.project_id
      AND (earlier.created_at < roadmap_items.created_at
           OR (earlier.created_at = roadmap_items.created_at
               AND earlier.rowid <= roadmap_items.rowid))
);

-- The PM's pending ask to reorder a whole board: one row per project, board
-- scoped rather than item scoped.
--
-- Deliberately not a `roadmap_proposals` row (0031): that table's `item_id NOT
-- NULL` FK is load-bearing — it is what makes an item's pending delta cascade
-- away with the item and what the `UNIQUE(item_id)` "one ask per item" rule is
-- built on. An order ask targets no single item; weakening the column to hold
-- one would cost more than a four-column table.
--
-- `codes_json` is the *complete* new order of the project's orderable items
-- (`proposed | open | queued`) — the op refuses a partial list, so the ask is
-- unambiguous: it IS the new backlog order, not a hint about part of one. The
-- user's ruling re-validates that set before applying, because items move on
-- their own (the drainer claims, the PM proposes more) while an ask is pending.
--
-- One pending ask per project, newer replaces older, same as an item's delta:
-- the user rules on the PM's current position, not a backlog of superseded ones.
--
-- Deliberately NOT added to the generic CRUD allow-list (`database::validate`),
-- like every roadmap table: an ask that didn't ride the validated RPC path could
-- name codes the ruling would refuse.
CREATE TABLE roadmap_order_proposals (
    project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    codes_json TEXT NOT NULL,               -- JSON array of item codes, in the proposed order
    note       TEXT,                        -- the PM's one-line rationale
    created_at INTEGER NOT NULL
);
