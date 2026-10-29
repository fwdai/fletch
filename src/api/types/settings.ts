/** A host-owned global setting was written (`settings:changed`). `value` is
 *  the stored string, `null` for a deleted row. Only ever fired for a key on the
 *  host's allowlist (docs/remote-protocol.md, "Settings"). */
export interface SettingsChangedEvent {
  key: string;
  value: string | null;
}

/** One project's client-writable setting was written
 *  (`project_settings:changed`); `value: null` is a deleted row, i.e. back to
 *  the key's default. */
export interface ProjectSettingsChangedEvent {
  project_id: string;
  key: string;
  value: string | null;
}

/** One of a project's settings files (`.fletch/settings.json`,
 *  `.fletch/settings.local.json`) as last read on the host. */
export interface ProjectFileReport {
  /** Repo-relative path. */
  path: string;
  exists: boolean;
  /** Why nothing in it applies: unreadable, too large, not JSON. */
  error: string | null;
  /** What was skipped while the rest applied. */
  warnings: string[];
}

/** `project_file_status`: what a project's settings files say, read from its
 *  primary repo on the host, and what of it applies. Host patterns the files
 *  add under `egress.allow` apply only once the user approved them. */
export interface ProjectFileStatus {
  project_id: string;
  repo_path: string | null;
  /** Why the repository was not read at all. */
  error: string | null;
  files: ProjectFileReport[];
  egress: { applied: string[]; pending: string[] };
}

/** `project_file:changed`: reload `project_file_status`. */
export interface ProjectFileChangedEvent {
  project_id: string;
}
