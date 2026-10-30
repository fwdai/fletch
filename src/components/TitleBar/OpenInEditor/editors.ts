import { ask } from "@tauri-apps/plugin-dialog";
import { type AgentRecord, api, type DetectedEditor } from "@/api";
import type { IconName } from "@/components/Icon";
import { EDITOR_LOGOS } from "./logos";

/** What a tool's tile shows: its brand logo, a terminal glyph, or a monogram —
 *  all on the one premium tile surface, so there's no per-editor color here.
 *  This is glyph selection only; labels come from the backend. */
export interface EditorFace {
  mono?: string;
  icon?: IconName;
  /** Single-path brand glyph (viewBox 0 0 24 24), rendered white on the tile. */
  logo?: string;
}

/** Face for a detected tool: brand logo if we have one, else a terminal glyph
 *  for terminals, else a monogram from the label so any tool still renders. */
export function editorFace(editor: DetectedEditor): EditorFace {
  const logo = EDITOR_LOGOS[editor.id];
  if (logo) return { logo };
  if (editor.kind === "terminal") return { icon: "terminal" };
  return { mono: monogram(editor.label) };
}

/** Up-to-two-letter monogram from a label: initials of the first two words, or
 *  the first two letters of a single word ("Nova" → "NO"). */
function monogram(label: string): string {
  const words = label.split(/\s+/).filter(Boolean);
  const letters = words.length > 1 ? words.map((w) => w[0]).join("") : label;
  return letters.slice(0, 2).toUpperCase();
}

// Detection scans PATH + the Applications folders — do it once per session and
// share the result across mounts (the launcher unmounts when no agent is open).
let cache: Promise<DetectedEditor[]> | null = null;

export function detectEditors(): Promise<DetectedEditor[]> {
  cache ??= api.detectEditors().catch(() => []);
  return cache;
}

/** Whether an agent's checkout lives inside a box, where only an editor with
 *  a remote mode can open it. */
export function inBox(agent: AgentRecord): boolean {
  return agent.sandbox_engine === "box";
}

/** The detected tools that can open this agent's checkout. */
export function editorsFor(agent: AgentRecord, editors: DetectedEditor[]): DetectedEditor[] {
  return inBox(agent) ? editors.filter((e) => e.remote) : editors;
}

export const EDITOR_PREF_KEY = "fletch:openInEditor";

/** Open an agent's checkout in `editorId`. A box checkout opens over ssh,
 *  which needs one `Include` line in the editor's ssh config and the
 *  editor's remote extension: the first time, the user is shown what Fletch
 *  will change and asked before it happens. */
export async function openEditor(agent: AgentRecord, editorId: string): Promise<void> {
  const outcome = await api.openInEditor(agent.id, editorId);
  if (outcome === "opened") return;
  const { ssh_config, extension } = outcome.needs_setup;
  const steps = [
    ssh_config && `add one Include line to ${ssh_config}, so the box has a host name`,
    extension && `install the ${extension} extension in the editor`,
  ].filter(Boolean);
  const agreed = await ask(
    "This workspace runs in a box. The editor connects to it over ssh and installs its own helper inside the box.\n\n" +
      `Fletch will ${steps.join(", and ")}, and keep the box running while this workspace is open.`,
    { title: "Open the box in your editor?", okLabel: "Set up and open", cancelLabel: "Cancel" },
  );
  if (agreed) await api.openInEditor(agent.id, editorId, true);
}

/** Open an agent's checkout in the editor the title-bar launcher last used
 *  (its first usable one until a pick is made). Resolves without doing
 *  anything when no editor can open it. */
export async function openInPreferredEditor(agent: AgentRecord): Promise<void> {
  const editors = editorsFor(agent, await detectEditors());
  const pref = localStorage.getItem(EDITOR_PREF_KEY);
  const editor = editors.find((e) => e.id === pref) ?? editors[0];
  if (editor) await openEditor(agent, editor.id);
}
