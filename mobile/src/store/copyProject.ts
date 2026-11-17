// A project given to a host that has no copy of it, so that an agent of it can
// be started there: the steps of the desktop's `moveDraft` (src/store/
// moveDraft.ts) before the draft itself moves, in its order. A host that has the
// project describes it (`project_definition`), the target makes sure it can run
// it (`ensure_project`, which clones the repositories the first time), and the
// target's workspace is read again so every list sees the copy. Whatever asked
// moves only once all of it is done, so a failure leaves it where it was.

import { travelReason } from "@desktop/components/Composer/RunTargetPicker/copyWords";
import { linkOf } from "../host";
import { type ProjectEntry, projectById } from "../lib/projects";
import { definitionSource } from "../lib/runsOn";
import { runsOnHostsNow } from "./everyHost";
import { hostStoreOf } from "./host";
import { appStore } from "./index";

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** How many times the target's workspace is read until it lists the copy. A
 *  read is dropped when a newer one starts before it lands (store/
 *  workspaceReads), such as the one `ensure_project`'s `workspace:changed`
 *  starts, so the first answer is not taken as the last word. */
const READS = 3;

/** Give `target`, a paired host without `entry`'s project, a copy of it, asked
 *  of the host that speaks for the project first. Resolves once `target`'s
 *  store lists the copy; rejects with nothing changed on the phone, the reason
 *  in `lastError`, which the sheet that asked shows. */
export async function copyProjectTo(target: string, entry: ProjectEntry): Promise<void> {
  appStore.setState({ lastError: null });
  try {
    const source = definitionSource(entry, runsOnHostsNow());
    if (!source) throw new Error(`No connected host can share ${entry.project.name}.`);
    const definition = await linkOf(source.hostKey).api.projectDefinition(entry.project.id);
    const stuck = travelReason(definition);
    if (stuck) throw new Error(stuck);
    // Asked again now that the definition is in hand: the target may have
    // gone since the pick.
    const name = appStore.getState().hosts.find((h) => h.hostKey === target)?.name;
    if (!name || !runsOnHostsNow().some((h) => h.hostKey === target)) {
      throw new Error(`${name ?? "The host"} · offline`);
    }
    await linkOf(target).api.ensureProject(definition);
    const there = hostStoreOf(target);
    const lists = () => !!projectById(there.getState().workspace, definition.project_id);
    for (let read = 0; read < READS && !lists(); read++) {
      await there.getState().refreshWorkspace();
    }
    if (!lists()) throw new Error(`${name} has no copy of ${definition.name}.`);
  } catch (e) {
    appStore.setState({ lastError: message(e) });
    throw e;
  }
}
