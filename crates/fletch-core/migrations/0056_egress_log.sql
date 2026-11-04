-- Every connection a box's egress proxy answered: what agents reach, and what
-- they were refused. The Network lists read it grouped by host; it is also the
-- record an open posture leaves behind, where nothing else says what left the
-- VM. Rows older than the retention window are pruned at boot.
CREATE TABLE egress_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  -- Unix milliseconds, when the connection was answered (a tunnel: when it
  -- closed).
  at INTEGER NOT NULL,
  agent_id TEXT NOT NULL,
  project_id TEXT NOT NULL,
  host TEXT NOT NULL,
  port INTEGER NOT NULL,
  -- 1 for a tunnel, 0 for a refusal.
  allowed INTEGER NOT NULL,
  -- How it was answered: `listed` (on the allow-list), `open` (the open
  -- posture), `turn` (the network opened for a turn); `host`, `port` or
  -- `quiet` for a refusal.
  via TEXT NOT NULL,
  bytes_up INTEGER NOT NULL DEFAULT 0,
  bytes_down INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX egress_log_at ON egress_log(at);
CREATE INDEX egress_log_project_host ON egress_log(project_id, host, at);
