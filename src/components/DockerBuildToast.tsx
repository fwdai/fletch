import { useAppStore } from "@/store";
import { Icon } from "./Icon";
import { Button } from "./ui/Button";

/**
 * Bottom-left toast for the embedded docker agent image build. The first docker
 * spawn triggers a (potentially minutes-long) `docker build`; this surfaces its
 * progress so the wait is legible, then clears itself when the build finishes.
 * A failed build stays up with the reason and a dismiss. Renders nothing when no
 * build is in flight. Fed by the `docker:build-progress` event (see store/app).
 */
export function DockerBuildToast() {
  const build = useAppStore((s) => s.dockerBuild);
  const dismiss = useAppStore((s) => s.dismissDockerBuild);

  if (!build) return null;

  const failed = build.status === "failed";

  return (
    <div className="update-toast docker-build-toast" role={failed ? "alert" : "status"}>
      <Icon name={failed ? "close" : "cube"} />
      <div className="update-toast-body">
        <div className="update-toast-text">
          <strong>
            {failed ? "Sandbox image build failed" : "Building Docker sandbox image…"}
          </strong>
          <span>
            {failed
              ? (build.error ?? "The build did not complete.")
              : "First container run — this can take a few minutes."}
          </span>
        </div>
        {!failed && build.lastLine && (
          <p className="update-toast-notes docker-build-line mono">{build.lastLine}</p>
        )}
        {failed && (
          <div className="update-toast-actions">
            <Button variant="ghost" onClick={dismiss}>
              Dismiss
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}
