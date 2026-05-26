import { useAppStore } from "../../store";
import { HistoryList } from "./HistoryList";
import { HistoryDetail } from "./HistoryDetail";

/** Main-pane root for the History view. Switches between the list of
 *  archived agents and the read-only chat preview of a selected one. */
export function History() {
  const selectedId = useAppStore((s) => s.selectedHistoryAgentId);
  return selectedId ? <HistoryDetail agentId={selectedId} /> : <HistoryList />;
}
