import { useAppStore } from "../../store";
import { basename, hueFromString } from "../../util/format";
import { Icon } from "../Icon";
import { QMark } from "../QMark";
import { IconButton } from "../ui/IconButton";
import { Breadcrumb, type CrumbEntry } from "./Breadcrumb";

/** Top-of-window bar. Houses the breadcrumb (Quorum / repo / agent)
 *  and the settings toggle. Drag + native double-click-to-zoom is
 *  handled by Tauri via the `data-tauri-drag-region` attribute — any
 *  click whose target carries that attribute is processed by the
 *  runtime; clicks on buttons fall through normally. */
export function TitleBar() {
  const settingsOpen = useAppStore((s) => s.settingsOpen);
  const toggleSettings = useAppStore((s) => s.toggleSettings);
  const historyOpen = useAppStore((s) => s.historyOpen);
  const toggleHistory = useAppStore((s) => s.toggleHistory);
  const entries = useCrumb();

  return (
    <div className="tb flex-center" data-tauri-drag-region>
      <div className="tb-lights-gutter" data-tauri-drag-region />
      <div className="tb-logo iflex-center" data-tauri-drag-region aria-label="Quorum">
        <span className="tb-wordmark" aria-hidden="true">
          <QMark className="tb-qmark" />
          uorum
        </span>
        <span className="tb-badge iflex-center">beta</span>
      </div>
      <Breadcrumb entries={entries} />
      <div className="tb-right flex-center">
        <IconButton tip="History" active={historyOpen} onClick={() => toggleHistory()}>
          <Icon name="history" />
        </IconButton>
        <IconButton tip="Settings (⌘,)" active={settingsOpen} onClick={() => toggleSettings()}>
          <Icon name="settings" />
        </IconButton>
      </div>
    </div>
  );
}

/** Derive the breadcrumb from active draft / agent. Quorum > repo > agent. */
function useCrumb(): CrumbEntry[] {
  const workspace = useAppStore((s) => s.workspace);
  const selectedId = useAppStore((s) => s.selectedAgentId);
  const drafts = useAppStore((s) => s.drafts);
  const activeDraftId = useAppStore((s) => s.activeDraftId);

  const draft = activeDraftId ? drafts.find((d) => d.id === activeDraftId) : null;
  const agent = !draft && selectedId ? workspace?.agents.find((a) => a.id === selectedId) : null;

  const repoPath = draft?.repoPath ?? agent?.repos[0]?.repo_path ?? null;
  const repoLabel = repoPath ? basename(repoPath) : null;
  const repoHue = repoPath ? hueFromString(repoPath) : undefined;

  const entries: CrumbEntry[] = [];
  if (repoLabel) entries.push({ label: repoLabel, mono: true, swatchHue: repoHue });
  if (draft) entries.push({ label: draft.name, mono: true, active: true });
  else if (agent) entries.push({ label: agent.name, mono: true, active: true });
  return entries;
}
