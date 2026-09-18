-- Optional model chosen when the agent session was created. NULL keeps the
-- provider CLI default for older sessions and drafts where the user did not
-- pick a specific model.
ALTER TABLE sessions ADD COLUMN model TEXT;
