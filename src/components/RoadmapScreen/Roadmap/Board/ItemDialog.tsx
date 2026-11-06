import { useMemo, useState } from "react";
import type { Horizon, RoadmapItem } from "@/api";
import { Icon } from "@/components/Icon";
import { Segmented } from "@/components/Settings/Segmented";
import { Button } from "@/components/ui/Button";
import { Modal, ModalBody, ModalFooter } from "@/components/ui/Modal";
import { Select, type SelectOption } from "@/components/ui/Select";
import { TextArea, TextInput } from "@/components/ui/TextInput";
import { addDep, removeDep, suggestCodes } from "../depsField";
import { HORIZONS } from "../types";
import type { ProjectWorkflows } from "../useProjectWorkflows";

/** The fields a human fills in. Deliberately a subset of the row: `code`,
 *  `status`, `source` and everything the agent runtime owns are not typed by
 *  hand. Shaped so it can be passed straight to either `roadmap_create_item` or
 *  `roadmap_update_item` — the same keys mean the same thing in both. */
export interface ItemDraft {
  title: string;
  why: string;
  horizon: Horizon;
  area: string | null;
  accept: string[];
  /** Codes this item must land after. The queue skips an item whose deps aren't
   *  `done`, so this is the user's half of dispatch order (rank is the other).
   *  Validated against the board here for speed and against the real dependency
   *  graph backend-side, which is the authority — only it can see a loop. */
  deps: string[];
  /** Which workflow the queue dispatches this under. `null` means "whatever the
   *  project's default is at dispatch time" — resolved by the Rust drainer, not
   *  frozen here, so changing the project default moves every un-pinned item. */
  workflow_def_id: string | null;
}

/** The picker's stand-in for `workflow_def_id: null`. */
const PROJECT_DEFAULT = "default";

const HORIZON_OPTIONS = HORIZONS.map((h) => ({ value: h.id, label: h.label }));

/** Acceptance criteria are edited as lines, not as a list widget: they're
 *  written in one sitting, and a textarea beats five inputs and an "add" button
 *  for that. Blank lines are dropped. */
const linesToList = (text: string) =>
  text
    .split("\n")
    .map((l) => l.trim().replace(/^[-*]\s*/, ""))
    .filter(Boolean);

interface Props {
  /** The row being edited, or `null` to create one. */
  item: RoadmapItem | null;
  /** Where a new item lands — the group whose "+" was pressed. */
  horizon: Horizon;
  /** The workflows this project could run the item under, and which one is its
   *  default. */
  workflows: ProjectWorkflows;
  /** Every code on the board, for the dep field — exact matches only, because
   *  the prefix varies per project. */
  codes: ReadonlySet<string>;
  onClose: () => void;
  onSave: (draft: ItemDraft) => Promise<unknown>;
  /** Absent while creating: there is nothing to delete yet. */
  onDelete?: () => Promise<unknown>;
}

/** Create or edit one roadmap item. The same form for both, because the fields
 *  are the same and a separate "quick add" would drift from it. */
export function ItemDialog({ item, horizon, workflows, codes, onClose, onSave, onDelete }: Props) {
  const [title, setTitle] = useState(item?.title ?? "");
  const [why, setWhy] = useState(item?.why ?? "");
  const [place, setPlace] = useState<Horizon>(item?.horizon ?? horizon);
  const [area, setArea] = useState(item?.area ?? "");
  const [accept, setAccept] = useState((item?.accept ?? []).join("\n"));
  const [deps, setDeps] = useState<string[]>(item?.deps ?? []);
  /** What is typed in the dep box but not yet a chip. */
  const [depDraft, setDepDraft] = useState("");
  const [workflow, setWorkflow] = useState<string>(item?.workflow_def_id ?? PROJECT_DEFAULT);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const canSave = title.trim().length > 0 && !busy;

  /** Every code this item could still depend on — the board minus itself and
   *  minus the chips already there. Independent of what is typed, so the field
   *  doesn't disable itself mid-word when a query matches nothing. */
  const depsAvailable = useMemo(
    () => suggestCodes("", codes, deps, item?.code),
    [codes, deps, item?.code],
  );
  /** …narrowed to what has been typed, for the suggestion list. Filtered here
   *  rather than left to the datalist, whose matching rules vary by engine. */
  const depOptions = useMemo(
    () => suggestCodes(depDraft, codes, deps, item?.code),
    [depDraft, codes, deps, item?.code],
  );

  /** Turn the typed token into a chip, or say why it can't be one. Refusals the
   *  board can answer (unknown code, self) land here; a *loop* comes back from
   *  the backend on save, into the same slot. */
  const commitDep = () => {
    const { deps: next, error: refusal } = addDep(deps, depDraft, codes, item?.code);
    if (refusal) {
      setError(refusal);
      return;
    }
    if (next) {
      setDeps(next);
      setError(null);
    }
    setDepDraft("");
  };

  // "Project default" names the definition when there is one, so the choice is
  // legible rather than a promise the user has to go and verify. When there
  // isn't one, the option says so — queueing still works, it just won't move
  // until a workflow exists, and the card will say that too.
  const workflowOptions: SelectOption<string>[] = [
    {
      value: PROJECT_DEFAULT,
      label: "Project default",
      hint: workflows.defaultName ?? (workflows.ready ? "none set yet" : undefined),
    },
    ...workflows.definitions.map((d) => ({
      value: d.id,
      label: d.name,
      hint: d.description || undefined,
    })),
  ];

  const save = async () => {
    if (!canSave) return;
    // A code left in the box is meant as a dep — saving without it would drop
    // it silently. A refusal stops the save, with the reason on screen.
    const pending = addDep(deps, depDraft, codes, item?.code);
    if (pending.error) {
      setError(pending.error);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onSave({
        title: title.trim(),
        why: why.trim(),
        horizon: place,
        area: area.trim() || null,
        accept: linesToList(accept),
        deps: pending.deps ?? deps,
        workflow_def_id: workflow === PROJECT_DEFAULT ? null : workflow,
      });
      onClose();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!onDelete || busy) return;
    setBusy(true);
    setError(null);
    try {
      await onDelete();
      onClose();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <Modal
      icon={item ? "edit" : "plus"}
      title={item ? `Edit ${item.code}` : "New roadmap item"}
      size="lg"
      className="rm-dialog"
      onClose={onClose}
    >
      <ModalBody>
        <div className="modal-field">
          <label className="modal-label text-sm" htmlFor="rm-title">
            Title
          </label>
          <TextInput
            id="rm-title"
            autoFocus
            placeholder="Persist worktree state across app restarts"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            onKeyDown={(e) => {
              // Enter commits from the one-line field; the prose boxes below
              // need their newlines.
              if (e.key === "Enter") void save();
            }}
          />
        </div>

        <div className="modal-field">
          <span className="modal-label text-sm">Horizon</span>
          <Segmented value={place} options={HORIZON_OPTIONS} onChange={setPlace} />
        </div>

        {/* Dependencies as chips: a code is a thing you pick, not prose, and the
            row already draws them this way ("after FLT-100"). The queue skips an
            item whose deps aren't done, so this field is dispatch order the user
            can state — the loop check is the backend's (see roadmap::deps). */}
        <div className="modal-field">
          <label className="modal-label text-sm" htmlFor="rm-deps">
            After <span className="modal-opt">codes this must land after, optional</span>
          </label>
          <div className="rm-deps flex-center">
            {deps.map((d) => (
              <span key={d} className="rm-dep-chip iflex-center mono text-xs">
                {d}
                <button
                  type="button"
                  className="rm-dep-x iflex-center"
                  aria-label={`Remove ${d}`}
                  onClick={() => {
                    setDeps(removeDep(deps, d));
                    setError(null);
                  }}
                >
                  <Icon name="close" size={9} />
                </button>
              </span>
            ))}
            <input
              id="rm-deps"
              className="rm-deps-in mono text-xs"
              list="rm-deps-codes"
              autoComplete="off"
              placeholder={deps.length ? "" : (depsAvailable[0] ?? "nothing else on the board yet")}
              disabled={depsAvailable.length === 0 && deps.length === 0}
              value={depDraft}
              onChange={(e) => setDepDraft(e.target.value)}
              onBlur={commitDep}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === ",") {
                  // The chip, not the form: Enter here means "that's a dep".
                  e.preventDefault();
                  commitDep();
                } else if (e.key === "Backspace" && !depDraft && deps.length) {
                  setDeps(deps.slice(0, -1));
                }
              }}
            />
            {/* Native suggestions rather than a popup: the answer is a short
                list of codes, and the datalist filters as the user types. */}
            <datalist id="rm-deps-codes">
              {depOptions.map((c) => (
                <option key={c} value={c} />
              ))}
            </datalist>
          </div>
        </div>

        <div className="modal-field">
          <label className="modal-label text-sm" htmlFor="rm-area">
            Area <span className="modal-opt">optional</span>
          </label>
          <TextInput
            id="rm-area"
            placeholder="runtime"
            value={area}
            onChange={(e) => setArea(e.target.value)}
          />
        </div>

        <div className="modal-field">
          <span className="modal-label text-sm">
            Build it with <span className="modal-opt">when queued</span>
          </span>
          <Select
            value={workflow}
            options={workflowOptions}
            onChange={setWorkflow}
            ariaLabel="Workflow"
          />
        </div>

        <div className="modal-field">
          <label className="modal-label text-sm" htmlFor="rm-why">
            Why <span className="modal-opt">optional</span>
          </label>
          <TextArea
            id="rm-why"
            className="rm-textarea"
            placeholder="The one line that justifies its place on the board."
            value={why}
            onChange={(e) => setWhy(e.target.value)}
          />
        </div>

        <div className="modal-field">
          <label className="modal-label text-sm" htmlFor="rm-accept">
            Done when <span className="modal-opt">one per line, optional</span>
          </label>
          <TextArea
            id="rm-accept"
            className="rm-textarea"
            placeholder={"Worktree registry survives a hard quit\nOrphans are offered for cleanup"}
            value={accept}
            onChange={(e) => setAccept(e.target.value)}
          />
        </div>

        {error && <div className="modal-error text-sm">{error}</div>}
      </ModalBody>

      <ModalFooter>
        {onDelete && (
          <Button
            variant="ghost"
            danger
            disabled={busy}
            onClick={() => (confirmDelete ? void remove() : setConfirmDelete(true))}
          >
            <Icon name="trash" size={12} />
            {confirmDelete ? "Delete for good?" : "Delete"}
          </Button>
        )}
        <span className="grow" />
        <Button variant="ghost" onClick={onClose} disabled={busy}>
          Cancel
        </Button>
        <Button variant="primary" onClick={() => void save()} disabled={!canSave}>
          {item ? "Save" : "Add to roadmap"}
        </Button>
      </ModalFooter>
    </Modal>
  );
}
