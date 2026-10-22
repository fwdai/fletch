import { Avatar } from "@/components/Avatar";
import { useAppStore } from "@/store";
import { displayIdentity } from "@/util/identity";
import { SidebarUsage } from "./SidebarUsage";

/** The user, bottom-left, as a button into Settings › Account. No sign-in is
 *  required to use Fletch, so this is whatever is known: the saved profile,
 *  else the global git identity, else an invitation to fill the profile in. */
export function SidebarFooter() {
  const account = useAppStore((s) => s.account);
  const gitIdentity = useAppStore((s) => s.gitIdentity);
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);

  const who = displayIdentity(account, gitIdentity);
  const name = who.name || (who.email ? who.email.split("@")[0] : "");
  const sub = who.name ? who.email : "";
  const primary = who.source === "none" ? "Set up your profile" : name;
  const secondary = who.source === "none" ? "Name, email and avatar" : sub || "Local workspace";

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
          initials={who.initials}
          alt={name}
        />
        <div className="user-info">
          <div className="un">{primary}</div>
          <div className="ue">{secondary}</div>
        </div>
      </button>
      <SidebarUsage />
    </div>
  );
}
