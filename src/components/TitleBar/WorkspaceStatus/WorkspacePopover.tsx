import type { AgentRecord } from "@/api";
import { ProviderIcon } from "@/components/ProviderIcon";
import { boxSetup } from "@/components/SettingsScreen/Sandbox/boxSetup";
import { providerChip } from "@/data/providers";
import { useAppStore } from "@/store";
import { useProjectSettings } from "@/util/useProjectSettings";
import { BoxActions } from "./BoxActions";
import { type DotStatus, STATUS_LABEL } from "./derive";
import { IsolationIcon } from "./IsolationIcon";
import { boxPhaseLabel, isolationOf, seatbeltNote } from "./isolation";
import { StatusDot } from "./StatusDot";

/** The workspace segment's hover/focus popover: who + what, then where it
 *  runs — the isolation, and for a box its live phase and the one action it
 *  takes here; for a seatbelt workspace, where the isolated VM stands, so a
 *  workspace that started before the download reads as early rather than
 *  as the rule. */
export function WorkspacePopover({ agent, status }: { agent: AgentRecord; status: DotStatus }) {
  const boxState = useAppStore((s) => s.boxStates[agent.id]);
  const isolation = isolationOf(agent.sandbox_engine);
  const isBox = isolation.kind === "box";
  const failure = isBox && boxState?.phase === "failed" ? boxState.reason : null;

  return (
    <div className="ws-pop" role="dialog" aria-label={`${agent.name} workspace`}>
      <div className="ws-pop-head">
        <StatusDot status={status} big />
        <div className="ws-pop-id">
          <div className="ws-pop-name">{agent.name}</div>
          <div className="ws-pop-sub">
            {STATUS_LABEL[status]}
            {agent.task && <> · {agent.task}</>}
          </div>
        </div>
        <ProviderIcon slug={agent.provider} {...providerChip(agent.provider)} size={22} />
      </div>

      <div className="ws-pop-div" />

      <div className="ws-pop-iso">
        <IsolationIcon isolation={isolation} boxPhase={boxState?.phase} size={14} />
        <div className="ws-pop-iso-text">
          <div className="ws-pop-iso-title">
            {isolation.title}
            {isBox && boxState && (
              <span className={`ws-pop-iso-phase ${boxState.phase}`}>
                {boxPhaseLabel(boxState)}
              </span>
            )}
          </div>
          {failure && <div className="ws-pop-iso-fail">{failure}</div>}
          <div className="ws-pop-iso-hint">{isolation.hint}</div>
          {isolation.kind === "seatbelt" && <VmNote repoPath={agent.repos[0]?.repo_path} />}
        </div>
      </div>

      {isBox && <BoxActions agent={agent} />}
    </div>
  );
}

/** Where the isolated VM stands, under a seatbelt workspace. */
function VmNote({ repoPath }: { repoPath: string | undefined }) {
  const boxesEnabled = useAppStore((s) => s.boxesEnabled);
  const probe = useAppStore((s) => s.boxProbe);
  const image = useAppStore((s) => s.boxImage);
  const tools = useAppStore((s) => s.boxTools);
  const projectId = useAppStore(
    (s) => s.workspace?.projects.find((p) => p.path === repoPath)?.project_id,
  );
  const { settings } = useProjectSettings(projectId);
  const note = seatbeltNote(
    boxesEnabled && !!probe?.runtime_ok,
    boxSetup(probe, image, tools),
    settings?.["sandbox.placement"],
  );
  return note ? <div className="ws-pop-iso-note">{note}</div> : null;
}
