import type { GitPanelState } from "@/components/RightPanel/primaryActions";

/** The centered title + blurb shown for the non-list panel states. Returns
 *  null for states that render their own card/list instead. */
export function EmptyState({ state, base }: { state: GitPanelState; base: string }) {
  const copy = emptyCopy(state, base);
  if (!copy) return null;
  return (
    <div className="empty-msg" style={{ margin: "auto" }}>
      <div className="et">{copy.title}</div>
      <div>{copy.body}</div>
    </div>
  );
}

function emptyCopy(state: GitPanelState, base: string): { title: string; body: string } | null {
  switch (state) {
    case "loading":
      return { title: "Loading…", body: "Fetching git state." };
    case "pushed":
      return {
        title: "Ready for a pull request",
        body: "All changes are committed & pushed. Open a PR to start review.",
      };
    case "merged":
      return {
        title: `Merged into ${base}`,
        body: "This workspace’s work is shipped. Archive it or keep going.",
      };
    case "clean":
      return {
        title: "All clean",
        body: "No uncommitted changes. Type a follow-up to start working.",
      };
    default:
      return null;
  }
}
