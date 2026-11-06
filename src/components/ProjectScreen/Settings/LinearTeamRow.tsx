import { useEffect, useState } from "react";
import { api, type LinearTeam } from "@/api";
import { SetRow } from "@/components/SettingsScreen/primitives";
import { Button } from "@/components/ui/Button";
import { Select } from "@/components/ui/Select";
import { LINEAR_TEAM_ID_KEY, LINEAR_TEAM_NAME_KEY } from "@/storage/projectSettings";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { useProjectSettings } from "@/util/useProjectSettings";

/** The Linear team this project draws tickets from. With a team set, Linear
 *  tickets join GitHub issues in the Home inbox and the composer's issue
 *  picker. The connection itself is this Mac's and lives in Settings ›
 *  Account; this row only links there while it is missing. */
export function LinearTeamRow({ projectId }: { projectId: string }) {
  const linear = useAppStore((s) => s.linear);
  const refreshLinear = useAppStore((s) => s.refreshLinear);
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);
  const [teams, setTeams] = useState<LinearTeam[] | null>(null);
  // The team is the host's row — the composer's issue picker reads it.
  const { settings, save } = useProjectSettings(projectId);
  const gate = useGate("projectSettings");
  const teamId = settings?.[LINEAR_TEAM_ID_KEY] ?? "";

  const connected = !!linear?.authenticated;

  // Reflect the current connection on open (it may have changed in Settings).
  useEffect(() => {
    void refreshLinear();
  }, [refreshLinear]);

  // Load the workspace's teams once connected, for the picker.
  useEffect(() => {
    if (!connected) return;
    let cancelled = false;
    api
      .linearListTeams()
      .then((list) => {
        if (!cancelled) setTeams(list);
      })
      .catch(() => {
        if (!cancelled) setTeams([]);
      });
    return () => {
      cancelled = true;
    };
  }, [connected]);

  function pickTeam(nextId: string) {
    const team = teams?.find((t) => t.id === nextId);
    // Blank clears both rows: Linear off for this project.
    save(LINEAR_TEAM_ID_KEY, nextId || null);
    save(LINEAR_TEAM_NAME_KEY, nextId ? (team?.name ?? "") : null);
  }

  if (!connected) {
    return (
      <SetRow title="Linear team" sub="Connect Linear in Settings › Account.">
        <Button variant="outline" size="sm" onClick={() => openSettingsScreen("account")}>
          Open Account
        </Button>
      </SetRow>
    );
  }

  const options = [
    { value: "", label: "None" },
    ...(teams === null
      ? // Keep a saved selection visible while teams load.
        teamId
        ? [{ value: teamId, label: "Loading…" }]
        : []
      : teams.map((t) => ({ value: t.id, label: t.name, hint: t.key }))),
  ];

  return (
    <SetRow
      title="Linear team"
      sub={gate ?? "Its open tickets join GitHub issues on Home and in the composer."}
    >
      <Select
        value={teamId}
        options={options}
        ariaLabel="Linear team"
        disabled={!!gate || settings === null}
        onChange={pickTeam}
      />
    </SetRow>
  );
}
