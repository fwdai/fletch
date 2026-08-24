import { useAppStore } from "@/store";
import { NEUTRAL_BUILD_RUNTIME } from "@/store/sandbox";
import { Icon } from "./Icon";
import { Button } from "./ui/Button";

/**
 * Bottom-left toasts for the embedded agent image builds, one per container
 * runtime. The first spawn under a runtime triggers a (potentially
 * minutes-long) image build; this surfaces its progress so the wait is legible,
 * then clears itself when the build finishes. A failed build stays up with the
 * reason and a dismiss. Renders nothing when no build is in flight. Fed by the
 * `docker:build-progress` event (see store/eventListeners); Docker and Podman
 * builds are independent, so both can be in flight and rendered at once.
 */
export function DockerBuildToast() {
  const builds = useAppStore((s) => s.containerBuilds);
  const dismiss = useAppStore((s) => s.dismissDockerBuild);

  const entries = Object.entries(builds);
  if (entries.length === 0) return null;

  return (
    <div className="docker-build-stack">
      {entries.map(([runtime, build]) => {
        const failed = build.status === "failed";
        const what =
          runtime === NEUTRAL_BUILD_RUNTIME
            ? "container sandbox image"
            : `${runtime} sandbox image`;
        const heading =
          runtime === NEUTRAL_BUILD_RUNTIME
            ? "Sandbox image build failed"
            : `${runtime} sandbox image build failed`;
        return (
          <div
            key={runtime}
            className="update-toast docker-build-toast"
            role={failed ? "alert" : "status"}
          >
            <Icon name={failed ? "close" : "cube"} />
            <div className="update-toast-body">
              <div className="update-toast-text">
                <strong>{failed ? heading : `Building ${what}…`}</strong>
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
                  <Button variant="ghost" onClick={() => dismiss(runtime)}>
                    Dismiss
                  </Button>
                </div>
              )}
            </div>
          </div>
        );
      })}
    </div>
  );
}
