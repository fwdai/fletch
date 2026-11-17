import { createKeyedQueue } from "@/util/keyedQueue";
import { perHost } from "./perHost";

/** Per-agent order of the messages this window sends to one host. A send takes
 *  its place in its agent's line the moment it is made (the user's Enter, a
 *  draft's first prompt), and everything it waits on before reaching the host
 *  happens inside that place: its folders resolving and being granted, a config
 *  change in flight, command discovery, the agent coming up. No later send can
 *  reach the host ahead of it, however long an earlier one's steps take. A
 *  place is given up once the host has taken the message, or the send has
 *  failed. */
export const sendOrder = perHost(createKeyedQueue);

/** Each host's sends still on the wire from this window (see `whileSending` in
 *  helpers/sending): what a workspace snapshot of that host must not settle. */
export const sendsInFlight = perHost(() => new Set<string>());
