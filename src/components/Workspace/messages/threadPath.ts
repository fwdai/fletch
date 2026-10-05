// Which sub-agent thread the rows being rendered belong to: the chain of
// launching tool_use ids above them (see `openThread` in store/ui). The main
// conversation renders at the default (empty) path; the thread view provides
// its own, so a sub-agent card inside a thread opens the thread *below* it.

import { createContext, useContext } from "react";

const ThreadPathContext = createContext<readonly string[]>([]);

export const ThreadPathProvider = ThreadPathContext.Provider;

export function useThreadPath(): readonly string[] {
  return useContext(ThreadPathContext);
}
