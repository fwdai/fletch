-- How a turn ended, as the backend saw it: 'completed' (it ran to its natural
-- end), 'interrupted' (the user stopped it), or 'failed' (it errored, or its
-- message was abandoned before it ever ran). Written once, when the turn closes
-- (`mark_user_turn_ended`) or its queued message is dropped. NULL while the
-- turn is in flight or waiting to be delivered, and for a row written before
-- this column existed.
ALTER TABLE session_user_turns ADD COLUMN outcome TEXT;
