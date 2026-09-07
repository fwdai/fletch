// The protocol client: envelope encoding, request/response matching, the
// pair/hello handshake, event fan-out and reconnect with backoff. Transport
// agnostic — it drives whatever `SocketFactory` it is handed, which is how the
// mock host and the real WebSocket share every line of this.

import { backoffDelay } from "./backoff";
import { wsUrl } from "./pairing";
import type { Socket, SocketFactory } from "./socket";
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
  type RemoteClient,
  type RequestFrame,
  type StateHandler,
} from "./types";

/** Close codes that mean "do not retry" — the device has to be paired again,
 *  or the host's remote-access switch turned back on. */
const FATAL_CLOSE = new Set([CLOSE_UNAUTHENTICATED, CLOSE_REMOTE_DISABLED]);

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

  get host(): HostInfo | null {
    return this._host;
  }

  /** The host identity this client is bound to — supplied by the pairing link
   *  or pinned on first contact. */
  get hostKey(): string | null {
    return this._target?.hostKey ?? null;
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
   *  (`pair` or `hello`). Every failure inside it — the open rejecting, the
   *  handshake being refused — goes through `fail`, so the client is never
   *  left sitting in `connecting` with nothing scheduled. */
  private async attemptConnect(): Promise<HelloResult> {
    let target = this._target;
    if (!target) throw new Error("no host configured");
    const gen = ++this.gen;
    this.setState(target.pairingToken ? "pairing" : "connecting");
    try {
      const socket = await this.opts.openSocket(
        wsUrl(target),
        {
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
        },
        { hostKey: target.hostKey },
      );
      if (!this.current(gen)) {
        // A newer attempt (or a disconnect) landed while the socket opened.
        socket.close();
        throw new Error("Connection superseded");
      }
      this.socket = socket;
      // Trust on first use: a hand-typed pairing has no key to compare, so the
      // one the handshake authenticated becomes the pinned identity. A target
      // that did have a key never gets here with a different one — the
      // transport refuses the connection.
      if (!target.hostKey) {
        target = { ...target, hostKey: socket.hostKey };
        this._target = target;
      }
      return await this.handshake(target);
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      // A pairing code is single use and short lived, so a refused pairing
      // attempt is the user's to retry. A host presenting the wrong identity
      // is never worth retrying either: only re-pairing can clear it.
      const retryable = !this._target?.pairingToken && !message.includes(HOST_KEY_MISMATCH);
      const shown = reportable(message);
      if (this.current(gen)) this.fail(shown, retryable);
      throw shown === message && e instanceof Error ? e : new Error(shown);
    }
  }

  private current(gen: number): boolean {
    return gen === this.gen;
  }

  /** The first frame on a fresh socket: `pair` when a pairing code is held,
   *  `hello` otherwise. */
  private async handshake(target: HostTarget): Promise<HelloResult> {
    if (target.pairingToken) {
      const paired = await this.pair(target.pairingToken, this.opts.device);
      // Pairing is single use: drop the code, so a reconnect greets the host
      // with `hello` on the device key it just registered.
      this._target = { ...target, pairingToken: undefined };
      this.setState("connected");
      // `pair` answers with the host identity but no snapshot, so ask for it.
      const workspace = await this.call<HelloResult["workspace"]>("get_workspace");
      return this.publishSnapshot({ host: paired.host, workspace });
    }
    const result = await this.hello(this.opts.device);
    this.setState("connected");
    return this.publishSnapshot(result);
  }

  /** The single failure path: report it, and schedule a retry unless the
   *  failure is one only the user can clear. */
  private fail(message: string, retryable: boolean) {
    this.setState("error", message);
    if (retryable && !this.closedByUs) this.scheduleRetry();
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
