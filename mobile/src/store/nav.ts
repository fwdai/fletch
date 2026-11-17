// The app's navigation: the stack of screens and the one sheet over it, shared
// by every host. What a host does to them, and which host each acts on, is
// store/hostNav's.

import type { AppState } from "./index";

type Set = (partial: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => void;
type Get = () => AppState;

export type ScreenName =
  | "home"
  | "project"
  | "agent"
  | "subagent"
  | "code"
  | "file"
  | "diff"
  | "providers";
export type SheetName =
  | "host"
  | "newAgent"
  | "newPlan"
  | "addProject"
  | "agentMore"
  | "shipMore"
  | "modelPicker"
  | "accountPicker"
  | "pr";

export interface NavItem {
  key: number;
  screen: ScreenName;
  /** What the screen shows. `hostKey` names the host it acts on (`withHost`
   *  in store/hostNav); Home, which names none, acts on the host on screen. */
  props: Record<string, string>;
  phase: "enter" | "idle" | "leave";
}

/** The agent screen's tabs, as its `tab` nav prop names them. */
export type AgentTab = "chat" | "changes" | "ship";

export interface SheetState {
  name: SheetName;
  /** As a screen's: `hostKey` names the host the sheet acts on. */
  props: Record<string, string>;
  open: boolean;
}

export interface NavSlice {
  nav: NavItem[];
  sheet: SheetState | null;
  push(screen: ScreenName, props?: Record<string, string>): void;
  pop(): void;
  openSheet(name: SheetName, props?: Record<string, string>): void;
  /** Close whatever sheet is open. Safe to hand straight to an `onClose` or
   *  `onClick`: it ignores its arguments. */
  closeSheet(): void;
}

/** Home, as the one screen of a fresh stack. */
export const homeItem = (): NavItem => ({
  key: Date.now(),
  screen: "home",
  props: {},
  phase: "idle",
});

export function createNavSlice(set: Set, get: Get): NavSlice {
  return {
    nav: [{ key: 1, screen: "home", props: {}, phase: "idle" }],
    sheet: null,

    push(screen, props = {}) {
      const key = Date.now() + Math.random();
      set((s) => ({ nav: [...s.nav, { key, screen, props, phase: "enter" }] }));
      // Two frames so the enter transform is painted before it is released.
      requestAnimationFrame(() =>
        requestAnimationFrame(() =>
          set((s) => ({
            nav: s.nav.map((i) =>
              i.key === key && i.phase === "enter" ? { ...i, phase: "idle" } : i,
            ),
          })),
        ),
      );
    },

    pop() {
      const live = get().nav.filter((i) => i.phase !== "leave");
      if (live.length <= 1) return;
      const top = live[live.length - 1];
      set((s) => ({ nav: s.nav.map((i) => (i.key === top.key ? { ...i, phase: "leave" } : i)) }));
      setTimeout(() => set((s) => ({ nav: s.nav.filter((i) => i.phase !== "leave") })), 470);
    },

    openSheet(name, props = {}) {
      set({ sheet: { name, props, open: true } });
    },
    closeSheet() {
      set((s) => (s.sheet ? { sheet: { ...s.sheet, open: false } } : s));
    },
  };
}
