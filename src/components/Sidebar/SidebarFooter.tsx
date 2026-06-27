import { useAppStore } from "../../store";
import { accountInitials } from "../../util/format";
import { Avatar } from "../Avatar";
import { Icon } from "../Icon";
import { IconButton } from "../ui/IconButton";

export function SidebarFooter() {
  const theme = useAppStore((s) => s.theme);
  const setTheme = useAppStore((s) => s.setTheme);
  const account = useAppStore((s) => s.account);
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);
  const isDark = theme === "dark";

  const fullName = account ? `${account.firstName} ${account.lastName}`.trim() : "";
  const initial = account
    ? accountInitials(account.firstName, account.lastName, account.email)
    : "?";
  const name = fullName || "you";
  const sub = account?.email || "local workspace";

  return (
    <div className="side-foot">
      <button
        className="side-user"
        onClick={() => openSettingsScreen("account")}
        aria-label="Account settings"
      >
        <Avatar
          className="user-avatar"
          avatarUrl={account?.avatarUrl ?? null}
          initials={initial}
          alt={name}
        />
        <div className="user-info">
          <div className="un">{name}</div>
          <div className="ue">{sub}</div>
        </div>
      </button>
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
