import { Icon } from "../Icon";
import { IconButton } from "../ui/IconButton";
import { useAppStore } from "../../store";

export function SidebarFooter() {
  const theme = useAppStore((s) => s.theme);
  const setTheme = useAppStore((s) => s.setTheme);
  const isDark = theme === "dark";
  return (
    <div className="side-foot">
      <div className="user-avatar">A</div>
      <div className="user-info">
        <div className="un">you</div>
        <div className="ue">local workspace</div>
      </div>
      <IconButton
        tip={isDark ? "Light theme (⌘⇧L)" : "Dark theme (⌘⇧L)"}
        onClick={() => setTheme(isDark ? "light" : "dark")}
        aria-label="Toggle theme"
      >
        <Icon name={isDark ? "sun" : "moon"} />
      </IconButton>
    </div>
  );
}
