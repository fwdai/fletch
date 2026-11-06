import { api, type EgressDelta } from "@/api";

/** A list of hosts the Network rows show and add to: the user's own, for
 *  every project (`globalHosts`), or one project's, which its page builds from
 *  the project's settings files and the hosts added in the app. */
export interface HostList {
  /** Set for a project's list. */
  projectId?: string;
  /** Each host, where it lives, and how to take it off when that can be done here. */
  hosts: { pattern: string; where: string; remove?: () => Promise<unknown> }[];
  /** Puts a host on the list; absent when it can't be edited from here. */
  allow?: (host: string) => Promise<unknown>;
}

/** The user's global list, written whole on every change. */
export function globalHosts(global: EgressDelta): HostList {
  const save = (add: string[]) => api.setBoxEgressGlobal({ add, remove: [] });
  return {
    hosts: global.add.map((pattern) => ({
      pattern,
      where: "All projects",
      remove: () => save(global.add.filter((p) => p !== pattern)),
    })),
    allow: (host) => save([...global.add, host]),
  };
}
