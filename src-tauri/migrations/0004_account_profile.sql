-- Capture structured profile fields on the single local account row.
-- `name` is kept as a derived "First Last" string for existing consumers;
-- `email` and `avatar_url` already exist from the initial schema.
ALTER TABLE accounts ADD COLUMN first_name TEXT;
ALTER TABLE accounts ADD COLUMN last_name TEXT;
