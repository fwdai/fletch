import { FletchMark } from "@/components/FletchMark";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
import { OpenInEditor } from "./OpenInEditor";
import { WorkspaceStatus } from "./WorkspaceStatus";

/** Top-of-window bar. Houses the workspace-status capsule (center) and the
 *  history + settings controls (right). Drag + native double-click-to-zoom is
 *  handled by Tauri via the `data-tauri-drag-region` attribute — any click
 *  whose target carries that attribute is processed by the runtime; clicks on
 *  the capsule and buttons fall through normally. */
export function TitleBar() {
  const settingsOpen = useAppStore((s) => s.settingsOpen);
  const toggleSettings = useAppStore((s) => s.toggleSettings);
  const historyOpen = useAppStore((s) => s.historyOpen);
  const toggleHistory = useAppStore((s) => s.toggleHistory);

  return (
    <div className="tb flex-center" data-tauri-drag-region>
      <div className="tb-lights-gutter" data-tauri-drag-region />
      <div className="tb-logo iflex-center" data-tauri-drag-region aria-label="Fletch">
        <FletchMark className="tb-mark" />
        <span className="tb-wordmark" aria-hidden="true">
          fletch
        </span>
        <span className="tb-badge iflex-center">beta</span>
      </div>
      <div className="tb-center">
        <WorkspaceStatus />
      </div>
      <div className="tb-right flex-center">
        <OpenInEditor />
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
