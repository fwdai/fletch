// The host's answer to "where may a new agent run" for the draft on screen:
// one read at a time, keyed by its inputs. The composer's chip draws it and
// `spawnFromDraft` sends what the chip declares from it, so both read the one
// answer, and neither ever reads an answer for other inputs.

import {
  api,
  type BoxImageStatus,
  type BoxStateEvent,
  type BoxToolsStatus,
  type WorkspaceTypeOption,
} from "@/api";
import {
  answeredOptions,
  type WorkspaceTypeAnswer,
  workspaceTypeResponseLands,
  workspaceTypesKey,
} from "@/components/Composer/workspaceType";
import type { AppState, SliceCreator } from "./types";

/** What the picker on screen reads the options for. */
export interface WorkspaceTypeInputs {
  repoPath: string;
  provider: string;
}

export interface WorkspaceTypesSlice {
  workspaceTypes: {
    /** The picker on screen's inputs; null while none is mounted. */
    inputs: WorkspaceTypeInputs | null;
    /** The last successful read, with its key. Dropped the moment a read for
     *  other inputs starts, so it is never shown for inputs it was not for. */
    answer: WorkspaceTypeAnswer | null;
    /** The key whose latest read failed, if any. A failure keeps `answer`. */
    failedKey: string | null;
  };
  /** Read the options for `repoPath` and `provider` (the picker's mount, its
   *  inputs or the settings changing, a chip open, a retry). The latest read
   *  wins; a response for other inputs never lands. */
  loadWorkspaceTypes: (repoPath: string, provider: string) => Promise<void>;
  /** Re-read for the picker on screen, if any: an event moved the default. */
  reloadWorkspaceTypes: () => void;
  /** The picker left the screen: forget its inputs and answer, and drop the
   *  read in flight. */
  releaseWorkspaceTypes: () => void;
}

type KeyState = Pick<AppState, "sandboxEngine" | "boxesEnabled">;

/** The key for `repoPath` and `provider` under the current settings. */
export function workspaceTypesKeyFor(s: KeyState, repoPath: string, provider: string): string {
  return workspaceTypesKey(repoPath, provider, s.sandboxEngine, s.boxesEnabled);
}

/** The host's options for these inputs, or null while there is no answer for
 *  them. What the chip draws and what the spawn sends both read this. */
export function workspaceTypeOptionsFor(
  s: KeyState & Pick<AppState, "workspaceTypes">,
  repoPath: string,
  provider: string,
): WorkspaceTypeOption[] | null {
  return answeredOptions(s.workspaceTypes.answer, workspaceTypesKeyFor(s, repoPath, provider));
}

/** Whether the latest read for these inputs failed. */
export function workspaceTypesFailedFor(
  s: KeyState & Pick<AppState, "workspaceTypes">,
  repoPath: string,
  provider: string,
): boolean {
  return s.workspaceTypes.failedKey === workspaceTypesKeyFor(s, repoPath, provider);
}

/** One box setup event, as the listeners receive it. */
export type BoxSetupEvent =
  | { image: BoxImageStatus }
  | { tools: BoxToolsStatus }
  | { state: BoxStateEvent };

/** Fold a box setup event into the store, and re-read the picker's options
 *  when it moved a phase (the image installed, a tools disk built, a box's
 *  runtime state): those move the default. A download's progress tick does
 *  not, and reads nothing. */
export function applyBoxSetupEvent(get: () => AppState, e: BoxSetupEvent): void {
  const s = get();
  let moved: boolean;
  if ("image" in e) {
    moved = s.boxImage?.phase !== e.image.phase;
    s.applyBoxImage(e.image);
  } else if ("tools" in e) {
    moved = s.boxTools[e.tools.provider]?.phase !== e.tools.phase;
    s.applyBoxTools(e.tools);
  } else {
    moved = s.boxStates[e.state.agent_id]?.phase !== e.state.phase;
    s.applyBoxState(e.state);
  }
  if (moved) get().reloadWorkspaceTypes();
}

/** The latest read started; a response lands only while it is still this. */
let latestRead = 0;

export const createWorkspaceTypesSlice: SliceCreator<WorkspaceTypesSlice> = (set, get) => {
  const currentKey = () => {
    const s = get();
    const inputs = s.workspaceTypes.inputs;
    return inputs ? workspaceTypesKeyFor(s, inputs.repoPath, inputs.provider) : null;
  };

  return {
    workspaceTypes: { inputs: null, answer: null, failedKey: null },

    loadWorkspaceTypes: async (repoPath, provider) => {
      const key = workspaceTypesKeyFor(get(), repoPath, provider);
      latestRead += 1;
      const request = { id: latestRead, key };
      set((s) => {
        const prev = s.workspaceTypes;
        return {
          workspaceTypes: {
            inputs: { repoPath, provider },
            // Other inputs' answer goes now, not when the new one lands: the
            // chip is back to loading rather than showing options that are
            // not these inputs'.
            answer: prev.answer?.key === key ? prev.answer : null,
            failedKey: prev.failedKey === key ? key : null,
          },
        };
      });
      const lands = () => workspaceTypeResponseLands(request, latestRead, currentKey());
      try {
        const options = await api.availableWorkspaceTypes(repoPath, provider);
        if (!lands()) return;
        set((s) => ({
          workspaceTypes: { ...s.workspaceTypes, answer: { key, options }, failedKey: null },
        }));
      } catch {
        if (!lands()) return;
        set((s) => ({ workspaceTypes: { ...s.workspaceTypes, failedKey: key } }));
      }
    },

    reloadWorkspaceTypes: () => {
      const inputs = get().workspaceTypes.inputs;
      if (inputs) void get().loadWorkspaceTypes(inputs.repoPath, inputs.provider);
    },

    releaseWorkspaceTypes: () => {
      latestRead += 1;
      set({ workspaceTypes: { inputs: null, answer: null, failedKey: null } });
    },
  };
};
