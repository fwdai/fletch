import { Icon } from "../Icon";
import { IconButton } from "../ui/IconButton";
import { useAppStore } from "../../store";

export function SidebarFooter() {
  const toggleSettings = useAppStore((s) => s.toggleSettings);
  return (
    <div className="side-foot">
      <div className="user-avatar">A</div>
      <div className="user-info">
        <div className="un">you</div>
        <div className="ue">local workspace</div>
      </div>
      <IconButton tip="Settings (⌘,)" onClick={() => toggleSettings(true)}>
        <Icon name="settings" />
      </IconButton>
    </div>
  );
}
