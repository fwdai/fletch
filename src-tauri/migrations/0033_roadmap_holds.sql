-- Holds: the brake pedal on autonomous progress, at two scopes.
--
-- Why this exists: the queue drains on its own. Once an item is `queued` the
-- drainer claims it, launches a run, and opens a PR without anyone typing
-- anything — which is the point, and which means there has to be a way to say
-- "stop, we need to agree on direction first" that does not require the user to
-- be at the keyboard when the tick fires. Unqueueing is not that: it is a status
-- move, it loses the reason, and it is the user's alone. A hold is a *reason*
-- attached to a scope, and it is the one thing the PM agent may write directly —
-- because it can only ever *reduce* autonomy (invariant 2 in
-- .context/roadmap-pm-plan.md). Releasing is the user's alone; there is no RPC
-- op for it.
--
-- Item-level: three nullable columns rather than a table. A hold is one fact
-- about one row — the reason, who applied it, when — and every reader that cares
-- (the drainer's queue filter, the card's chip, the strip's card) already holds
-- the row. A side table would mean a join on the hottest read in the module for
-- a column trio. One hold at a time per item, by construction: a second hold
-- overwrites the reason, and the durable trail (`roadmap_item_events`, kinds
-- `held`/`released`) keeps both — the *current* reason lives here, the *history*
-- of holds lives there. That split is deliberate: this table answers "is it
-- held, and why", the trail answers "what has been held, by whom, and when".
--
-- `held_by` stores an `EventKind` actor spelling (`user` | `pm`), the same
-- vocabulary the history rows use, so "who stopped this" means one thing on both
-- sides. NULL exactly when `hold_reason` is NULL.
--
-- Project-level: its own table rather than a `project_settings` key, because a
-- hold has three fields and settings has one value column — a JSON blob in a
-- k/v row would be a schema nobody could query and a parse every reader would
-- have to repeat. One row per project (PRIMARY KEY), so holding an already-held
-- project replaces the reason exactly like the item case, and the CASCADE means
-- a deleted project cannot leave a hold behind that nothing can release.
--
-- Deliberately NOT added to the generic CRUD allow-list (`database::validate`),
-- like every roadmap table: a hold that didn't ride a typed write path could
-- exist with no `held` event explaining it, and the drainer would silently stop
-- dispatching for a reason no surface could show.
ALTER TABLE roadmap_items ADD COLUMN hold_reason TEXT;
ALTER TABLE roadmap_items ADD COLUMN held_by TEXT;
ALTER TABLE roadmap_items ADD COLUMN held_at INTEGER;

CREATE TABLE roadmap_project_holds (
    project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    reason     TEXT NOT NULL,              -- why the whole board is stopped
    held_by    TEXT NOT NULL,              -- user | pm, the EventActor spelling
    created_at INTEGER NOT NULL
);
