-- The provider account (a Fletch-managed config dir under
-- ~/.fletch/accounts/<provider>/<id>, see agent::accounts) the agent was
-- stamped with at creation. Reused on every later process spawn, like
-- sandbox_engine, so switching the active account never moves an existing
-- agent to another login. NULL = the CLI's own default account, which every
-- agent created before accounts existed ran under.
ALTER TABLE workspaces ADD COLUMN provider_account TEXT;
