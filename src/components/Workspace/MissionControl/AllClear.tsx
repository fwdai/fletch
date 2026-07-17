import { Icon } from "@/components/Icon";

/** The calm empty state — a quiet checkmark and one line, in the app's
 *  empty-state vocabulary. Not a blank pane, not celebratory. Tailors its copy
 *  to whether the workspace has any agents yet (first-run onboarding hint vs.
 *  "nothing needs you"). */
export function AllClear({ hasAgents }: { hasAgents: boolean }) {
  return (
    <div className="mc-allclear">
      <span className="mc-allclear-mark iflex-center">
        <Icon name={hasAgents ? "check" : "sparkle"} size={22} />
      </span>
      <div className="mc-allclear-title">{hasAgents ? "All clear" : "No agents yet"}</div>
      <div className="mc-allclear-sub">
        {hasAgents
          ? "Nothing needs your review right now."
          : "Spawn an agent from a project in the sidebar, and it'll show up here when it needs you."}
      </div>
    </div>
  );
}
