// Onboarding — a cinematic, native-feeling entry into Quorum. Full-screen
// overlay shown to new users on first launch and re-openable from Settings ›
// General. Ported from the design prototype (onboarding/app.jsx): ambient
// stage, step sequence, cinematic transitions, progress rail, keyboard nav.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { useCallback, useEffect, useRef, useState } from "react";
import { getOrCreateAccount, linkOAuthAccount, type OAuthProfile } from "../../storage/accounts";
import { useAppStore } from "../../store";
import { Icon } from "../Icon";
import { Ambient } from "./Ambient";
import { BEATS } from "./beats";
import { DeviceCode, type DeviceCodeInfo } from "./DeviceCode";
import { Beat, IgniteStep, WelcomeStep } from "./steps";
import "./onboarding.css";

// flat step model: welcome · three feature beats · finale
type Step = { kind: "welcome" } | { kind: "beat"; beat: number } | { kind: "ignite" };

const STEPS: Step[] = [
  { kind: "welcome" },
  { kind: "beat", beat: 0 },
  { kind: "beat", beat: 1 },
  { kind: "beat", beat: 2 },
  { kind: "ignite" },
];
const RAIL_LEN = 4; // welcome..last beat (finale excluded)

export function Onboarding() {
  const closeOnboarding = useAppStore((s) => s.closeOnboarding);
  const refreshAccount = useAppStore((s) => s.refreshAccount);

  const [idx, setIdx] = useState(0);
  const [phase, setPhase] = useState<"in" | "out">("in");
  const [ready, setReady] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);

  const [device, setDevice] = useState<DeviceCodeInfo | null>(null);
  const [authError, setAuthError] = useState<string | null>(null);
  // Monotonic id for the active sign-in. Bumping it (on cancel or a new
  // attempt) invalidates any in-flight oauth_device_login so its late result
  // is ignored — the backend poll keeps running until it expires, but its
  // outcome no longer touches the UI or the DB.
  const authRunRef = useRef(0);

  const step = STEPS[idx];

  // Trigger the staggered reveal after the step mounts / changes. setTimeout
  // (not rAF) so it still fires when the window is backgrounded.
  useEffect(() => {
    setReady(false);
    const id = window.setTimeout(() => setReady(true), 40);
    return () => window.clearTimeout(id);
  }, [idx]);

  const go = useCallback(
    (next: number) => {
      if (next < 0 || next >= STEPS.length || next === idx) return;
      setPhase("out");
      window.setTimeout(() => {
        setIdx(next);
        setPhase("in");
        const stage = document.querySelector(".ob-view");
        if (stage) stage.scrollTop = 0;
      }, 380);
    },
    [idx],
  );

  const next = useCallback(() => go(idx + 1), [go, idx]);
  const back = useCallback(() => go(idx - 1), [go, idx]);

  const onAuth = useCallback(
    async (provider: string) => {
      if (busy) return;
      const runId = ++authRunRef.current;
      const stale = () => authRunRef.current !== runId;
      setBusy(provider);
      setAuthError(null);
      setDevice(null);
      // Default to a no-op so the finally can always call it — listen() lives
      // inside the try so an IPC failure is caught and surfaced, not thrown
      // unhandled (which would strand the cancel-less loading panel).
      let unlisten: () => void = () => {};
      try {
        // The backend emits the user code once the provider issues it; show it
        // and open the verification page in the user's browser.
        unlisten = await listen<{
          provider: string;
          user_code: string;
          verification_uri: string;
        }>("oauth:device-code", (e) => {
          if (stale()) return;
          setDevice({
            provider: e.payload.provider,
            userCode: e.payload.user_code,
            verificationUri: e.payload.verification_uri,
          });
          void openExternal(e.payload.verification_uri).catch(() => {});
        });
        const profile = await invoke<OAuthProfile>("oauth_device_login", {
          provider,
        });
        if (stale()) return; // cancelled or superseded — drop the result
        const account = await getOrCreateAccount();
        await linkOAuthAccount(account.id, profile);
        await refreshAccount();
        if (stale()) return;
        // Keep the device panel visible through the transition-out — clearing
        // `device`/`busy` here would flash the welcome buttons mid-fade. The
        // step-change effect below tears them down once we've left welcome.
        go(1);
      } catch (err) {
        if (stale()) return;
        setAuthError(String(err));
        setBusy(null);
      } finally {
        unlisten();
      }
    },
    [busy, go, refreshAccount],
  );

  const cancelAuth = useCallback(() => {
    authRunRef.current++; // invalidate any in-flight sign-in
    setDevice(null);
    setAuthError(null);
    setBusy(null);
  }, []);

  // Once we leave the welcome step, tear down any auth/device state so the
  // panel fades out cleanly and a later return to welcome shows the sign-in
  // buttons, not a stale code panel.
  useEffect(() => {
    if (step.kind !== "welcome") {
      setBusy(null);
      setDevice(null);
      setAuthError(null);
    }
  }, [step.kind]);

  // Finale handoff: just drop into the real app. Its empty state prompts the
  // user to add their first repo from the sidebar — no auto-picker.
  const onEnter = useCallback(() => closeOnboarding(), [closeOnboarding]);

  // keyboard navigation
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement | null)?.tagName;
      const inField = tag === "TEXTAREA" || tag === "INPUT";
      if (e.key === "Escape") {
        closeOnboarding();
      } else if (e.key === "Enter" && step.kind === "welcome" && !busy) {
        onAuth("github");
      } else if (
        (e.key === "ArrowRight" || e.key === "Enter") &&
        step.kind === "beat" &&
        !inField
      ) {
        e.preventDefault();
        next();
      } else if (e.key === "ArrowLeft" && idx > 0 && step.kind !== "ignite" && !inField) {
        back();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [idx, busy, step.kind, next, back]);

  let content = null;
  if (step.kind === "welcome")
    content =
      busy || device || authError ? (
        <DeviceCode info={device} error={authError} onCancel={cancelAuth} />
      ) : (
        <WelcomeStep onAuth={onAuth} busy={busy} />
      );
  else if (step.kind === "beat") content = <Beat beat={BEATS[step.beat]} />;
  else if (step.kind === "ignite") content = <IgniteStep onEnter={onEnter} />;

  const showFoot = step.kind !== "ignite";
  const showBack = idx > 0 && step.kind !== "ignite";
  const showNext = step.kind === "beat";

  return (
    <div className="ob">
      <div className="ob-tb" data-tauri-drag-region>
        <div className="ob-tb-gutter" data-tauri-drag-region />
        <div className="ob-tb-mark text-xs">
          <span className="d" />
          <span>FLETCH</span>
        </div>
        <div className="ob-tb-right">
          {step.kind !== "ignite" && (
            <span className="ob-step-count text-xs">
              <b>{Math.min(idx + 1, RAIL_LEN)}</b> / {RAIL_LEN}
            </span>
          )}
          {step.kind !== "ignite" && (
            <button className="ob-skip text-sm" onClick={() => go(STEPS.length - 1)}>
              Skip
            </button>
          )}
          <button
            className="ob-close"
            title="Close (Esc)"
            aria-label="Close onboarding"
            onClick={() => closeOnboarding()}
          >
            <Icon name="close" size={15} />
          </button>
        </div>
      </div>

      <div className="ob-stage">
        <Ambient phase={idx} />

        <div
          className={`ob-view ${ready ? "ready" : ""} ${phase === "out" ? "out" : ""}`}
          key={idx}
        >
          {content}
        </div>

        {showFoot && (
          <div className="ob-foot">
            <div className="ob-foot-l">
              {showBack && (
                <button className="ob-back text-base" onClick={back}>
                  <Icon name="chevL" /> Back
                </button>
              )}
            </div>
            <div className="ob-rail">
              {Array.from({ length: RAIL_LEN }).map((_, i) => (
                <span
                  key={i}
                  className={`seg ${i < idx ? "done" : ""} ${i === idx ? "cur" : ""}`}
                  onClick={() => {
                    if (i <= idx) go(i);
                  }}
                />
              ))}
            </div>
            <div className="ob-foot-r">
              {showNext && (
                <button className="ob-next text-base" onClick={next}>
                  Continue <Icon name="arrowR" />
                </button>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
