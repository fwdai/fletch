import { Avatar } from "@/components/Avatar";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
import { accountInitials } from "@/util/format";

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
    <div className="side-foot flex-center">
      <button
        className="side-user"
        onClick={() => openSettingsScreen("account")}
        aria-label="Account settings"
      >
        <Avatar
          className="user-avatar flex-center"
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
