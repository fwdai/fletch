import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { readFile } from "@tauri-apps/plugin-fs";
import { useCallback, useEffect, useState } from "react";
import { api } from "@/api";
import type { ProjectIconCandidate, ProjectRef } from "@/api/types/agent";
import { Button } from "@/components/ui/Button";
import { Scrim } from "@/components/ui/Scrim";
import { useAppStore } from "@/store";
import { activeEntry, useGate } from "@/store/capabilities";
import { bytesToBase64 } from "@/util/base64";
import { useProjectSettings } from "@/util/useProjectSettings";
import { PROJECT_GLYPHS } from "./glyphs";
import { ProjectIcon } from "./index";
import { ProjectMark } from "./ProjectMark";

const ICON_SOURCE_KEY = "icon.source";
const IMAGE_FILTERS = [
  { name: "Image", extensions: ["png", "svg", "jpg", "jpeg", "webp", "gif", "ico"] },
];

/** The project's icon as a button, opening the choices: what the repository
 *  offers (a fresh scan each time), a file from this Mac, the built-in glyphs,
 *  the initial, and "Detect again", which takes the scan's top pick. On a
 *  paired host the file choice is hidden — its picker can only see this Mac. */
export function IconPicker({ project }: { project: ProjectRef }) {
  const id = project.project_id;
  const [open, setOpen] = useState(false);
  const [found, setFound] = useState<ProjectIconCandidate[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const setProjectIcon = useAppStore((s) => s.setProjectIcon);
  const local = useAppStore((s) => activeEntry(s).kind === "local");
  const gate = useGate("projectIcon");
  const current = useProjectSettings(open ? id : null).settings?.[ICON_SOURCE_KEY] ?? null;

  const scan = useCallback(async () => {
    setFound(null);
    try {
      const list = await api.projectIconCandidates(id);
      setFound(list);
      return list;
    } catch (e) {
      setError(String(e));
      setFound([]);
      return [];
    }
  }, [id]);

  useEffect(() => {
    if (open) void scan();
  }, [open, scan]);

  const choose = async (source: string | null, bytes?: string) => {
    try {
      await setProjectIcon(id, source, bytes);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  const chooseFile = async () => {
    try {
      const path = await openDialog({
        title: "Choose an icon",
        multiple: false,
        directory: false,
        filters: IMAGE_FILTERS,
      });
      if (typeof path !== "string") return;
      await choose("file", bytesToBase64(await readFile(path)));
    } catch (e) {
      setError(String(e));
    }
  };

  const detectAgain = async () => {
    const [top] = await scan();
    if (top) await choose(`detected:${top.path}`);
  };

  return (
    <span className="dd-anchor">
      <button
        type="button"
        className="proj-icon-btn tip"
        data-tip={gate ?? (open ? undefined : "Change icon")}
        aria-label="Project icon"
        aria-expanded={open}
        disabled={!!gate}
        onClick={() => setOpen(!open)}
      >
        <ProjectIcon project={project} size={28} />
      </button>

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <div className="pop proj-icon-pop">
            <div className="proj-icon-label">In the repository</div>
            {found === null ? (
              <div className="proj-icon-note">Looking…</div>
            ) : found.length === 0 ? (
              <div className="proj-icon-note">No icon found.</div>
            ) : (
              <div className="proj-icon-grid">
                {found.map((c) => (
                  <button
                    key={c.path}
                    type="button"
                    className={`proj-icon-choice ${current === `detected:${c.path}` ? "active" : ""}`}
                    title={`${c.path}\n${c.source}`}
                    onClick={() => void choose(`detected:${c.path}`)}
                  >
                    {/* Past the scan's thumbnail budget a candidate comes without
                        its bytes and draws the initial; choosing it still works. */}
                    <ProjectMark
                      name={project.name}
                      icon={c.data ? { mime: c.mime, data: c.data } : null}
                      size={32}
                    />
                    <span className="proj-icon-cap">{c.path}</span>
                  </button>
                ))}
              </div>
            )}

            <div className="proj-icon-label">Built in</div>
            <div className="proj-icon-glyphs">
              {PROJECT_GLYPHS.map((g) => (
                <button
                  key={g.id}
                  type="button"
                  className={`proj-icon-choice ${current === `builtin:${g.id}` ? "active" : ""}`}
                  title={g.label}
                  aria-label={g.label}
                  onClick={() => void choose(`builtin:${g.id}`)}
                >
                  <ProjectMark name={project.name} icon={{ glyph: g.id }} size={28} />
                </button>
              ))}
            </div>

            <div className="proj-icon-actions">
              {local && (
                <Button variant="outline" size="sm" onClick={() => void chooseFile()}>
                  Choose file…
                </Button>
              )}
              {/* Recorded, not cleared: a relocate must not detect over it. */}
              <Button
                variant="outline"
                size="sm"
                disabled={current === "initial"}
                onClick={() => void choose("initial")}
              >
                Use initial
              </Button>
              <Button variant="ghost" size="sm" onClick={() => void detectAgain()}>
                Detect again
              </Button>
            </div>
            {error && <div className="proj-icon-note error">{error}</div>}
          </div>
        </>
      )}
    </span>
  );
}
