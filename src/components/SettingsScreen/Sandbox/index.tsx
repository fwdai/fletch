import { useEffect } from "react";
import { Select } from "@/components/ui/Select";
import type { SandboxEngine } from "@/storage/preferences";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { SetGroup, SetHead, SetRow, SetToggle } from "../primitives";
import { BoxKnobs } from "./BoxKnobs";
import { BoxSetupStatus } from "./BoxSetupStatus";
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

/** Settings › Sandbox: which isolation engine new agents get, the isolated
 *  VM behind the automatic choice, how container agents authenticate, and
 *  the per-runtime launch knobs. */
export function SandboxPane() {
  const sandboxEngine = useAppStore((s) => s.sandboxEngine);
  const setSandboxEngine = useAppStore((s) => s.setSandboxEngine);
  const dockerProbe = useAppStore((s) => s.dockerProbe);
  const refreshDockerProbe = useAppStore((s) => s.refreshDockerProbe);
  const podmanProbe = useAppStore((s) => s.podmanProbe);
  const refreshPodmanProbe = useAppStore((s) => s.refreshPodmanProbe);
  const boxesEnabled = useAppStore((s) => s.boxesEnabled);
  const setBoxesEnabled = useAppStore((s) => s.setBoxesEnabled);
  const boxProbe = useAppStore((s) => s.boxProbe);
  const boxImage = useAppStore((s) => s.boxImage);
  const boxTools = useAppStore((s) => s.boxTools);
  const refreshBoxProbe = useAppStore((s) => s.refreshBoxProbe);
  // The engine (and the launch knobs below) are the host's: it stamps them on
  // the agents it spawns.
  const hostGate = useGate("hostSettings");

  useEffect(() => {
    let cancelled = false;
    const probe = () => {
      if (cancelled) return;
      void refreshDockerProbe();
      void refreshPodmanProbe();
      void refreshBoxProbe();
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
  }, [refreshDockerProbe, refreshPodmanProbe, refreshBoxProbe]);

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
  // The machine can boot a box: the switch is usable, and with it on the box
  // controls below are shown. An Intel Mac, or one without the runtime, gets
  // a disabled switch that says why rather than a refusal on click.
  const boxRuntimeProblem =
    boxProbe && !boxProbe.runtime_ok ? boxProbe.problems.join("; ") : undefined;
  const boxUsable = boxesEnabled && !!boxProbe?.runtime_ok;
  const setup = boxSetup(boxProbe, boxImage, boxTools);
  // What Automatic lands on right now, in two words: the Select's hint.
  const autoHint = !boxUsable
    ? "macOS sandbox"
    : setup.state === "ready"
      ? "Isolated VM"
      : "macOS sandbox until the VM is ready";

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
            "Automatic runs each new workspace in an isolated VM once one is ready, and in the macOS sandbox until then. Applies to new workspaces only."
          }
        >
          <Select<SandboxEngine>
            value={sandboxEngine}
            ariaLabel="Sandbox engine"
            disabled={!!hostGate}
            options={[
              { value: "auto", label: "Automatic", hint: autoHint },
              { value: "sandbox-exec", label: "macOS sandbox (Seatbelt)" },
              ...(boxUsable
                ? [
                    {
                      value: "box" as const,
                      label: "Isolated VM (box)",
                      hint: setup.text,
                    },
                  ]
                : []),
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
            back to Automatic.
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
            or switch back to Automatic.
          </div>
        )}
        <ContainerAuth />
      </SetGroup>

      <SetGroup label="Isolated VM">
        <SetRow
          title="Use isolated VMs when available"
          sub={
            hostGate ??
            "A micro-VM per workspace that shares no folder with your Mac and reaches only the project's allowed hosts. Its image downloads once, in the background; until it's in, workspaces use the macOS sandbox. Projects that build a macOS or iOS app stay on your Mac."
          }
        >
          <SetToggle
            on={boxUsable}
            onClick={() => void setBoxesEnabled(!boxesEnabled)}
            disabled={!!hostGate || !!boxRuntimeProblem || !boxProbe}
            tip={hostGate ?? boxRuntimeProblem ?? (!boxProbe ? "Checking…" : undefined)}
          />
        </SetRow>
        {boxUsable && <BoxSetupStatus />}
      </SetGroup>

      {boxUsable && <BoxKnobs />}
      <ContainerLaunchKnobs runtime="docker" />
      <ContainerLaunchKnobs runtime="podman" last={!boxUsable} />
      {boxUsable && <EgressGlobal last />}
    </div>
  );
}
