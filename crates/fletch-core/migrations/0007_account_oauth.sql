-- Link the single local account to an external identity provider.
ALTER TABLE accounts ADD COLUMN oauth_provider TEXT;
ALTER TABLE accounts ADD COLUMN oauth_id TEXT;
