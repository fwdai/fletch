-- The device a project belongs to, for a project this host only runs.
--
-- A project is owned by the device it was added on (its home), which is the
-- only one that edits it. Another device that is asked to run one of its
-- workspaces keeps a copy: the same project id, clones of its repositories,
-- and these two columns saying whose it is. Both are NULL for a project this
-- host owns, which is every row from before this migration.
--
-- `home_device` is the home's host id (its public key, base64url), the id a
-- client already knows that device by. `home_name` is what the home called
-- itself when the copy was last refreshed, for a row shown while the home is
-- out of reach.
ALTER TABLE projects ADD COLUMN home_device TEXT;
ALTER TABLE projects ADD COLUMN home_name TEXT;

-- A repository of such a copy. `home_id` is the id the repository has on the
-- project's home, which is what tells one repository from another when the
-- home relabels or removes one; the row's own `id` stays this host's. NULL for
-- a repository of a project this host owns.
ALTER TABLE repos ADD COLUMN home_id TEXT;
