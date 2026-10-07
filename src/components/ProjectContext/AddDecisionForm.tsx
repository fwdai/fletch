import { useState } from "react";
import { type AssertionKind, api, type ContextEntity, type Domain, type Stance } from "@/api";
import { Button } from "@/components/ui/Button";
import { TextArea, TextInput } from "@/components/ui/TextInput";
import { ASSERTION_KINDS, DOMAINS, matchesSearch } from "./format";

/** Record a decision, constraint or fact about one or more entities. Lands
 *  confirmed: the user saying it is the confirmation. */
export function AddDecisionForm({
  projectId,
  entities,
  initialAbout,
  onDone,
}: {
  projectId: string;
  entities: ContextEntity[];
  initialAbout: string[];
  onDone: () => void;
}) {
  const [kind, setKind] = useState<AssertionKind>("decision");
  const [domain, setDomain] = useState<Domain>("architectural");
  const [stance, setStance] = useState<Stance>("adopted");
  const [statement, setStatement] = useState("");
  const [rationale, setRationale] = useState("");
  const [about, setAbout] = useState<string[]>(initialAbout);
  const [search, setSearch] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ready = statement.trim() && about.length > 0 && !busy;

  const toggleAbout = (id: string) =>
    setAbout((prev) => (prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]));

  const submit = () => {
    setBusy(true);
    setError(null);
    api
      .contextRecordAssertion(projectId, {
        kind,
        domain,
        stance,
        statement: statement.trim(),
        rationale: rationale.trim(),
        paths: [],
        about,
        contradicts: [],
        status: "confirmed",
      })
      .then(onDone)
      .catch((e) => {
        setError(String(e));
        setBusy(false);
      });
  };

  return (
    <div className="pc-form">
      <h3 className="text-base">New decision</h3>
      <div className="pc-form-row">
        <div className="ps-field">
          <label className="ps-label text-sm" htmlFor="pc-dec-kind">
            Kind
          </label>
          <select
            id="pc-dec-kind"
            className="ps-input text-sm"
            value={kind}
            onChange={(e) => setKind(e.target.value as AssertionKind)}
          >
            {ASSERTION_KINDS.map((k) => (
              <option key={k} value={k}>
                {k}
              </option>
            ))}
          </select>
        </div>
        <div className="ps-field">
          <label className="ps-label text-sm" htmlFor="pc-dec-domain">
            Domain
          </label>
          <select
            id="pc-dec-domain"
            className="ps-input text-sm"
            value={domain}
            onChange={(e) => setDomain(e.target.value as Domain)}
          >
            {DOMAINS.map((d) => (
              <option key={d} value={d}>
                {d}
              </option>
            ))}
          </select>
        </div>
        <div className="ps-field">
          <label className="ps-label text-sm" htmlFor="pc-dec-stance">
            Stance
          </label>
          <select
            id="pc-dec-stance"
            className="ps-input text-sm"
            value={stance}
            onChange={(e) => setStance(e.target.value as Stance)}
          >
            <option value="adopted">adopted</option>
            <option value="rejected">rejected</option>
          </select>
        </div>
      </div>
      <div className="ps-field">
        <label className="ps-label text-sm" htmlFor="pc-dec-statement">
          Statement
        </label>
        <TextArea
          id="pc-dec-statement"
          value={statement}
          placeholder="What was decided, in one sentence"
          onChange={(e) => setStatement(e.target.value)}
        />
      </div>
      <div className="ps-field">
        <label className="ps-label text-sm" htmlFor="pc-dec-rationale">
          Rationale
        </label>
        <TextArea
          id="pc-dec-rationale"
          value={rationale}
          onChange={(e) => setRationale(e.target.value)}
        />
      </div>
      <div className="ps-field">
        <span className="ps-label text-sm">About (at least one)</span>
        <TextInput
          placeholder="Filter entities…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          aria-label="Filter entities"
        />
        <div className="pc-picks text-sm">
          {entities
            .filter((e) => matchesSearch(e, search))
            .map((e) => (
              <label key={e.id}>
                <input
                  type="checkbox"
                  checked={about.includes(e.id)}
                  onChange={() => toggleAbout(e.id)}
                />
                {e.name} <span className="pc-meta text-xs">{e.slug}</span>
              </label>
            ))}
        </div>
      </div>
      {error && <div className="pc-error text-sm">{error}</div>}
      <div className="pc-form-actions">
        <Button variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button variant="primary" disabled={!ready} onClick={submit}>
          Record
        </Button>
      </div>
    </div>
  );
}
