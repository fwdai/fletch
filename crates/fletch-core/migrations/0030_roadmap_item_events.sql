-- Durable history for roadmap items: one row per lifecycle transition.
--
-- Until now the only record of *why* an item moved was transient — the
-- `roadmap:queue-note` event a card renders until the next tick, or a line in
-- the PM chat. A failed run's reason vanished on reload; "when did this ship?"
-- had no answer beyond `updated_at`, which any edit overwrites. This table is
-- the durable object every later slice hangs off: decision cards, PM oversight
-- digests, and `done_at` (the `shipped` event's timestamp).
--
-- Written exclusively by `roadmap::events::record`, in the same connection-lock
-- scope as the item write it describes — every status transition writes exactly
-- one event. `actor` is who moved the item (user | pm | drainer | sweep);
-- `kind` is the transition's name; `detail` is the human-readable payload
-- (a failure reason, a PR url, a workflow id) rendered on the card's history
-- line.
--
-- ON DELETE CASCADE on `item_id` is deliberate: a deleted item was ruled off
-- the board and needs no history, so its events go with it. `project_id` is
-- denormalized so event listeners can filter to a board without a join.
--
-- Deliberately NOT added to the generic CRUD allow-list (`database::validate`),
-- like `roadmap_items` itself: events must ride the typed write paths so they
-- can never disagree with the transition they record.
CREATE TABLE roadmap_item_events (
    id         TEXT PRIMARY KEY,            -- uuid
    item_id    TEXT NOT NULL REFERENCES roadmap_items(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL,
    actor      TEXT NOT NULL,               -- user | pm | drainer | sweep
    kind       TEXT NOT NULL,               -- see roadmap::events::EventKind
    detail     TEXT,                        -- human-readable; NULL when the kind says it all
    created_at INTEGER NOT NULL
);

-- Every read is "this item's history"; nothing queries events any other way.
CREATE INDEX idx_roadmap_item_events_item ON roadmap_item_events(item_id);
