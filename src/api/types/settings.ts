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
