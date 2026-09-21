-- A short, agent-written title for the work a workspace is doing ("Swap
-- sidebar title and prompt"). The agent sets it through the `set_title`
-- mailbox op once it knows what the user actually wants — which may not be
-- the first message — and may refine it as the purpose sharpens.
--
-- NULL until the agent titles the workspace; the sidebar then falls back to
-- the first line of `task`. Distinct from `name` (the random codename that
-- also names the branch) and from `task` (the verbatim first user message).
ALTER TABLE workspaces ADD COLUMN title TEXT;
