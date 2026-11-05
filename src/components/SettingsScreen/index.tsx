import { open as openExternal } from "@tauri-apps/plugin-shell";
import { Fragment, lazy, Suspense, useMemo } from "react";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { IS_MAC } from "@/util/platform";
import { WorkflowsPane } from "@/workflows/builder";
import pkg from "../../../package.json";
import { AccountPane } from "./AccountPane";
import { CustomAgentsPane } from "./CustomAgents";
import { DictationPane } from "./Dictation";
import { ExperimentalPane } from "./ExperimentalPane";
import { GeneralPane } from "./GeneralPane";
import { GitPane } from "./GitPane";
import { LayoutPane } from "./LayoutPane";
import { McpServersPane } from "./McpServers";
import { developerUnlocked, settingsNav } from "./nav";
import { ProvidersPane } from "./ProvidersPane";
import { RemoteControlPane } from "./RemoteControl";
import { ReviewPane } from "./ReviewPane";
import { SandboxPane } from "./Sandbox";
import { NetworkPane } from "./Sandbox/NetworkPane";
import { ShortcutsPane } from "./ShortcutsPane";
import { SkillsPane } from "./Skills";

// The libkrunfw release packaging/libkrun/build.sh pins; its tree names the
// kernel tarball and config the bundled kernel was built from.
const KERNEL_SOURCE_URL = "https://github.com/libkrun/libkrunfw/tree/v5.6.2";

// Lazily loaded — code-split into its own chunk, fetched only when the Developer
// section is actually opened. Visibility is gated at render (dev builds, or an
// admin user in production), not by dropping the chunk from the build.
const DeveloperPane = lazy(() =>
  import("./DeveloperPane").then((m) => ({ default: m.DeveloperPane })),
);

/** Dedicated full-screen settings surface. Rendered in place of the workspace
 *  panes while `settingsScreenOpen` is true. The quick-settings popover stays
 *  for fast access; this is the comprehensive surface. */
export function SettingsScreen() {
  const section = useAppStore((s) => s.settingsSection);
  const setSection = useAppStore((s) => s.setSettingsSection);
  const close = useAppStore((s) => s.closeSettingsScreen);
  const admin = useAppStore((s) => s.admin);
  const workflowGate = useGate("workflows");

  const showDeveloper = developerUnlocked(admin);
  const nav = useMemo(
    () => settingsNav({ workflows: !workflowGate, mac: IS_MAC, developer: showDeveloper }),
    [showDeveloper, workflowGate],
  );

  // A section with no nav entry (a stale "developer" after the admin flag
  // flipped off, or "dictation" off-Mac) falls back to General.
  const visible = nav.some((n) => n.id === section || n.subsections?.includes(section));

  return (
    <div className="set-screen">
      <nav className="set-nav">
        <button className="set-back flex-center text-base" onClick={close}>
          <Icon name="chevL" size={13} />
          <span>Back to app</span>
        </button>
        <div className="set-nav-list">
          {nav.map((n, i) => {
            // A header opens wherever the group changes between consecutive
            // visible entries, so a group with nothing visible never shows one.
            const opensGroup = !!n.group && n.group !== nav[i - 1]?.group;
            // A grouped entry stays active for any of its sub-sections, and
            // clicking it while already inside the group keeps the current
            // sub-tab rather than snapping back to the default.
            const active = n.subsections ? n.subsections.includes(section) : section === n.id;
            return (
              <Fragment key={n.id}>
                {opensGroup && <div className="set-nav-group mono text-xs">{n.group}</div>}
                <button
                  className={`set-nav-item flex-center text-base ${active ? "active" : ""}`}
                  onClick={() => {
                    if (active && n.subsections) return;
                    setSection(n.id);
                  }}
                >
                  <Icon name={n.icon} size={14} />
                  <span>{n.label}</span>
                </button>
              </Fragment>
            );
          })}
        </div>
        <div className="set-nav-foot text-xs">
          <span className="mono">Fletch</span>
          <span className="mono dim">v{pkg.version}</span>
        </div>
        {IS_MAC && (
          // The macOS app bundles a GPL-2.0 kernel (libkrunfw) for the box
          // sandbox, so its source is offered next to the version (NOTICE).
          <div className="set-nav-licence text-xs">
            Box runtime: libkrun (Apache-2.0) and a Linux 6.12 kernel (GPL-2.0),{" "}
            <a
              href={KERNEL_SOURCE_URL}
              onClick={(e) => {
                e.preventDefault();
                void openExternal(KERNEL_SOURCE_URL).catch(() => {});
              }}
            >
              source
            </a>
          </div>
        )}
      </nav>

      <div className="set-main">
        <div className="set-content">
          {!visible && <GeneralPane />}
          {visible && section === "general" && <GeneralPane />}
          {visible && section === "account" && <AccountPane />}
          {visible && section === "layout" && <LayoutPane />}
          {visible && section === "shortcuts" && <ShortcutsPane />}
          {visible && section === "dictation" && <DictationPane />}
          {visible && section === "git" && <GitPane />}
          {visible && section === "review" && <ReviewPane />}
          {visible && section === "sandbox" && <SandboxPane />}
          {visible && section === "network" && <NetworkPane />}
          {visible && section === "remote" && <RemoteControlPane />}
          {visible && section === "providers" && <ProvidersPane />}
          {visible && section === "agents" && <CustomAgentsPane />}
          {visible && section === "workflows" && <WorkflowsPane />}
          {visible && section === "skills" && <SkillsPane />}
          {visible && section === "tools" && <McpServersPane />}
          {visible && section === "experimental" && <ExperimentalPane />}
          {visible && section === "developer" && (
            <Suspense fallback={null}>
              <DeveloperPane />
            </Suspense>
          )}
        </div>
      </div>
    </div>
  );
}
