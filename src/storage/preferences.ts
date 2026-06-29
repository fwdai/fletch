import { DEFAULT_PROVIDER_ID } from "../data/providers";

// Typed app-preference parsers: turn the flat string→string settings blob read
// by ./settings.ts into the structured values the store holds. Kept out of the
// store so the migration/clamping logic lives next to the persistence layer it
// belongs to and can be unit-tested in isolation.

// ---- Appearance & feature-flag types -----------------------------------------

export type ThemeMode = "dark" | "light";
export type Density = "comfortable" | "compact";
export type WorkspaceView = "custom" | "native";
export type SettingsSection =
  | "general"
  | "account"
  | "providers"
  | "agents"
  | "workflows"
  | "experimental"
  | "developer";

/** One-shot deep-link intent handed to a settings pane when it opens. */
export type SettingsIntent = "new-custom-agent";

export interface FeatureFlags {
  git: boolean;
  /** The unified Code panel: file explorer/editor + the Live diff feed. */
  code: boolean;
  run: boolean;
  terminal: boolean;
  thinkingBudget: boolean;
  autoEdit: boolean;
  /** Show the context-window usage meter in the composer foot. */
  tokenUsage: boolean;
  /** Experimental: expose the Custom/Native view switcher so agents can be
   *  driven through the provider's own terminal UI. Off by default — native
   *  mode isn't equally solid across providers yet. */
  nativeView: boolean;
}

export const DEFAULT_FEATURES: FeatureFlags = {
  git: true,
  code: true,
  run: false,
  terminal: false,
  thinkingBudget: true,
  autoEdit: false,
  tokenUsage: true,
  nativeView: false,
};

export function parseFeatures(raw: string | undefined): FeatureFlags {
  if (!raw) return DEFAULT_FEATURES;
  try {
    const saved = JSON.parse(raw) as Partial<FeatureFlags> & {
      // legacy flags folded into `code`
      files?: boolean;
      diff?: boolean;
      // removed in this version; its presence marks a pre-migration blob
      statusBar?: boolean;
    };
    // The old "Files" and "Diff" tabs were merged into the Code panel; honor a
    // saved preference for either when migrating an existing settings blob.
    const legacyCode =
      saved.code ??
      (saved.files !== undefined || saved.diff !== undefined
        ? !!(saved.files || saved.diff)
        : undefined);
    // A blob still carrying the removed `statusBar` flag predates `tokenUsage`
    // gating the composer meter — back then it was a no-op that defaulted off.
    // Drop its stored `tokenUsage` so the new default (meter on, matching the
    // old always-visible behavior) applies; honor the value for newer blobs.
    const preMigration = saved.statusBar !== undefined;
    const { files: _files, diff: _diff, statusBar: _statusBar, ...rest } = saved;
    void _files;
    void _diff;
    void _statusBar;
    if (preMigration) delete rest.tokenUsage;
    return {
      ...DEFAULT_FEATURES,
      ...rest,
      ...(legacyCode !== undefined ? { code: legacyCode } : {}),
    };
  } catch {
    return DEFAULT_FEATURES;
  }
}

export function parseProviderFlags(raw: string | undefined): Record<string, boolean> {
  if (!raw) return {};
  try {
    return JSON.parse(raw) as Record<string, boolean>;
  } catch {
    return {};
  }
}

export interface NewDraftSelection {
  provider: string;
  model?: string;
  /** The custom agent the new-draft picker last selected, if any. Resolved
   *  against the live `custom_agents` list on use — a stale id is ignored. */
  customAgentId?: string;
}

export const DEFAULT_NEW_DRAFT_SELECTION: NewDraftSelection = {
  provider: DEFAULT_PROVIDER_ID,
};

export function parseNewDraftSelection(raw: string | undefined): NewDraftSelection {
  if (!raw) return DEFAULT_NEW_DRAFT_SELECTION;
  try {
    const saved = JSON.parse(raw) as Partial<NewDraftSelection>;
    const provider =
      typeof saved.provider === "string" && saved.provider.trim()
        ? saved.provider
        : DEFAULT_PROVIDER_ID;
    const model = typeof saved.model === "string" && saved.model.trim() ? saved.model : undefined;
    const customAgentId =
      typeof saved.customAgentId === "string" && saved.customAgentId.trim()
        ? saved.customAgentId
        : undefined;
    return {
      provider,
      ...(model ? { model } : {}),
      ...(customAgentId ? { customAgentId } : {}),
    };
  } catch {
    return DEFAULT_NEW_DRAFT_SELECTION;
  }
}

// ---- Pane widths --------------------------------------------------------------

/** Default pane widths (px); also the fallback when a stored value is missing
 *  or corrupt. Mirrored in the initial store state. */
export const DEFAULT_LEFT_WIDTH = 312;
export const DEFAULT_RIGHT_WIDTH = 520;
/** Lower bound matches the splitter's MIN_WIDTH; the right pane's true upper
 *  bound is dynamic (capped at render via CSS `min()`), so we only guard
 *  against absurd/NaN persisted values here. */
const MIN_PANE_WIDTH = 220;
const MAX_PANE_WIDTH = 4000;

/** Restore a persisted pane width, clamping to a sane range and falling back
 *  to the default on a missing or non-numeric value. */
export function parsePaneWidth(raw: string | undefined, fallback: number): number {
  const n = Number(raw);
  if (!Number.isFinite(n)) return fallback;
  return Math.min(MAX_PANE_WIDTH, Math.max(MIN_PANE_WIDTH, n));
}

// ---- Provider binary path overrides ------------------------------------------

/** Settings-key prefix for per-agent custom binary paths. Must match the
 *  backend's `database::AGENT_BIN_PREFIX` so both read/write the same rows. */
const AGENT_BIN_PREFIX = "agent_bin_path_";

/** Pull the `agent_bin_path_<id>` rows out of the flat settings map into an
 *  id → path override map (blank values dropped, matching the backend). */
export function parseProviderPathOverrides(s: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(s)) {
    if (key.startsWith(AGENT_BIN_PREFIX) && value.trim()) {
      out[key.slice(AGENT_BIN_PREFIX.length)] = value;
    }
  }
  return out;
}
