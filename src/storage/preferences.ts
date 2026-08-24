import { DEFAULT_PROVIDER_ID } from "@/data/providers";

// Typed app-preference parsers: turn the flat string→string settings blob read
// by ./settings.ts into the structured values the store holds. Kept out of the
// store so the migration/clamping logic lives next to the persistence layer it
// belongs to and can be unit-tested in isolation.

// ---- Appearance & feature-flag types -----------------------------------------

export type ThemeMode = "dark" | "light";
export type SettingsSection =
  | "general"
  | "account"
  | "providers"
  | "agents"
  | "skills"
  | "tools"
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
  /** Show the context-window usage meter in the composer foot. */
  tokenUsage: boolean;
  /** Experimental: expose the Custom/Native view switcher so agents can be
   *  driven through the provider's own terminal UI. Off by default — native
   *  mode isn't equally solid across providers yet. */
  nativeView: boolean;
  /** Use Mission Control (the fleet review queue) as the Home view. Off by
   *  default — Home is then the quick-actions landing screen. Gated behind the
   *  Developer tab. */
  missionControl: boolean;
}

export const DEFAULT_FEATURES: FeatureFlags = {
  git: true,
  code: true,
  run: false,
  terminal: false,
  thinkingBudget: true,
  tokenUsage: true,
  nativeView: false,
  missionControl: false,
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
      // removed feature; drop any stored value on read
      autoEdit?: boolean;
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
    const {
      files: _files,
      diff: _diff,
      statusBar: _statusBar,
      autoEdit: _autoEdit,
      ...rest
    } = saved;
    void _files;
    void _diff;
    void _statusBar;
    void _autoEdit;
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

// ---- Mission Control dismissals ----------------------------------------------

/** Mission Control's "dismissed" marks: a review-queue item id → the signature
 *  of the signal state it was dismissed at (see MissionControl/queue.ts). A mark
 *  is honored only while the item's live signature still matches, so a dismissed
 *  item resurfaces the moment its underlying signal changes (a new turn, a new
 *  diff, a CI flip). Stored as one JSON object under `reviewDismissed`; a corrupt
 *  or missing blob reads as "nothing dismissed". */
export function parseReviewDismissed(raw: string | undefined): Record<string, string> {
  if (!raw) return {};
  try {
    const saved = JSON.parse(raw) as unknown;
    if (!saved || typeof saved !== "object") return {};
    const out: Record<string, string> = {};
    for (const [k, v] of Object.entries(saved as Record<string, unknown>)) {
      if (typeof v === "string") out[k] = v;
    }
    return out;
  } catch {
    return {};
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

/** Floors for the Roadmap tab's two columns. The board stops being a list
 *  below ~360px (the row's chips start wrapping); the chat's transcript is
 *  laid out for a reading column and stops being one below ~400px. */
export const ROADMAP_MIN_BOARD = 360;
export const ROADMAP_MIN_THREAD = 400;

/** Board width on the Roadmap tab, or `null` for the default even split.
 *
 *  `null` is meaningfully different from a number here: until the user drags
 *  the splitter the columns stay *proportional* (a CSS 50%), so the page looks
 *  right on any window. The first drag converts that into a fixed width, which
 *  is how the app's other panes behave. */
export function parseRoadmapBoardWidth(raw: string | undefined): number | null {
  if (!raw) return null;
  const n = Number(raw);
  if (!Number.isFinite(n)) return null;
  return Math.min(MAX_PANE_WIDTH, Math.max(ROADMAP_MIN_BOARD, n));
}

// ---- Sandbox engine ------------------------------------------------------------

/** Isolation engine new agents are stamped with. Mirrors the backend's
 *  `EngineKind::as_setting` spellings (`sandbox/engine.rs`), so both sides
 *  agree on the wire strings. */
export type SandboxEngine = "sandbox-exec" | "docker" | "podman";

export const DEFAULT_SANDBOX_ENGINE: SandboxEngine = "sandbox-exec";

/** Parse the `sandbox_engine` setting. The key is backend-owned (snake_case,
 *  written by the `set_sandbox_engine` Rust command — never via a frontend
 *  `setSetting`, same posture as `telemetry_enabled`). Unknown/missing values
 *  fall back to the seatbelt default, matching the backend's parser. */
export function parseSandboxEngine(raw: string | undefined): SandboxEngine {
  return raw === "docker" || raw === "podman" ? raw : DEFAULT_SANDBOX_ENGINE;
}

/** Whether an engine runs the agent inside a container — mirrors the backend's
 *  `EngineKind::is_container`. Every surface that gates on "containerized"
 *  (provider support, the workspace badge) asks this instead of comparing
 *  against `"docker"`, so a third container runtime lands in one place. Takes a
 *  loose string because the badge reads an agent's stamped `sandbox_engine`
 *  straight off the record, where it is unparsed and may be absent. */
export function isContainerEngine(engine: string | null | undefined): boolean {
  return engine === "docker" || engine === "podman";
}

/** The engine's name in prose ("Docker isn't available…"). */
export function sandboxEngineLabel(engine: SandboxEngine): string {
  switch (engine) {
    case "docker":
      return "Docker";
    case "podman":
      return "Podman";
    default:
      return "Seatbelt";
  }
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
