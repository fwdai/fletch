import { useEffect } from "react";
import { Select } from "@/components/ui/Select";
import type { SandboxEngine } from "@/storage/preferences";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { SetGroup, SetHead, SetRow } from "../primitives";
import { BoxKnobs } from "./BoxKnobs";
import { boxSetup } from "./boxSetup";
import { ContainerAuth } from "./ContainerAuth";
import { ContainerLaunchKnobs } from "./ContainerLaunchKnobs";
import { EgressGlobal } from "./EgressGlobal";

// A container runtime can start or stop while this pane stays open, so we
// re-probe on a steady interval (plus immediately on mount and on window focus)
// rather than once. Polling both ways keeps each engine option AND the
// "selected but unavailable" warning tracking the live runtime: a one-shot or
// stop-when-available probe would latch a stale state and, e.g., leave the
// warning hidden after the daemon stops. The backend caches each probe for a
// few seconds, so a tight interval mostly hits that cache.
const PROBE_INTERVAL_MS = 3_000;

/** Settings › Sandbox: which isolation engine new agents get, how container
 *  agents authenticate, and the per-runtime launch knobs. */
export function SandboxPane() {
  const sandboxEngine = useAppStore((s) => s.sandboxEngine);
  const setSandboxEngine = useAppStore((s) => s.setSandboxEngine);
  const dockerProbe = useAppStore((s) => s.dockerProbe);
  const refreshDockerProbe = useAppStore((s) => s.refreshDockerProbe);
  const podmanProbe = useAppStore((s) => s.podmanProbe);
  const refreshPodmanProbe = useAppStore((s) => s.refreshPodmanProbe);
  // The box sandbox is piloted behind Settings › Developer: with the flag off
  // nothing below asks about it, so this page is what it was before boxes.
  const boxesEnabled = useAppStore((s) => s.boxesEnabled);
  const boxProbe = useAppStore((s) => s.boxProbe);
  const boxImage = useAppStore((s) => s.boxImage);
  const boxTools = useAppStore((s) => s.boxTools);
  const refreshBoxProbe = useAppStore((s) => s.refreshBoxProbe);
  const refreshBoxImage = useAppStore((s) => s.refreshBoxImage);
  // The engine (and the launch knobs below) are the host's: it stamps them on
  // the agents it spawns.
  const hostGate = useGate("hostSettings");

  useEffect(() => {
    let cancelled = false;
    const probe = () => {
      if (cancelled) return;
      void refreshDockerProbe();
      void refreshPodmanProbe();
      if (boxesEnabled) void refreshBoxProbe();
    };
    probe(); // immediately on mount
    const timer = setInterval(probe, PROBE_INTERVAL_MS);
    // Re-check right away when the window returns (e.g. the user just launched
    // Docker Desktop from the hint) instead of waiting for the next tick.
    const onFocus = () => probe();
    window.addEventListener("focus", onFocus);
    return () => {
      cancelled = true;
      clearInterval(timer);
      window.removeEventListener("focus", onFocus);
    };
  }, [refreshDockerProbe, refreshPodmanProbe, refreshBoxProbe, boxesEnabled]);

  // The image state is read once; `box:image` carries every change after.
  useEffect(() => {
    if (boxesEnabled) void refreshBoxImage();
  }, [refreshBoxImage, boxesEnabled]);

  // Each container option is enabled only when the runtime answered the probe;
  // otherwise it's disabled with a hint saying how to fix it. A `null` probe is
  // still in flight, so it gates the option off but says "Checking…" rather
  // than calling an installed runtime missing.
  const dockerAvailable = dockerProbe?.status === "available";
  const dockerHint = dockerAvailable
    ? dockerProbe?.version && `v${dockerProbe.version}`
    : !dockerProbe
      ? "Checking…"
      : dockerProbe.status === "daemon-down"
        ? "Start Docker Desktop"
        : "Install Docker Desktop";
  const podmanAvailable = podmanProbe?.status === "available";
  const podmanHint = podmanAvailable
    ? podmanProbe?.version && `v${podmanProbe.version}`
    : !podmanProbe
      ? "Checking…"
      : podmanProbe.status === "machine-down"
        ? "Run podman machine start"
        : "Install Podman";
  // Offered as soon as the flag is on (the toggle already refused a machine
  // that cannot boot a box), and selectable right away: a launch waits for
  // the image and the tools disk while they are still being set up, so the
  // hint only says where that stands. Settings › Developer has the controls.
  const boxOffered = boxesEnabled;
  const boxHint = boxSetup(boxProbe, boxImage, boxTools).text;

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Sandbox"
        title="Sandbox"
        desc="How agents are isolated from your machine, and how container agents launch and sign in."
      />

      <SetGroup label="Isolation">
        <SetRow
          title="Engine"
          sub={
            hostGate ??
            "Docker and Podman run agents in a Linux container. Applies to new agents only."
          }
        >
          <Select<SandboxEngine>
            value={sandboxEngine}
            ariaLabel="Sandbox engine"
            disabled={!!hostGate}
            options={[
              { value: "sandbox-exec", label: "Seatbelt (sandbox-exec)" },
              {
                value: "docker",
                label: "Docker",
                hint: dockerHint || undefined,
                disabled: !dockerAvailable,
              },
              {
                value: "podman",
                label: "Podman",
                hint: podmanHint || undefined,
                disabled: !podmanAvailable,
              },
              ...(boxOffered
                ? [
                    {
                      value: "box" as const,
                      label: "Box (micro-VM)",
                      hint: boxHint,
                    },
                  ]
                : []),
            ]}
            onChange={(v) => void setSandboxEngine(v)}
          />
        </SetRow>
        {sandboxEngine === "docker" && dockerProbe && !dockerAvailable && (
          <div className="set-sandbox-warn">
            Docker is selected but{" "}
            {dockerProbe.status === "daemon-down" ? "the daemon isn't running" : "isn't installed"}.
            New agents won't launch until it's available.{" "}
            {dockerProbe.status === "daemon-down" ? "Start" : "Install"} Docker Desktop, or switch
            back to Seatbelt.
          </div>
        )}
        {sandboxEngine === "podman" && podmanProbe && !podmanAvailable && (
          <div className="set-sandbox-warn">
            Podman is selected but{" "}
            {podmanProbe.status === "machine-down"
              ? "its machine isn't running"
              : "isn't installed"}
            . New agents won't launch until it's available.{" "}
            {podmanProbe.status === "machine-down" ? "Run podman machine start" : "Install Podman"},
            or switch back to Seatbelt.
          </div>
        )}
        <ContainerAuth />
      </SetGroup>

      <ContainerLaunchKnobs runtime="docker" />
      <ContainerLaunchKnobs runtime="podman" last={!boxesEnabled} />
      {boxesEnabled && <BoxKnobs />}
      {boxesEnabled && <EgressGlobal last />}
    </div>
  );
}
