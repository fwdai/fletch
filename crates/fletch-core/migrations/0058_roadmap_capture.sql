-- Inbox and triage: capture lands untriaged, the PM turns it into rulings.
--
-- `status` gains the `captured` spelling with no schema change of its own — it
-- is plain TEXT with no CHECK (see 0035/0037). A captured row sits in the
-- board's Inbox, outside the horizons, the orderable set and the queue, until
-- the PM's `roadmap_triage` classifies it.
--
-- `kind` is that classification (`ask | idea | brainstorm`), NULL while
-- untriaged. `triage_json` is what the classification carried, as JSON:
-- `{brief?, questions?: [..], mined?: true}`. `from_item` links a row a triage
-- proposed back to the captured row it came from; deleting that captured row
-- clears the link rather than taking the proposal with it.
--
-- Plain ADD COLUMNs, like 0036/0037: nullable, no default, O(1). The
-- self-reference needs a NULL default to be addable, which it has.
ALTER TABLE roadmap_items ADD COLUMN kind TEXT;
ALTER TABLE roadmap_items ADD COLUMN triage_json TEXT;
ALTER TABLE roadmap_items ADD COLUMN from_item TEXT REFERENCES roadmap_items(id) ON DELETE SET NULL;
