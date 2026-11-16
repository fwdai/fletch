import { useMemo, useState } from "react";
import { type IssueSource, issueDisplayKey, type PrSummary, type TrackerIssue } from "@/api";
import { Icon } from "@/components/Icon";
import {
  FlyoutFoot,
  FlyoutGlyph,
  FlyoutMenu,
  FlyoutRow,
  FlyoutSection,
  SideFlyout,
} from "@/components/ui/FlyoutMenu";
import { IconButton } from "@/components/ui/IconButton";
import { useCachedList } from "../useCachedList";
import { type RefEntry, RefList } from "./RefList";

/** The row whose side panel is showing. */
type Fly = "issues" | "prs" | "skills";

const SOURCE_LABEL: Record<IssueSource, string> = { github: "GitHub", linear: "Linear" };

/** A skill the `/` menu offers, under the token that invokes it. */
export interface SkillChoice {
  command: string;
  description: string;
}

interface Props {
  /** Open the file picker and attach what it returns. */
  onAttachFiles: () => void;
  /** Open the folder picker. Omit where a folder can't be given to the agent. */
  onAttachFolders?: () => void;
  /** Open tracker issues (GitHub, Linear) to work on. Omit to hide the row. */
  issues?: { list: () => Promise<TrackerIssue[]>; onPick: (issue: TrackerIssue) => void };
  /** The repo's open pull requests to reference. Omit to hide the row. */
  prs?: { list: () => Promise<PrSummary[]>; onPick: (pr: PrSummary) => void };
  /** Library skills to invoke. Omit to hide the row. */
  skills?: { choices: SkillChoice[]; onPick: (command: string) => void };
  /** The workspace's VM network for this turn: whether it is open, and the
   *  switch. Omit where there is nothing to open. */
  network?: { open: boolean; onToggle: (open: boolean) => void };
  /** Typing `@` completes a file here, so the card's hint may say so. */
  fileMentions?: boolean;
  /** The menu closed, with or without a pick: the composer takes the focus
   *  back from the filter. */
  onClose?: () => void;
}

/** The composer's one "add to the prompt" control: a `+` that opens every way
 *  of putting something into this message besides typing it. Files and folders
 *  attach; an issue, a pull request or a skill is picked from a list beside
 *  the card (what `#` and `/` complete inline, for a user who hasn't met
 *  them); and the turn's network opens from here.
 *
 *  It sits at the right of the footer, so the card lines its right edge up
 *  with the button and its lists open over whatever is to the right of the
 *  conversation. */
export function AddMenu({
  onAttachFiles,
  onAttachFolders,
  issues,
  prs,
  skills,
  network,
  fileMentions,
  onClose,
}: Props) {
  const [open, setOpen] = useState(false);
  const [fly, setFly] = useState<Fly | null>(null);

  // Fetched as its panel first shows, then kept briefly: the lists are remote
  // (`gh`, Linear) and a pointer crossing the rows must not refetch them.
  const issueList = useCachedList(issues?.list, fly === "issues");
  const prList = useCachedList(prs?.list, fly === "prs");

  const issueEntries = useMemo<RefEntry[] | null>(
    () =>
      issueList &&
      // By tracker, so each heading shows once; the sort is stable, so each
      // tracker keeps the order it answered in.
      [...issueList]
        .sort((a, b) => a.source.localeCompare(b.source))
        .map((issue) => ({
          id: issueId(issue),
          tag: issueDisplayKey(issue),
          title: issue.title,
          group: SOURCE_LABEL[issue.source],
        })),
    [issueList],
  );
  const prEntries = useMemo<RefEntry[] | null>(
    () =>
      prList &&
      [...prList]
        .sort((a, b) => b.number - a.number)
        .map((pr) => ({ id: String(pr.number), tag: `#${pr.number}`, title: pr.title })),
    [prList],
  );
  const skillEntries = useMemo<RefEntry[]>(
    () =>
      (skills?.choices ?? []).map((s) => ({
        id: s.command,
        tag: `/${s.command}`,
        title: s.description || "Skill",
      })),
    [skills?.choices],
  );

  function close() {
    setOpen(false);
    setFly(null);
    onClose?.();
  }

  /** A row or a list entry was chosen: the menu goes, then the choice acts. */
  function choose(act: () => void) {
    close();
    act();
  }

  const flyout =
    fly === "issues" && issues ? (
      <SideFlyout
        flyKey="issues"
        icon={<FlyoutGlyph name="issue" head />}
        title="Open issues"
        tag="issue"
        fill
      >
        <RefList
          entries={issueEntries}
          placeholder="Filter issues"
          empty="No open issues"
          onPick={(id) => {
            const issue = issueList?.find((i) => issueId(i) === id);
            if (issue) choose(() => issues.onPick(issue));
          }}
        />
      </SideFlyout>
    ) : fly === "prs" && prs ? (
      <SideFlyout
        flyKey="prs"
        icon={<FlyoutGlyph name="pr" head />}
        title="Pull requests"
        tag="pr"
        fill
      >
        <RefList
          entries={prEntries}
          placeholder="Filter pull requests"
          empty="No open pull requests"
          onPick={(id) => {
            const pr = prList?.find((p) => String(p.number) === id);
            if (pr) choose(() => prs.onPick(pr));
          }}
        />
      </SideFlyout>
    ) : fly === "skills" && skills ? (
      <SideFlyout
        flyKey="skills"
        icon={<FlyoutGlyph name="notebookPen" head />}
        title="Skills"
        tag="skill"
        fill
      >
        <RefList
          entries={skillEntries}
          placeholder="Filter skills"
          empty="No skills in the library"
          onPick={(command) => choose(() => skills.onPick(command))}
        />
      </SideFlyout>
    ) : null;

  return (
    <span className="add-menu">
      <IconButton
        className="composer-action"
        tip={open ? undefined : "Add to the prompt"}
        aria-label="Add to the prompt"
        aria-haspopup="menu"
        aria-expanded={open}
        active={open}
        onClick={() => (open ? close() : setOpen(true))}
      >
        <Icon name="plus" size={15} />
      </IconButton>

      {open && (
        <FlyoutMenu
          align="end"
          sideWidth={300}
          // One height for the card and its lists, whichever rows this
          // composer has: the card is a handful of rows, a list is not.
          minHeight={300}
          onClose={close}
          flyout={flyout}
        >
          <FlyoutSection>Add to the prompt</FlyoutSection>
          <FlyoutRow
            icon={<FlyoutGlyph name="attach" />}
            label="Attach files"
            onMouseEnter={() => setFly(null)}
            onClick={() => choose(onAttachFiles)}
          />
          {onAttachFolders && (
            <FlyoutRow
              icon={<FlyoutGlyph name="folder" />}
              label="Attach a folder"
              title="Give the agent a folder on this Mac, read-only until you say otherwise"
              onMouseEnter={() => setFly(null)}
              onClick={() => choose(onAttachFolders)}
            />
          )}
          {issues && (
            <FlyoutRow
              icon={<FlyoutGlyph name="issue" />}
              label="Work on an issue"
              aria-haspopup="menu"
              aria-expanded={fly === "issues"}
              hot={fly === "issues"}
              chevron
              onMouseEnter={() => setFly("issues")}
              onClick={() => setFly("issues")}
            />
          )}
          {prs && (
            <FlyoutRow
              icon={<FlyoutGlyph name="pr" />}
              label="Reference a pull request"
              aria-haspopup="menu"
              aria-expanded={fly === "prs"}
              hot={fly === "prs"}
              chevron
              onMouseEnter={() => setFly("prs")}
              onClick={() => setFly("prs")}
            />
          )}
          {skills && (
            <FlyoutRow
              icon={<FlyoutGlyph name="notebookPen" />}
              label="Use a skill"
              aria-haspopup="menu"
              aria-expanded={fly === "skills"}
              hot={fly === "skills"}
              chevron
              onMouseEnter={() => setFly("skills")}
              onClick={() => setFly("skills")}
            />
          )}
          {network && (
            <>
              <FlyoutSection>This turn</FlyoutSection>
              <FlyoutRow
                icon={<FlyoutGlyph name="globe" />}
                label={network.open ? "Close the network" : "Open the network"}
                note={network.open ? "open" : undefined}
                title={
                  network.open
                    ? "The network is open until this turn ends"
                    : "Let the agent reach any host until this turn ends"
                }
                active={network.open}
                onMouseEnter={() => setFly(null)}
                onClick={() => choose(() => network.onToggle(!network.open))}
              />
            </>
          )}
          <FlyoutFoot>
            <span>Or type</span>
            {fileMentions && (
              <span>
                <kbd className="add-key">@</kbd> file
              </span>
            )}
            {prs && (
              <span>
                <kbd className="add-key">#</kbd> pull request
              </span>
            )}
            <span>
              <kbd className="add-key">/</kbd> command
            </span>
          </FlyoutFoot>
        </FlyoutMenu>
      )}
    </span>
  );
}

function issueId(issue: TrackerIssue): string {
  return `${issue.source}:${issue.key}`;
}
