// ArtifactPanel — the gate's reviewed artifact (spec §9), rendered as markdown.
// This is the document the reviewer reads first (the plan is the decision, the
// diff is the consequence), so ReviewSurface mounts it above the Diff section.
// Reuses the shared Markdown renderer and the chat's `.m-agent` prose skin
// (the ProductBrief precedent) rather than growing a second markdown stack.

import type { GateArtifact } from "../../api";
import { Markdown } from "../Markdown";

export function ArtifactPanel({ artifact }: { artifact: GateArtifact }) {
  return (
    <div className="rv-artifact">
      <div className="rv-artifact-body m-agent">
        <Markdown>{artifact.content}</Markdown>
      </div>
      {artifact.truncated && (
        <div className="rv-checks-note">
          Truncated for review — read the full <code>{artifact.path}</code> in the checkout.
        </div>
      )}
    </div>
  );
}
