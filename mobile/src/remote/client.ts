// The protocol client: envelope encoding, request/response matching, the
// pair/hello handshake, event fan-out and reconnect with backoff. Transport
// agnostic — it drives whatever `SocketFactory` it is handed, which is how the
// mock host and the real WebSocket share every line of this.

import { backoffDelay } from "./backoff";
import { type Candidate, candidatesFor } from "./candidates";
import type { Socket, SocketFactory, SocketHandlers } from "./socket";
import {
  CLOSE_REASONS,
  CLOSE_REMOTE_DISABLED,
  CLOSE_UNAUTHENTICATED,
  type ConnectionState,
  type DeviceInfo,
  type EventFrame,
  type EventHandler,
  type HelloResult,
  HOST_KEY_MISMATCH,
  HOST_KEY_MISMATCH_REASON,
  type HostFrame,
  type HostInfo,
  type HostTarget,
  isEventFrame,
  type PairResult,
  type PairStep,
  type RemoteClient,
  type RequestFrame,
  type StateHandler,
  type StepHandler,
  type Via,
} from "./types";

/** Close codes that mean "do not retry" — the device has to be paired again,
 *  or the host's remote-access switch turned back on. */
const FATAL_CLOSE = new Set([CLOSE_UNAUTHENTICATED, CLOSE_REMOTE_DISABLED]);

/** Bound on the first frames after the socket opens: `pair` or `hello`, and
 *  the snapshot that follows a `pair`. The transport's open budget stops once
 *  the Noise handshake is done, and a relay that has accepted the socket for a
 *  Mac that has gone quiet forwards the request into nothing — so without a
 *  bound of its own the attempt would sit in `pairing` for ever. */
export const HANDSHAKE_TIMEOUT_MS = 15_000;
const HANDSHAKE_TIMED_OUT = "The Mac did not answer. Check that it is awake and connected.";

interface Pending {
  resolve: (v: unknown) => void;
  reject: (e: Error) => void;
}

export interface ClientOptions {
  openSocket: SocketFactory;
  device: DeviceInfo;
  newId?: () => string;
  /** Injected in tests so backoff can be driven without real time. */
  setTimer?: (fn: () => void, ms: number) => unknown;
  clearTimer?: (handle: unknown) => void;
  baseDelay?: number;
  maxDelay?: number;
  /** Open timeout for the LAN candidate, in ms (see `LAN_OPEN_TIMEOUT_MS`).
   *  Lowered in tests so a hanging dial does not cost three seconds. */
  openTimeout?: number;
}

/** The transport's mismatch marker carries the two keys, which is right for a
 *  log and wrong for a user. */
const reportable = (message: string) =>
  message.includes(HOST_KEY_MISMATCH) ? HOST_KEY_MISMATCH_REASON : message;

const randomId = () =>
  globalThis.crypto?.randomUUID?.() ??
  `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;

export class ProtocolClient implements RemoteClient {
  private socket: Socket | null = null;
  private pending = new Map<string, Pending>();
  private listeners = new Map<string, Set<EventHandler>>();
  private stateListeners = new Set<StateHandler>();
  private stepListeners = new Set<StepHandler>();
  private snapshotListeners = new Set<(r: HelloResult) => void>();
  private _target: HostTarget | null = null;
  /** Bumped for every attempt and every teardown. A socket's callbacks carry
   *  the generation they were opened under and are ignored once it is stale,
   *  so a close arriving late from a replaced socket cannot touch the live
   *  one. */
  private gen = 0;
  private attempt = 0;
  private retryHandle: unknown = null;
  private closedByUs = false;
  private _state: ConnectionState = "disconnected";
  private _host: HostInfo | null = null;
  private _via: Via | null = null;
  private readonly newId: () => string;
  private readonly setTimer: (fn: () => void, ms: number) => unknown;
  private readonly clearTimer: (handle: unknown) => void;

  constructor(private readonly opts: ClientOptions) {
    this.newId = opts.newId ?? randomId;
    this.setTimer = opts.setTimer ?? ((fn, ms) => setTimeout(fn, ms));
    this.clearTimer = opts.clearTimer ?? ((h) => clearTimeout(h as ReturnType<typeof setTimeout>));
  }

  get state(): ConnectionState {
    return this._state;
  }

  get retrying(): boolean {
    return this.retryHandle !== null;
  }

  get host(): HostInfo | null {
    return this._host;
  }

  /** The host identity this client is bound to — supplied by the pairing link
   *  or pinned on first contact. */
  get hostKey(): string | null {
    return this._target?.hostKey ?? null;
  }

  /** Which candidate the live socket came from. Reported for the Host sheet;
   *  nothing in the protocol depends on it. */
  get via(): Via | null {
    return this.socket ? this._via : null;
  }

  /** Add or change the relay on the held target. Nothing reconnects: the next
   *  attempt picks up the new dial list. */
  setRelay(relay: string | null): void {
    if (!this._target) return;
    this._target = { ...this._target, relay: relay?.trim() || undefined };
  }

  /** Where this client is (or was last) pointed. The client owns it: after a
   *  pairing handshake the spent `pairingToken` is gone and the host key the
   *  transport authenticated is pinned in, so this is always what a reconnect
   *  should use. */
  get target(): Readonly<HostTarget> | null {
    return this._target;
  }

  onState(cb: StateHandler): () => void {
    this.stateListeners.add(cb);
    cb(this._state);
    return () => this.stateListeners.delete(cb);
  }

  onStep(cb: StepHandler): () => void {
    this.stepListeners.add(cb);
    return () => this.stepListeners.delete(cb);
  }

  onSnapshot(cb: (result: HelloResult) => void): () => void {
    this.snapshotListeners.add(cb);
    return () => this.snapshotListeners.delete(cb);
  }

  on(event: string, cb: EventHandler): () => void {
    const set = this.listeners.get(event) ?? new Set();
    set.add(cb);
    this.listeners.set(event, set);
    return () => {
      set.delete(cb);
    };
  }

  async connect(target: HostTarget): Promise<HelloResult> {
    this.teardown("Reconnecting");
    this._target = target;
    this.closedByUs = false;
    this.attempt = 0;
    return this.attemptConnect();
  }

  /** Reconnect to the target the client already holds — the pinned host key,
   *  never a spent pairing token. */
  async reconnect(): Promise<HelloResult> {
    if (!this._target) throw new Error("no host configured");
    return this.connect(this._target);
  }

  /** Stop for good and forget the target. A dropped link is handled by the
   *  client itself, so the only caller is unpairing — which must not leave a
   *  host a later `reconnect` could greet as if it were still paired. */
  disconnect(): void {
    this.closedByUs = true;
    this.teardown("Disconnected");
    this._target = null;
    this.setState("disconnected");
  }

  async call<T>(op: string, args: Record<string, unknown> = {}): Promise<T> {
    if (!this.socket) throw new Error("not connected");
    return this.request<T>(op, args);
  }

  async pair(token: string, device: DeviceInfo): Promise<PairResult> {
    const result = await this.request<PairResult>("pair", { token, device });
    this._host = result.host;
    return result;
  }

  async hello(client: DeviceInfo): Promise<HelloResult> {
    const result = await this.request<HelloResult>("hello", { client });
    this._host = result.host;
    return result;
  }

  // --- internals -----------------------------------------------------------

  private setState(state: ConnectionState, error?: string) {
    this._state = state;
    for (const cb of this.stateListeners) cb(state, error);
  }

  /** Announce where the attempt has got to. Not held anywhere: a step is only
   *  meaningful while the attempt that reported it is still running, and the
   *  state changes above are what say how it ended. */
  private step(step: PairStep) {
    for (const cb of this.stepListeners) cb(step);
  }

  private request<T>(op: string, args: Record<string, unknown>): Promise<T> {
    const socket = this.socket;
    if (!socket) return Promise.reject(new Error("not connected"));
    const id = this.newId();
    const frame: RequestFrame = { id, op, args };
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      void Promise.resolve(socket.send(JSON.stringify(frame))).catch((e) => {
        this.pending.delete(id);
        reject(e instanceof Error ? e : new Error(String(e)));
      });
    });
  }

  /** One connection attempt: open the socket, then run the first frame
   *  (`pair` or `hello`). Every failure inside it — every candidate rejecting,
   *  the handshake being refused — goes through `fail`, so the client is never
   *  left sitting in `connecting` with nothing scheduled. */
  private async attemptConnect(): Promise<HelloResult> {
    let target = this._target;
    if (!target) throw new Error("no host configured");
    const gen = ++this.gen;
    this.setState(target.pairingToken ? "pairing" : "connecting");
    this.step("connecting");
    try {
      const socket = await this.openFirstReachable(target, gen);
      if (!this.current(gen)) {
        // A newer attempt (or a disconnect) landed while the socket opened.
        socket.close();
        throw new Error("Connection superseded");
      }
      this.socket = socket;
      this._via = socket.via;
      // Trust on first use: a hand-typed pairing has no key to compare, so the
      // one the handshake authenticated becomes the pinned identity. A target
      // that did have a key never gets here with a different one — the
      // transport refuses the connection.
      if (!target.hostKey) {
        target = { ...target, hostKey: socket.hostKey };
        this._target = target;
      }
      return await this.withTimeout(
        this.handshake(target),
        HANDSHAKE_TIMEOUT_MS,
        HANDSHAKE_TIMED_OUT,
      );
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      // A pairing code is single use and short lived, so a refused pairing
      // attempt is the user's to retry. A host presenting the wrong identity
      // is never worth retrying either: only re-pairing can clear it.
      const retryable = !this._target?.pairingToken && !message.includes(HOST_KEY_MISMATCH);
      const shown = reportable(message);
      if (this.current(gen)) {
        // A socket whose handshake did not complete is no use, and after a
        // timeout it is still open with the host yet to answer.
        const socket = this.socket;
        this.socket = null;
        socket?.close();
        this.fail(shown, retryable);
      }
      throw shown === message && e instanceof Error ? e : new Error(shown);
    }
  }

  /** `promise`, or `message` as an error once `ms` have passed. Runs on the
   *  client's injectable timer so tests can drive it. */
  private withTimeout<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const handle = this.setTimer(() => reject(new Error(message)), ms);
      promise.then(
        (value) => {
          this.clearTimer(handle);
          resolve(value);
        },
        (error: unknown) => {
          this.clearTimer(handle);
          reject(error);
        },
      );
    });
  }

  private current(gen: number): boolean {
    return gen === this.gen;
  }

  /** Try the dial list in order within this one attempt: LAN, then the relay
   *  (docs/remote-protocol.md, "Relay" → "Phone side"). The first candidate
   *  that opens *is* the connection; the rest are never dialled. A candidate
   *  that rejects — refused, unreachable, or timed out by the transport —
   *  hands over to the next, and when none is left the attempt fails through
   *  `fail` with the last error, retryable as before.
   *
   *  A host-key mismatch is the exception and stops the list at once: the
   *  host's identity is wrong, not the path. Falling through to the relay
   *  would let an impostor on the LAN quietly move the phone onto the relay
   *  (and a hostile relay push it back onto the LAN), turning an alarm the
   *  user must see into a silent path switch. */
  private async openFirstReachable(target: HostTarget, gen: number): Promise<Socket> {
    const handlers: SocketHandlers = {
      onOpen: () => {},
      onMessage: (text) => {
        if (this.current(gen)) this.onMessage(text);
      },
      onClose: (code, reason) => {
        if (this.current(gen)) this.onClose(code, reason);
      },
      onError: (message) => {
        if (this.current(gen)) this.onSocketError(message);
      },
    };
    const list: Candidate[] = candidatesFor(target, this.opts.openTimeout);
    let last = new Error("no address to dial");
    for (const candidate of list) {
      try {
        this.step(candidate.via);
        return await this.opts.openSocket(candidate.url, handlers, {
          hostKey: target.hostKey,
          timeoutMs: candidate.timeoutMs,
          via: candidate.via,
        });
      } catch (e) {
        last = e instanceof Error ? e : new Error(String(e));
        if (last.message.includes(HOST_KEY_MISMATCH)) throw last;
        // Superseded mid-list: the newer attempt owns the dialling now.
        if (!this.current(gen)) throw last;
      }
    }
    throw last;
  }

  /** The first frame on a fresh socket: `pair` when a pairing code is held,
   *  `hello` otherwise. */
  private async handshake(target: HostTarget): Promise<HelloResult> {
    if (target.pairingToken) {
      this.step("registering");
      const paired = await this.pair(target.pairingToken, this.opts.device);
      // Pairing is single use: drop the code, so a reconnect greets the host
      // with `hello` on the device key it just registered.
      this._target = { ...target, pairingToken: undefined };
      this.setState("connected");
      // `pair` answers with the host identity but no snapshot, so ask for it.
      // Still part of pairing as far as anyone watching is concerned: the
      // state above says `connected`, but there is nothing to show until this
      // answers, and over a relay it is not instant.
      this.step("workspace");
      const workspace = await this.call<HelloResult["workspace"]>("get_workspace");
      return this.publishSnapshot({ host: paired.host, workspace });
    }
    this.step("greeting");
    const result = await this.hello(this.opts.device);
    this.setState("connected");
    return this.publishSnapshot(result);
  }

  /** The single failure path: report it, and schedule a retry unless the
   *  failure is one only the user can clear. */
  private fail(message: string, retryable: boolean) {
    // Scheduled before the state is announced, so a listener reading
    // `retrying` in its callback sees the retry that goes with this error.
    if (retryable && !this.closedByUs) this.scheduleRetry();
    this.setState("error", message);
  }

  /** Every successful handshake — the first and every reconnect — hands its
   *  snapshot to subscribers, which is how the store satisfies the doc's
   *  "refetch on reconnect" requirement without polling. */
  private publishSnapshot(result: HelloResult): HelloResult {
    for (const cb of this.snapshotListeners) cb(result);
    return result;
  }

  private onMessage(text: string) {
    let frame: HostFrame;
    try {
      frame = JSON.parse(text) as HostFrame;
    } catch {
      return; // A frame we can't parse is not worth tearing the link down for.
    }
    if (isEventFrame(frame)) {
      this.dispatchEvent(frame);
      return;
    }
    const waiter = this.pending.get(frame.id);
    if (!waiter) return; // Late response to a request we already gave up on.
    this.pending.delete(frame.id);
    if (frame.ok) waiter.resolve(frame.result);
    else waiter.reject(new Error(frame.error));
  }

  private dispatchEvent(frame: EventFrame) {
    const set = this.listeners.get(frame.event);
    if (!set) return;
    for (const cb of set) cb(frame.payload);
  }

  private onSocketError(message: string) {
    if (this._state === "connected") this.setState("error", message);
  }

  private onClose(code: number, reason?: string) {
    const message = CLOSE_REASONS[code] ?? reason ?? `Connection closed (${code})`;
    this.teardown(message);
    if (this.closedByUs) return;
    this.fail(message, !FATAL_CLOSE.has(code));
  }

  /** Abandon the current attempt: its socket and any callback still to arrive
   *  from it stop counting, and everything waiting on it is rejected. */
  private teardown(reason: string) {
    this.gen += 1;
    if (this.retryHandle !== null) {
      this.clearTimer(this.retryHandle);
      this.retryHandle = null;
    }
    for (const [, waiter] of this.pending) waiter.reject(new Error(reason));
    this.pending.clear();
    const socket = this.socket;
    this.socket = null;
    socket?.close();
  }

  private scheduleRetry() {
    if (this.retryHandle !== null) return;
    const delay = backoffDelay(this.attempt, this.opts.baseDelay, this.opts.maxDelay);
    this.attempt += 1;
    this.retryHandle = this.setTimer(() => {
      this.retryHandle = null;
      // A failed attempt reports itself through `fail`, which schedules the
      // next one, so there is nothing to do on the rejection here.
      void this.attemptConnect().then(
        () => {
          this.attempt = 0;
        },
        () => {},
      );
    }, delay);
  }
}
