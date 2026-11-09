import { API_VERSION, type ControlInfo, type ControlMethod, type Zot, type ZotCommand, type ZotEvent, type ZotFile } from './generated.js';
export * from './generated.js';

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export type ErrorCode = 'unauthorized' | 'unsupported' | 'transport' | 'cancelled' | 'overflow' | 'operation';
export class ZuseError extends Error {
  constructor(public readonly code: ErrorCode, message: string) { super(message); this.name = 'ZuseError'; }
}
export interface ClientOptions {
  endpoint: string;
  token: string;
  /** Supply a compatible constructor for tests or a non-browser runtime. */
  webSocket?: typeof WebSocket;
}
export interface RequestOptions { signal?: AbortSignal; targetDeviceId?: string }
interface Pending {
  started(id: number): void;
  value(value: unknown): void;
  error(error: ZuseError): void;
  done(): void;
  cleanup(): void;
}

/** One authenticated multiplexed connection; mutations are never silently replayed. */
export function createZuseClient(options: ClientOptions) {
  let endpoint: URL;
  try { endpoint = new URL(options.endpoint); }
  catch { throw new ZuseError('transport', 'Invalid control endpoint'); }
  if (!['ws:', 'wss:'].includes(endpoint.protocol) || endpoint.username || endpoint.password || endpoint.search || endpoint.hash) {
    throw new ZuseError('transport', 'Use a WebSocket endpoint without embedded credentials');
  }
  if (endpoint.protocol === 'ws:' && !['localhost', '127.0.0.1', '[::1]'].includes(endpoint.hostname)) {
    throw new ZuseError('transport', 'Remote control requires TLS');
  }
  if (!options.token) throw new ZuseError('unauthorized', 'A control credential is required');
  const Socket = options.webSocket ?? globalThis.WebSocket;
  if (!Socket) throw new ZuseError('transport', 'A WebSocket implementation is required');
  let socket: WebSocket;
  try { socket = new Socket(endpoint.toString()); }
  catch { throw new ZuseError('transport', 'Could not create a Zuse connection'); }
  const lifetime = new AbortController();
  let sequence = 0;
  let closed = false;
  const pending = new Map<number, Pending>();
  const opened = new Promise<void>((resolve, reject) => {
    socket.addEventListener('open', () => resolve(), { once: true });
    socket.addEventListener('error', () => reject(new ZuseError('transport', 'Could not connect to Zuse')), { once: true });
    socket.addEventListener('close', () => reject(new ZuseError('transport', 'Zuse connection closed')), { once: true });
  });
  // Connecting starts immediately; callers may attach after a failed handshake.
  void opened.catch(() => {});
  const finish = (id: number) => { const item = pending.get(id); if (item) { pending.delete(id); item.cleanup(); } };
  const disconnect = () => {
    if (closed) return;
    closed = true;
    lifetime.abort();
    for (const [id, item] of pending) { finish(id); item.error(new ZuseError('transport', 'Zuse connection closed; reconcile mutations before retrying')); }
  };
  const cancel = (id: number) => {
    finish(id);
    if (!closed) {
      try { socket.send(JSON.stringify({ id, cancel: true })); } catch { /* Cancellation is best effort; never replay. */ }
    }
  };
  socket.addEventListener('close', disconnect);
  socket.addEventListener('message', event => {
    let frame: { id: number; ok?: unknown; item?: unknown; err?: string; done?: boolean };
    try {
      frame = JSON.parse(String(event.data));
      if (!frame || typeof frame !== 'object' || !Number.isSafeInteger(frame.id)
        || (frame.err !== undefined && typeof frame.err !== 'string')
        || !(typeof frame.err === 'string' || Object.hasOwn(frame, 'item') || frame.done === true || Object.hasOwn(frame, 'ok'))) {
        throw new Error('Invalid response frame');
      }
    } catch { disconnect(); socket.close(); return; }
    const item = pending.get(frame.id);
    if (!item) return;
    if (frame.err !== undefined) {
      finish(frame.id);
      const code = frame.err.includes('unauthorized') ? 'unauthorized' : /unsupported|unknown method|capability error/.test(frame.err) ? 'unsupported' : 'operation';
      item.error(new ZuseError(code, frame.err));
    } else if (Object.hasOwn(frame, 'item')) { item.value(frame.item); }
    else if (frame.done) { finish(frame.id); item.done(); }
    else if (Object.hasOwn(frame, 'ok')) { item.value(frame.ok); }
  });

  async function send(method: ControlMethod, params: object, settings: RequestOptions, receiver: Pending): Promise<void> {
    if (closed) throw new ZuseError('transport', 'Zuse connection is closed');
    if (settings.signal?.aborted) throw new ZuseError('cancelled', 'Request cancelled');
    const signal = AbortSignal.any([lifetime.signal, ...(settings.signal ? [settings.signal] : [])]);
    await new Promise<void>((resolve, reject) => {
      const abort = () => { signal.removeEventListener('abort', abort); reject(new ZuseError(closed ? 'transport' : 'cancelled', closed ? 'Zuse connection is closed' : 'Request cancelled')); };
      signal.addEventListener('abort', abort, { once: true });
      void opened.then(() => { signal.removeEventListener('abort', abort); resolve(); }, error => {
        signal.removeEventListener('abort', abort); reject(error);
      });
    });
    if (closed) throw new ZuseError('transport', 'Zuse connection is closed');
    if (settings.signal?.aborted) throw new ZuseError('cancelled', 'Request cancelled');
    const id = ++sequence;
    const abort = () => { cancel(id); receiver.error(new ZuseError('cancelled', 'Request cancelled')); };
    receiver.started(id);
    pending.set(id, { ...receiver, cleanup: () => { settings.signal?.removeEventListener('abort', abort); receiver.cleanup(); } });
    settings.signal?.addEventListener('abort', abort, { once: true });
    try {
      socket.send(JSON.stringify({ id, method: 'ControlCall', params: {
        token: options.token, method, params: { ...params, ...(settings.targetDeviceId ? { targetDeviceId: settings.targetDeviceId } : {}) },
      } }));
    } catch { finish(id); throw new ZuseError('transport', 'Could not send request'); }
  }

  function call<T = Json>(method: ControlMethod, params: object = {}, settings: RequestOptions = {}): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      let id: number | undefined;
      void send(method, params, settings, {
        started(value) { id = value; },
        value(value) {
          if (id !== undefined) {
            finish(id);
            if (isStream(method)) cancel(id);
          }
          resolve(value as T);
        },
        error: reject, done() { reject(new ZuseError('operation', 'Operation ended without a result')); }, cleanup() {},
      }).catch(reject);
    });
  }

  function watch<T = Json>(method: ControlMethod, params: object = {}, settings: RequestOptions = {}): AsyncIterableIterator<T> {
    if (!isStream(method)) throw new ZuseError('unsupported', 'This operation does not provide a subscription');
    const buffer: T[] = [];
    let failure: ZuseError | undefined;
    let ended = false;
    const waiters = new Set<() => void>();
    const wake = () => { for (const resolve of waiters) resolve(); waiters.clear(); };
    let id: number | undefined;
    let returned = false;
    const controller = new AbortController();
    const stop = () => { if (id !== undefined) cancel(id); controller.abort(); };
    void send(method, params, { ...settings, signal: AbortSignal.any([controller.signal, ...(settings.signal ? [settings.signal] : [])]) }, {
      started(value) { id = value; },
      value(value) {
        if (buffer.length >= 256) { failure = new ZuseError('overflow', 'Subscription exceeded its buffer; resubscribe for a fresh snapshot'); ended = true; stop(); }
        else buffer.push(value as T);
        wake();
      },
      error(error) { if (!returned && !failure) failure = error; ended = true; wake(); },
      done() { ended = true; wake(); }, cleanup() {},
    }).catch(error => { if (!returned && !failure) failure = error; ended = true; wake(); });
    return {
      [Symbol.asyncIterator]() { return this; },
      async next() {
        while (!buffer.length && !ended) await new Promise<void>(resolve => { waiters.add(resolve); });
        if (failure) throw failure;
        if (buffer.length) return { value: buffer.shift()!, done: false };
        return { value: undefined, done: true };
      },
      async return() { returned = true; ended = true; buffer.length = 0; stop(); wake(); return { value: undefined, done: true }; },
    };
  }

  return {
    call, watch,
    async info(settings?: RequestOptions) {
      const info = await call<ControlInfo>('ControlInfo', {}, settings);
      if (info.apiVersion !== API_VERSION) throw new ZuseError('unsupported', `Unsupported control API version ${info.apiVersion}`);
      return info;
    },
    zots: {
      watch: (settings?: RequestOptions) => watch<Zot[]>('WatchZots', {}, settings),
      command: (command: ZotCommand, settings?: RequestOptions) => call<Zot | null>('ZotCommand', command, settings),
      files: (zotId: string, history = false, settings?: RequestOptions) => call<ZotFile[]>('ZotFiles', { zotId, history }, settings),
      events: (zotId: string, settings?: RequestOptions) => call<ZotEvent[]>('ZotEvents', { zotId }, settings),
      acknowledge: (zotId: string, eventIds: string[], settings?: RequestOptions) => call('AckZotEvents', { zotId, eventIds }, settings),
    },
    close() { disconnect(); socket.close(); },
  };
}

function isStream(method: ControlMethod): boolean {
  return method.startsWith('Watch') || method === 'SubscribeTerminal' || method === 'UpdateStatus';
}
