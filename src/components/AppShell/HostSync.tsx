import { useFleetGitSync } from "@/store/gitSync";

/** What one host keeps polled whether or not it is on screen: its fleet's
 *  sidebar numbers and PR seed. Rendered once per host, in its scope. */
export function HostSync() {
  useFleetGitSync();
  return null;
}
