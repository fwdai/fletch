// The task Fletch suggests for a user's very first agent. Seeded into the
// composer of the draft that opens right after the first project is added, so
// activation is one keypress away instead of a blank box. Safe on any repo:
// needs no GitHub, changes nothing, and shows what an agent is for in a minute.
// Kept here, not in the store, so a later "suggested tasks" surface (Home, the
// draft screen) can offer the same prompt.

export const FIRST_TASK_PROMPT =
  "Explain the structure of this repository: the main modules, how it is built and run, and where to start reading.";
