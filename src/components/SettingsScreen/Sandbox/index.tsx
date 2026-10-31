import { useEffect } from "react";
import { Select } from "@/components/ui/Select";
import type { SandboxEngine } from "@/storage/preferences";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { useBoxesUnlocked } from "@/util/boxesUnlocked";
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
  // Boxes are rolled out behind the developer unlock: until then this page is
  // what it was before them — no VM group, no Automatic option, no probe —
  // and the stored `auto` reads as the macOS sandbox it resolves to.
  const unlocked = useBoxesUnlocked();

  useEffect(() => {
    let cancelled = false;
    const probe = () => {
      if (cancelled) return;
      void refreshDockerProbe();
      void refreshPodmanProbe();
      if (unlocked) void refreshBoxProbe();
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
  }, [refreshDockerProbe, refreshPodmanProbe, refreshBoxProbe, unlocked]);

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
  // Two facts, kept apart: the stored switch (`boxesEnabled`, the user's to
  // flip either way) and whether this machine can boot a box right now
  // (`boxRuntimeProblem`). The switch shows the stored value and only
  // turning it *on* needs the runtime: a switch left on by a machine that
  // has since lost the helper must still read as on and still turn off. The
  // box controls below need both.
  const boxRuntimeProblem =
    boxProbe && !boxProbe.runtime_ok ? boxProbe.problems.join("; ") : undefined;
  const boxUsable = unlocked && boxesEnabled && !!boxProbe?.runtime_ok;
  // A pinned `box` stays visible — and so escapable — while the runtime is
  // out: the option is listed with the problem as its hint, and the warning
  // below says what to do, as it does for a container engine that went away.
  const boxOffered = boxUsable || (unlocked && sandboxEngine === "box");
  const setup = boxSetup(boxProbe, boxImage, boxTools);
  // What Automatic lands on right now, in two words: the Select's hint.
  const autoHint = !boxUsable
    ? "macOS sandbox"
    : setup.state === "ready"
      ? "Isolated VM"
      : "macOS sandbox until the VM is ready";
  // Locked, `auto` is the macOS sandbox, so that is the option shown selected;
  // picking it back writes `auto`, which keeps the user on the automatic
  // choice (and so on boxes) once they are released.
  const shownEngine: SandboxEngine =
    !unlocked && sandboxEngine === "auto" ? "sandbox-exec" : sandboxEngine;
  const fallbackLabel = unlocked ? "Automatic" : "Seatbelt";

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
            (unlocked
              ? "Automatic runs each new workspace in an isolated VM once one is ready, and in the macOS sandbox until then. Applies to new workspaces only."
              : "Docker and Podman run agents in a Linux container. Applies to new workspaces only.")
          }
        >
          <Select<SandboxEngine>
            value={shownEngine}
            ariaLabel="Sandbox engine"
            disabled={!!hostGate}
            options={[
              ...(unlocked ? [{ value: "auto" as const, label: "Automatic", hint: autoHint }] : []),
              { value: "sandbox-exec", label: "macOS sandbox (Seatbelt)" },
              ...(boxOffered
                ? [
                    {
                      value: "box" as const,
                      label: "Isolated VM (box)",
                      hint: !boxesEnabled ? "Turned off below" : (boxRuntimeProblem ?? setup.text),
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
            onChange={(v) => void setSandboxEngine(!unlocked && v === "sandbox-exec" ? "auto" : v)}
          />
        </SetRow>
        {sandboxEngine === "docker" && dockerProbe && !dockerAvailable && (
          <div className="set-sandbox-warn">
            Docker is selected but{" "}
            {dockerProbe.status === "daemon-down" ? "the daemon isn't running" : "isn't installed"}.
            New agents won't launch until it's available.{" "}
            {dockerProbe.status === "daemon-down" ? "Start" : "Install"} Docker Desktop, or switch
            back to {fallbackLabel}.
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
            or switch back to {fallbackLabel}.
          </div>
        )}
        {sandboxEngine === "box" && !boxUsable && boxProbe && (
          <div className="set-sandbox-warn">
            Isolated VM is selected but{" "}
            {!boxesEnabled
              ? "isolated VMs are turned off below"
              : `isn't available: ${boxRuntimeProblem}`}
            . New agents won't launch until it is. Switch back to Automatic, or turn isolated VMs
            on.
          </div>
        )}
        <ContainerAuth />
      </SetGroup>

      {unlocked && (
        <SetGroup label="Isolated VM">
          <SetRow
            title="Use isolated VMs when available"
            sub={
              hostGate ??
              "A micro-VM per workspace that shares no folder with your Mac and reaches only the project's allowed hosts. Its image downloads once, in the background; until it's in, workspaces use the macOS sandbox. Projects that build a macOS or iOS app stay on your Mac."
            }
          >
            <SetToggle
              on={boxesEnabled}
              onClick={() => void setBoxesEnabled(!boxesEnabled)}
              disabled={!!hostGate || !boxProbe || (!!boxRuntimeProblem && !boxesEnabled)}
              tip={hostGate ?? (!boxProbe ? "Checking…" : boxRuntimeProblem)}
            />
          </SetRow>
          {boxesEnabled && boxRuntimeProblem && (
            <div className="set-row-s text-sm set-box-setup" data-state="error">
              <span>Not available on this Mac: {boxRuntimeProblem}</span>
            </div>
          )}
          {boxUsable && <BoxSetupStatus />}
        </SetGroup>
      )}

      {boxUsable && <BoxKnobs />}
      <ContainerLaunchKnobs runtime="docker" />
      <ContainerLaunchKnobs runtime="podman" last={!boxUsable} />
      {boxUsable && <EgressGlobal last />}
    </div>
  );
}
