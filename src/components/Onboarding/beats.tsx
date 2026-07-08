// Onboarding feature beats + the create-step's sample repo data.
// Ported from the design prototype (onboarding/steps.jsx).

import type { ReactNode } from "react";
import type { IconName } from "@/components/Icon";
import { ExhibitCode, ExhibitParallel, ExhibitProviders, ExhibitRoom } from "./exhibits";

export interface BeatPoint {
  icon: IconName;
  head: string;
  body: string;
}

export interface BeatDef {
  key: string;
  num: string;
  eyebrow: string;
  title: ReactNode;
  lede: ReactNode;
  points: BeatPoint[];
  Exhibit: () => JSX.Element;
}

const BEAT_PARALLEL: BeatDef = {
  key: "parallel",
  num: "01",
  eyebrow: "Parallel by design",
  title: (
    <>
      Every task gets its <em>own checkout.</em>
    </>
  ),
  lede: (
    <>
      Spin up as many agents as the work demands. Each runs on an isolated branch, so nothing
      collides — and <b>everything moves at once.</b>
    </>
  ),
  points: [
    { icon: "branch", head: "Isolated branches.", body: "No stepping on each other's changes." },
    { icon: "layers", head: "Run in parallel.", body: "Five tasks, five agents, one glance." },
    { icon: "map", head: "Named by landmark.", body: "Each checkout is easy to find again." },
  ],
  Exhibit: () => <ExhibitParallel />,
};

const BEAT_PROVIDERS: BeatDef = {
  key: "providers",
  num: "02",
  eyebrow: "Bring your own agent",
  title: (
    <>
      Claude, Codex, Cursor — <em>under one roof.</em>
    </>
  ),
  lede: (
    <>
      Point Fletch at the agents you already pay for. Switch per task, compare side by side —{" "}
      <b>no lock-in, ever.</b>
    </>
  ),
  points: [
    {
      icon: "cube",
      head: "Six providers, day one.",
      body: "Claude Code, Codex, Cursor, Gemini & more.",
    },
    { icon: "refresh", head: "Swap per task.", body: "Pick the right model for the job." },
    {
      icon: "settings",
      head: "Your keys, your limits.",
      body: "Connects to your existing subscriptions.",
    },
  ],
  Exhibit: () => <ExhibitProviders />,
};

// Control room — every checkout at a glance (shown as beat 03).
const BEAT_ROOM: BeatDef = {
  key: "room",
  num: "03",
  eyebrow: "One quiet control room",
  title: (
    <>
      Every checkout, <em>at a glance.</em>
    </>
  ),
  lede: (
    <>
      Home shows you what's running, what's waiting, and what needs you — across every project.{" "}
      <b>The whole fleet, one room.</b>
    </>
  ),
  points: [
    { icon: "panelGrid", head: "All projects, one view.", body: "No tab-hopping between repos." },
    { icon: "commit", head: "Status at a glance.", body: "Running, waiting, or needs your call." },
    { icon: "arrowR", head: "Jump straight in.", body: "One click into any agent's checkout." },
  ],
  Exhibit: () => <ExhibitRoom />,
};

// Live code — kept for later; not currently in the sequence.
export const BEAT_CODE: BeatDef = {
  key: "code",
  num: "03",
  eyebrow: "Nothing hidden",
  title: (
    <>
      Watch every edit <em>as it lands.</em>
    </>
  ),
  lede: (
    <>
      A live diff streams each change in real time. Follow the agent's reasoning, catch issues
      early, <b>stay in control.</b>
    </>
  ),
  points: [
    { icon: "diff", head: "Streaming diffs.", body: "Lines appear as the agent writes them." },
    { icon: "thinking", head: "Reasoning in context.", body: "See why each change was made." },
    { icon: "pr", head: "Straight to a PR.", body: "Review, commit, and ship without leaving." },
  ],
  Exhibit: () => <ExhibitCode />,
};

export const BEATS: BeatDef[] = [BEAT_PARALLEL, BEAT_PROVIDERS, BEAT_ROOM];
