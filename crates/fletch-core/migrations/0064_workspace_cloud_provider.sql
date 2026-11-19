-- The cloud provider whose machine a box-stamped workspace's box is, as
-- `cloud::ProviderId::as_str` spells it (sandbox::boxes::cloud). Stamped at
-- creation from the spawn's workspace type and never changed, like
-- sandbox_engine, which stays 'box' for it. NULL = a box on this machine, or
-- a workspace of another engine.
ALTER TABLE workspaces ADD COLUMN cloud_provider TEXT;
