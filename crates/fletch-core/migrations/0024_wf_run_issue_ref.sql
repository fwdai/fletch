-- The GitHub issue a workflow run was started from, captured when the user
-- hits "Start work" on a Home-inbox issue and launches a Pipeline (rather than
-- a Quick agent). Stored as the bare issue number (text) for the run's repo —
-- enough to append a `Closes #<n>` trailer to the PR the run's finalize path
-- opens, so merging that PR closes the originating issue.
--
-- NULL for a normal launch that didn't originate from an issue. Mirrors
-- `workspaces.issue_ref` (migration 0023) for the pipeline path.
ALTER TABLE wf_run ADD COLUMN issue_ref TEXT;
