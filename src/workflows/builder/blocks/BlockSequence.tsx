// BlockSequence.tsx — renders a sequence of blocks left-to-right with connectors
// and an "add block" menu. Used at the top level and (recursively) for loop
// bodies, so it takes its owning sequence's nid (`null` = top level).

import { Fragment, useState } from "react";
import { Icon } from "../../../components/Icon";
import { BLOCK_TYPES } from "../../data";
import type { BuilderCtx } from "../ctx";
import { useDismissOnViewportChange } from "../hooks";
import type { EBlock, NodeId } from "../model";
import { LoopContainer } from "./LoopContainer";
import { OrchestrateContainer } from "./OrchestrateContainer";
import { ParallelContainer } from "./ParallelContainer";
import { StepCard } from "./StepCard";

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

function BlockNode({
  block,
  index,
  ctx,
  canRemove,
}: {
  block: EBlock;
  index: number;
  ctx: BuilderCtx;
  canRemove: boolean;
}) {
  if (block.kind === "step") {
    return <StepCard step={block} ctx={ctx} indexLabel={pad2(index + 1)} canRemove={canRemove} />;
  }
  if (block.kind === "parallel") return <ParallelContainer block={block} ctx={ctx} />;
  if (block.kind === "loop") return <LoopContainer block={block} ctx={ctx} />;
  return <OrchestrateContainer block={block} ctx={ctx} />;
}

function Connector() {
  return (
    <div className="wb-conn">
      <Icon name="arrowR" />
    </div>
  );
}

export function BlockSequence({
  blocks,
  seqNid,
  ctx,
}: {
  blocks: EBlock[];
  seqNid: NodeId | null;
  ctx: BuilderCtx;
}) {
  // The menu is fixed-positioned from the button's rect so it escapes the
  // canvas's overflow clip (same trick as the builder's other popovers).
  const [menu, setMenu] = useState<{ top: number; left: number } | null>(null);

  useDismissOnViewportChange(!!menu, () => setMenu(null));
  const canRemove = blocks.length > 1 || seqNid !== null;
  // A non-null seqNid means this is a loop body, and the engine executes loop
  // bodies of plain steps only (spec §6.6) — don't offer containers there.
  const blockTypes = seqNid === null ? BLOCK_TYPES : BLOCK_TYPES.filter((t) => t.id === "step");

  return (
    <div className="wb-seq">
      {blocks.map((b, i) => (
        <Fragment key={b.nid}>
          {i > 0 && <Connector />}
          <BlockNode block={b} index={i} ctx={ctx} canRemove={canRemove} />
        </Fragment>
      ))}

      {blocks.length > 0 && <Connector />}

      <div className="wb-addwrap">
        <button
          className="wb-add"
          onClick={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            setMenu((m) => (m ? null : { top: r.bottom, left: r.left }));
          }}
        >
          <span className="wb-add-ic">
            <Icon name="plus" />
          </span>
          <span className="wb-add-l">Add block</span>
        </button>
        {menu && (
          <>
            <div
              style={{ position: "fixed", inset: 0, zIndex: 55 }}
              onClick={() => setMenu(null)}
            />
            <div
              className="dd wb-add-menu"
              style={{ position: "fixed", top: menu.top, left: menu.left }}
            >
              {blockTypes.map((t) => (
                <div
                  key={t.id}
                  className="dd-item"
                  onClick={() => {
                    ctx.addBlock(seqNid, t.id);
                    setMenu(null);
                  }}
                  style={{ alignItems: "flex-start", flexDirection: "column", gap: 2 }}
                >
                  <span style={{ display: "flex", alignItems: "center", gap: 8 }}>
                    <Icon name={t.icon} size={12} />{" "}
                    <span style={{ fontWeight: 500 }}>{t.label}</span>
                  </span>
                  <span
                    style={{ fontSize: 11, color: "var(--fg-2)", lineHeight: 1.4, paddingLeft: 20 }}
                  >
                    {t.note}
                  </span>
                </div>
              ))}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
