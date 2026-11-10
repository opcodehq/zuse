// A transport moves RPC text frames. Paired encrypted transports implement the same contract.

export interface Transport {
  /** Resolves once the transport can send; rejects if it cannot open. */
  open(): Promise<void>;
  send(text: string): void;
  close(): void;
  onMessage: ((text: string) => void) | null;
  /** Fired once when an open transport is lost. */
  onClose: ((reason: string) => void) | null;
}

/** The owner's cookie-authenticated socket on the page's own origin. */
export class OwnerTransport implements Transport {
  onMessage: ((text: string) => void) | null = null;
  onClose: ((reason: string) => void) | null = null;
  private socket: WebSocket | null = null;

  constructor(private readonly url = ownerSocketUrl()) {}

  open(): Promise<void> {
    return new Promise((resolve, reject) => {
      const socket = new WebSocket(this.url);
      this.socket = socket;
      let opened = false;
      socket.onopen = () => {
        opened = true;
        resolve();
      };
      socket.onmessage = (event) => {
        if (typeof event.data === "string") this.onMessage?.(event.data);
      };
      socket.onclose = (event) => {
        if (!opened) reject(new Error("Could not reach the engine"));
        else this.onClose?.(event.reason || "Connection closed");
        this.socket = null;
      };
    });
  }

  send(text: string) {
    if (this.socket?.readyState !== WebSocket.OPEN) throw new Error("The engine is not connected");
    this.socket.send(text);
  }

  close() {
    const socket = this.socket;
    this.socket = null;
    socket?.close();
  }
}

export function ownerSocketUrl(location: Location = window.location): string {
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  return `${scheme}://${location.host}/api/rpc`;
}
