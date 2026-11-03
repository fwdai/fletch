import type { FolderGrant } from "@/api";
import { AttachmentList } from "@/components/Composer/AttachmentList";
import { FolderList } from "@/components/Composer/FolderList";
import { agentRecord } from "@/helpers/agentLookups";
import { useAppStore } from "@/store";

/** A sent message's attachments. A folder the workspace still has granted
 *  shows as a live folder chip, where its access is changed: the mode flips
 *  and × takes it back, for the workspace rather than this message alone (a
 *  grant outlives the message that made it). Files, and folders no longer
 *  granted, are plain chips. Without an agent every path is a plain chip. */
export function SentAttachments({ agentId, paths }: { agentId?: string; paths: string[] }) {
  const grants = useAppStore((s) => (agentId ? agentRecord(s, agentId)?.folder_grants : undefined));
  const grantFolder = useAppStore((s) => s.grantAgentFolder);
  const revokeFolder = useAppStore((s) => s.revokeAgentFolder);
  const folders: FolderGrant[] = [];
  const files: string[] = [];
  for (const path of paths) {
    const grant = grants?.find((g) => g.path === path);
    if (grant) folders.push(grant);
    else files.push(path);
  }
  return (
    <>
      {agentId && folders.length > 0 && (
        <FolderList
          granted
          className="message-attachments"
          folders={folders}
          onToggle={(f) => void grantFolder(agentId, f.path, !f.writable)}
          onRemove={(path) => void revokeFolder(agentId, path)}
        />
      )}
      {files.length > 0 && <AttachmentList paths={files} className="message-attachments" />}
    </>
  );
}
