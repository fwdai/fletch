import { DEFAULT_FEATURES, type ThemeMode, type WorkspaceView } from "@/storage/preferences";
import { setSetting } from "@/storage/settings";
import type { AppearanceSlice, SliceCreator } from "./types";

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
