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
  type HostFrame,
  type HostInfo,
  type HostTarget,
  isEventFrame,
  type PairResult,
  type RemoteClient,
  type RequestFrame,
  type StateHandler,
} from "./types";

/** Close codes that mean "do not retry" — the credential or the host's
 *  remote-access switch has to change first. */
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

const randomId = () =>
  globalThis.crypto?.randomUUID?.() ??
  `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;

export class ProtocolClient implements RemoteClient {
  private socket: Socket | null = null;
  private pending = new Map<string, Pending>();
  private listeners = new Map<string, Set<EventHandler>>();
  private stateListeners = new Set<StateHandler>();
  private snapshotListeners = new Set<(r: HelloResult) => void>();
  private target: HostTarget | null = null;
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

  /** The credential the client is currently authenticating with — the one
   *  `pair` minted, once a pairing handshake has run. */
  get deviceToken(): string | null {
    return this.target?.deviceToken ?? null;
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
    this.disconnect();
    this.target = target;
    this.closedByUs = false;
    this.attempt = 0;
    return this.handshake();
  }

  disconnect(): void {
    this.closedByUs = true;
    if (this.retryHandle !== null) {
      this.clearTimer(this.retryHandle);
      this.retryHandle = null;
    }
    this.teardown("Disconnected");
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

  async hello(deviceToken: string, client: DeviceInfo): Promise<HelloResult> {
    const result = await this.request<HelloResult>("hello", { deviceToken, client });
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

  /** Open the socket and run the first frame (`pair` or `hello`). */
  private async handshake(): Promise<HelloResult> {
    const target = this.target;
    if (!target) throw new Error("no host configured");
    this.setState(target.pairingToken ? "pairing" : "connecting");
    this.socket = await this.opts.openSocket(wsUrl(target), {
      onOpen: () => {},
      onMessage: (text) => this.onMessage(text),
      onClose: (code, reason) => this.onClose(code, reason),
      onError: (message) => this.onSocketError(message),
    });
    try {
      if (target.pairingToken) {
        const paired = await this.pair(target.pairingToken, this.opts.device);
        // Pairing is single use: keep going on the device token from here, so a
        // reconnect doesn't replay a spent pairing token.
        this.target = {
          ...target,
          pairingToken: undefined,
          deviceToken: paired.deviceToken,
        };
        this.setState("connected");
        // `pair` answers with the host identity but no snapshot, so ask for it.
        const workspace = await this.call<HelloResult["workspace"]>("get_workspace");
        return this.publishSnapshot({ host: paired.host, workspace });
      }
      if (!target.deviceToken) throw new Error("no device token");
      const result = await this.hello(target.deviceToken, this.opts.device);
      this.setState("connected");
      return this.publishSnapshot(result);
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      this.setState("error", message);
      throw e instanceof Error ? e : new Error(message);
    }
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
    if (FATAL_CLOSE.has(code)) {
      this.setState("error", message);
      return;
    }
    this.setState("error", message);
    this.scheduleRetry();
  }

  private teardown(reason: string) {
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
      void this.handshake()
        .then(() => {
          this.attempt = 0;
        })
        .catch(() => {
          if (!this.closedByUs) this.scheduleRetry();
        });
    }, delay);
  }
}
