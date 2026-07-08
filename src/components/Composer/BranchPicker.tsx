import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "@/api";
import { Icon } from "@/components/Icon";
import { DropdownItem, DropdownMenu, DropdownSection } from "@/components/ui/Dropdown";
import { Scrim } from "@/components/ui/Scrim";

const ITEM_HEIGHT = 34; // approximate px per dd-item row
const VISIBLE_ITEMS = 8;
const MAX_HEIGHT = ITEM_HEIGHT * VISIBLE_ITEMS;
const SCROLL_STEP = ITEM_HEIGHT * 3;

interface Props {
  repoPath: string;
  value: string;
  onChange: (branch: string) => void;
}

export function BranchPicker({ repoPath, value, onChange }: Props) {
  const [open, setOpen] = useState(false);
  const [branches, setBranches] = useState<string[]>([]);
  const [canScrollUp, setCanScrollUp] = useState(false);
  const [canScrollDown, setCanScrollDown] = useState(false);
  const listRef = useRef<HTMLDivElement>(null);

  // Stable (reads only the ref + setters) so it can be an honest effect dep
  // without re-running the fetch every render.
  const updateScrollState = useCallback(() => {
    const el = listRef.current;
    if (!el) return;
    setCanScrollUp(el.scrollTop > 0);
    setCanScrollDown(el.scrollTop + el.clientHeight < el.scrollHeight - 1);
  }, []);

  useEffect(() => {
    if (!open) return;
    api
      .listRepoBranches(repoPath)
      .then((bs) => {
        setBranches(bs);
        // after render, check if the selected branch needs scroll and update indicators
        requestAnimationFrame(() => updateScrollState());
      })
      .catch(() => setBranches([]));
  }, [open, repoPath, updateScrollState]);

  function scrollBy(delta: number) {
    listRef.current?.scrollBy({ top: delta, behavior: "smooth" });
  }

  return (
    <span style={{ position: "relative", minWidth: 0 }}>
      <span className="pill is-action" title={value} onClick={() => setOpen((v) => !v)}>
        <Icon name="branch" />
        <span style={{ color: "var(--fg-2)" }}>from</span>
        <span
          className="v"
          style={{
            maxWidth: 140,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {value}
        </span>
        <Icon name="chevD" size={9} />
      </span>

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <DropdownMenu
            style={{ bottom: "calc(100% + 6px)", left: 0, padding: 0, overflow: "hidden" }}
          >
            <DropdownSection>Local branches</DropdownSection>

            {canScrollUp && (
              <button
                type="button"
                className="bp-scroll-btn flex-center"
                onClick={() => scrollBy(-SCROLL_STEP)}
              >
                <Icon name="chevU" size={11} />
              </button>
            )}

            <div
              ref={listRef}
              style={{ maxHeight: MAX_HEIGHT, overflowY: "auto" }}
              onScroll={updateScrollState}
            >
              {branches.length === 0 ? (
                <DropdownItem disabled style={{ padding: "7px 9px" }}>
                  Loading…
                </DropdownItem>
              ) : (
                branches.map((b) => (
                  <DropdownItem
                    key={b}
                    active={b === value}
                    style={{ padding: "7px 9px" }}
                    onClick={() => {
                      onChange(b);
                      setOpen(false);
                    }}
                  >
                    <Icon name="branch" size={14} />
                    <span className="di-l" style={{ fontFamily: "var(--font-mono)" }}>
                      {b}
                    </span>
                  </DropdownItem>
                ))
              )}
            </div>

            {canScrollDown && (
              <button
                type="button"
                className="bp-scroll-btn flex-center"
                onClick={() => scrollBy(SCROLL_STEP)}
              >
                <Icon name="chevD" size={11} />
              </button>
            )}
          </DropdownMenu>
        </>
      )}
    </span>
  );
}
