// What the "Paired hosts" field accepts, and what it says when it doesn't.
//
// Its own module because it is the pane's only logic and the only part worth a
// test: everything else in the section is rows and a button.

import { parsePairUrl } from "@/remote/pairing";
import type { PairTarget } from "@/remote/registry";

/** Turn pasted text into something pairable, or say why it isn't.
 *
 *  Three refusals, each with the step that produces the missing piece:
 *  anything that is not a `fletch://pair?…` link at all (or one with no
 *  dialable `addr=`, which `parsePairUrl` refuses for the same reason); a link
 *  with no `host=`, whose key is the environment id here and so cannot be
 *  pinned on first contact the way the phone does; and a link with no `token=`,
 *  which is a link to *reach* a host rather than to pair with one. */
export function pairTargetFrom(input: string): { target: PairTarget } | { error: string } {
  const target = parsePairUrl(input);
  if (!target) {
    return {
      error:
        "That isn't a Fletch pairing link. On the other machine, open Settings › Remote control › Pair a device and copy the link.",
    };
  }
  if (!target.hostKey) {
    return { error: "That pairing link carries no host key. Generate a new one on the host." };
  }
  if (!target.pairingToken) {
    return { error: "That link has no pairing code. Generate a new one on the host." };
  }
  return { target: { ...target, hostKey: target.hostKey, pairingToken: target.pairingToken } };
}
