-- The GitHub issue a workspace was started from, captured when the user hits
-- "Start work" on a Home-inbox issue. Stored as the bare issue number (text)
-- for the workspace's primary repo — enough to append a `Closes #<n>` trailer
-- to the PR the agent opens, so merging that PR closes the originating issue.
--
-- NULL for a normal spawn that didn't originate from an issue. Only the
-- primary repo's PR carries the trailer (the issue lives in that repo).
ALTER TABLE workspaces ADD COLUMN issue_ref TEXT;
