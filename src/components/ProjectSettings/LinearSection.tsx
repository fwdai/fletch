import { useEffect, useState } from "react";
import { api, type LinearTeam } from "@/api";
import {
  deleteProjectSetting,
  getProjectSettings,
  LINEAR_TEAM_ID_KEY,
  LINEAR_TEAM_NAME_KEY,
  setProjectSetting,
} from "@/storage/projectSettings";
import { useAppStore } from "@/store";

/** Linear integration: the account connection (an API key, app-wide) and the
 *  team this project draws tickets from. With a team set, Linear tickets join
 *  GitHub issues in the Home inbox and the composer's issue picker — the
 *  generalized tracker plumbing serves both from one list. */
export function LinearSection({ projectId }: { projectId: string }) {
  const linear = useAppStore((s) => s.linear);
  const refreshLinear = useAppStore((s) => s.refreshLinear);

  const [apiKey, setApiKey] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [teams, setTeams] = useState<LinearTeam[] | null>(null);
  const [teamId, setTeamId] = useState("");

  const connected = !!linear?.authenticated;

  // Reflect the current connection on open (it may have changed in another
  // project's settings) and load this project's saved team.
  useEffect(() => {
    void refreshLinear();
  }, [refreshLinear]);
  useEffect(() => {
    let cancelled = false;
    getProjectSettings(projectId)
      .then((all) => {
        if (!cancelled) setTeamId(all[LINEAR_TEAM_ID_KEY] ?? "");
      })
      .catch((e) => console.error("load linear team failed", e));
    return () => {
      cancelled = true;
    };
  }, [projectId]);

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

  async function connect() {
    const key = apiKey.trim();
    if (!key || connecting) return;
    setConnecting(true);
    setError(null);
    try {
      await api.linearConnect(key);
      setApiKey("");
      await refreshLinear();
    } catch (e) {
      setError(String(e));
    } finally {
      setConnecting(false);
    }
  }

  async function disconnect() {
    setError(null);
    try {
      await api.linearDisconnect();
      setTeams(null);
      await refreshLinear();
    } catch (e) {
      setError(String(e));
    }
  }

  function pickTeam(nextId: string) {
    setTeamId(nextId);
    const team = teams?.find((t) => t.id === nextId);
    const write = nextId
      ? Promise.all([
          setProjectSetting(projectId, LINEAR_TEAM_ID_KEY, nextId),
          setProjectSetting(projectId, LINEAR_TEAM_NAME_KEY, team?.name ?? ""),
        ])
      : Promise.all([
          deleteProjectSetting(projectId, LINEAR_TEAM_ID_KEY),
          deleteProjectSetting(projectId, LINEAR_TEAM_NAME_KEY),
        ]);
    write.catch((e) => console.error("save linear team failed", e));
  }

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Linear</h2>
        <p className="ps-section-lead text-sm">
          Connect Linear and pick the team this project draws tickets from. Its open tickets then
          appear alongside GitHub issues on Home and in the composer&rsquo;s issue picker, and a
          started ticket is closed by the agent&rsquo;s PR.
        </p>
      </header>

      {!connected ? (
        <div className="ps-field">
          <label className="ps-label text-sm" htmlFor="ps-linear-key">
            Personal API key
          </label>
          <div className="ps-name-row">
            <input
              id="ps-linear-key"
              className="ps-input text-base"
              type="password"
              placeholder="lin_api_…"
              value={apiKey}
              spellCheck={false}
              autoComplete="off"
              onChange={(e) => setApiKey(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  void connect();
                }
              }}
            />
            <button
              type="button"
              className="ps-btn"
              disabled={!apiKey.trim() || connecting}
              onClick={() => void connect()}
            >
              {connecting ? "Connecting…" : "Connect"}
            </button>
          </div>
          <p className="ps-section-lead text-sm">
            Create one in Linear under Settings → Security &amp; access → API keys. The key is
            stored in your keychain and shared by all projects.
          </p>
        </div>
      ) : (
        <>
          <div className="ps-field ps-name-row">
            <span className="ps-label text-sm">
              Connected{linear?.user ? ` as ${linear.user}` : ""}
            </span>
            <button type="button" className="ps-btn" onClick={() => void disconnect()}>
              Disconnect
            </button>
          </div>
          <div className="ps-field">
            <label className="ps-label text-sm" htmlFor="ps-linear-team">
              Team for this project
            </label>
            <select
              id="ps-linear-team"
              className="ps-input text-base"
              value={teamId}
              onChange={(e) => pickTeam(e.target.value)}
            >
              <option value="">None — Linear off for this project</option>
              {teams === null
                ? // Keep a saved selection visible while teams load.
                  teamId && <option value={teamId}>Loading…</option>
                : teams.map((t) => (
                    <option key={t.id} value={t.id}>
                      {t.name} ({t.key})
                    </option>
                  ))}
            </select>
          </div>
        </>
      )}
      {error && <div className="ps-error text-sm">{error}</div>}
    </section>
  );
}
