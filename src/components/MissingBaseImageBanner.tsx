import { useAppStore } from "../store";

/**
 * Inline call-to-action shown when the workspace's configured base image
 * isn't present in Tart. Without this, the only place the user sees the
 * "Build base image" option is inside ChooseRepoDialog — which they skip
 * when their workspace is already saved from a previous session.
 */
export function MissingBaseImageBanner({
  onBuild,
}: {
  onBuild: () => void;
}) {
  const workspace = useAppStore((s) => s.workspace);
  if (!workspace) return null;

  return (
    <div className="banner banner-warn">
      <div className="banner-body">
        <div className="banner-title">
          Base image <code>{workspace.base_image}</code> not found
        </div>
        <div className="banner-text">
          Agents can't spawn until the base VM exists. Building it downloads
          Ubuntu, installs Node + the Claude Code CLI, and bakes in your
          SSH key. Takes 5–10 minutes once.
        </div>
      </div>
      <div className="banner-actions">
        <button className="primary" onClick={onBuild}>
          Build base image
        </button>
      </div>
    </div>
  );
}
