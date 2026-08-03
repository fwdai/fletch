-- Pending PM deltas against existing roadmap items: one row per outstanding ask.
--
-- The PM proposes deltas, the user rules on them (invariant 2 of the roadmap
-- plan): a new ticket lands as a `proposed` item the user accepts or discards,
-- but a *revision* to an item that already exists — a retitle, a re-slice, a
-- retirement — can't be a new row without duplicating the item. This table is
-- that pending delta: `kind = 'update'` carries the patch the PM wants applied
-- (`patch_json`, only the reshapeable fields — never status/code/source), and
-- `kind = 'discard'` asks for the item's removal (`patch_json` NULL). `note` is
-- the PM's one-line rationale, quoted on the card and folded into the history
-- event the user's ruling writes.
--
-- UNIQUE(item_id) is the model: at most one pending proposal per item. The PM
-- changing its mind replaces the ask rather than queueing a second one — the
-- user rules on the PM's *current* position, not a backlog of superseded ones.
-- ON DELETE CASCADE because an ask about a row that's gone is about nothing.
--
-- Deliberately NOT added to the generic CRUD allow-list (`database::validate`),
-- like every roadmap table: proposals are written by the PM's RPC ops and ruled
-- on by the typed commands, and both sides validate what a raw insert would not.
CREATE TABLE roadmap_proposals (
    id         TEXT PRIMARY KEY,            -- uuid, stable across replacements
    project_id TEXT NOT NULL,
    item_id    TEXT NOT NULL UNIQUE REFERENCES roadmap_items(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,               -- update | discard
    patch_json TEXT,                        -- validated patch; NULL for discard
    note       TEXT,                        -- the PM's one-line rationale
    created_at INTEGER NOT NULL
);
