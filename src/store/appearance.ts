import {
  DEFAULT_FEATURES,
  type FeatureFlags,
  type ThemeMode,
  type WorkspaceView,
} from "@/storage/preferences";
import { setSetting } from "@/storage/settings";
import type { SliceCreator } from "./types";

export interface AppearanceSlice {
  // ── appearance & feature flags ────────────────────────────────────────────
  theme: ThemeMode;
  /** Syntax-highlighting theme for the File panel editor. "quorum" = the
   *  built-in palette; other ids map to a highlight.js theme family that
   *  follows the app's light/dark mode. See data/codeThemes.ts. */
  codeTheme: string;
  accent: string;
  features: FeatureFlags;
  /** Play the chime when an agent turn finishes or needs input while you're
   *  not watching that chat. Opt-out. */
  soundEnabled: boolean;
  /** Send a native OS notification when an agent turn finishes or needs input
   *  while you're not watching that chat. Opt-out. */
  notifyEnabled: boolean;
  /** View mode preference for the workspace pane. Persisted; falls
   *  back to the agent's own `view` field for native vs. custom
   *  switching. */
  viewMode: WorkspaceView;

  // appearance
  setTheme: (t: ThemeMode) => void;
  setCodeTheme: (id: string) => void;
  setAccent: (a: string) => void;
  setFeature: <K extends keyof FeatureFlags>(k: K, v: FeatureFlags[K]) => void;
  setSoundEnabled: (on: boolean) => void;
  setNotifyEnabled: (on: boolean) => void;
  setViewMode: (v: WorkspaceView) => void;
}

export const createAppearanceSlice: SliceCreator<AppearanceSlice> = (set) => ({
  theme: "dark" as ThemeMode,
  codeTheme: "quorum",
  accent: "copper",
  features: DEFAULT_FEATURES,
  soundEnabled: true,
  notifyEnabled: true,
  viewMode: "custom" as WorkspaceView,

  // ── appearance ──────────────────────────────────────────────────────────────
  setTheme: (t) => {
    set({ theme: t });
    setSetting("theme", t);
  },
  setCodeTheme: (id) => {
    set({ codeTheme: id });
    setSetting("codeTheme", id);
  },
  setAccent: (a) => {
    set({ accent: a });
    setSetting("accent", a);
  },
  setFeature: (k, v) =>
    set((s) => {
      const next = { ...s.features, [k]: v };
      setSetting("features", next);
      return { features: next };
    }),
  setSoundEnabled: (on) => {
    set({ soundEnabled: on });
    setSetting("soundEnabled", String(on));
  },
  setNotifyEnabled: (on) => {
    set({ notifyEnabled: on });
    setSetting("notifyEnabled", String(on));
  },
  setViewMode: (v) => {
    set({ viewMode: v });
    setSetting("viewMode", v);
  },
});
