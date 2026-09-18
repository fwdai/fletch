-- Per-repo display label for multi-repo projects ("Frontend", "Gateway", …).
-- NULL falls back to the folder basename in the UI. Independent of the
-- project's own name, which labels the whole group.
ALTER TABLE repos ADD COLUMN label TEXT;
