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
  /** This file's `egress.allow`. */
  allow: string[];
  /** This file's `egress.disable`. */
  disable: string[];
  /** This file's `seed` section. */
  seed: ProjectFileSeed;
}

/** What one settings file says of the project's starting disk (`seed`). */
export interface ProjectFileSeed {
  /** `seed.enabled`; null when the file does not say. */
  enabled: boolean | null;
  /** `seed.exclude`: paths inside a checkout, as the host spells them. */
  exclude: string[];
  /** The file's warnings about the `seed` section; also in its `warnings`. */
  warnings: string[];
}

/** The project's starting disk as both settings files set it. */
export interface SeedSettings {
  /** The local file's word over the team's; on when neither says. */
  enabled: boolean;
  /** Both files' exclusions, sorted. */
  exclude: string[];
}

/** Which settings file an edit goes to: the team's `.fletch/settings.json`
 *  or the user's own `.fletch/settings.local.json`. */
export type ProjectFileWhich = "shared" | "local";

/** An edit to one settings file's `egress` lists and `seed` section. */
export interface ProjectFileEdit {
  allowAdd?: string[];
  allowRemove?: string[];
  disableAdd?: string[];
  disableRemove?: string[];
  /** Writes `seed.enabled`; null takes it out of the file. */
  seedEnabled?: boolean | null;
  seedExcludeAdd?: string[];
  seedExcludeRemove?: string[];
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
  /** `disabled`: inherited patterns the files turn off. */
  egress: { applied: string[]; pending: string[]; disabled: string[] };
  seed: SeedSettings;
}

/** `project_file:changed`: reload `project_file_status`. */
export interface ProjectFileChangedEvent {
  project_id: string;
}
