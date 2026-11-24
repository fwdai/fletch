-- A session ran in one of two views: the structured chat (`custom`) or the
-- provider's own TUI in a PTY (`native`). The native view is gone, so every
-- session runs the one way and the column says nothing. A session left in
-- `native` simply opens as chat: both views shared one provider session.
ALTER TABLE sessions DROP COLUMN view;
