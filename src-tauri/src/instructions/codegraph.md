## Codegraph: query the index instead of grepping

This workspace has **codegraph** attached over MCP — a pre-built knowledge graph of
the repo's symbols, files, and edges. One `codegraph_explore` call (a
natural-language question, or just a bag of symbol/file names) returns the
verbatim, line-numbered source of the matching symbols grouped by file, plus their
callers and the blast radius of changing them. Its namespace differs per CLI
(`mcp__codegraph__codegraph_explore` on Claude, `codegraph.codegraph_explore` on
Codex) — it is whichever tool name ends in `codegraph_explore`.

- Prefer it over a grep + read loop, both for questions ("how does X work", "where
  is Y") and **before you edit** — one call gives you the code and everything that
  depends on it. The output is line-numbered source, safe to edit from.
- Fall back to grep/read when codegraph comes back empty; the index trails a
  just-written file by about a second.
- The "Explore budget: N calls" note in a result is advisory pacing, not a quota:
  nothing meters these calls, and N+1 works like the first. Read it as "N usually
  suffices", not "stop at N" — while the question is open, explore again rather
  than fall back to grep + read.
- **When you delegate code-location work, dispatch to the `codegraph` subagent
  type** rather than a general search agent — it is defined for exactly this and
  starts from the index. For any other subagent, say it in the task prompt
  yourself: subagents do not inherit these instructions and their definitions
  steer toward Grep/Glob, so add "use codegraph_explore rather than grep/read to
  locate code."
