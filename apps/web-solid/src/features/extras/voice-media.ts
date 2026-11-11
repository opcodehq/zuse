// Ephemeral browser WebRTC for a voice call (port of apps/web/src/voice_media.js).
// No audio, SDP or device state enters browser storage. Every step re-checks that its call is still current.
export type MediaEvent =
  | { kind: "prepared" }
  | { kind: "offer"; sdp: string }
  | { kind: "ready" }
  | { kind: "active" }
  | { kind: "levels"; microphone: number; speaker: number }
  | { kind: "error"; message: string };

const MAX_SDP_BYTES = 64 * 1024;

interface Meter {
  node: AnalyserNode;
  samples: Uint8Array<ArrayBuffer>;
}

class Retired extends Error {}

export class VoiceMedia {
  private closed = false;
  private activated = false;
  private muted = false;
  private stream?: MediaStream;
  private ctx?: AudioContext;
  private gain?: GainNode;
  private pc?: RTCPeerConnection;
  private microphone?: Meter;
  private speaker?: Meter;
  private meter?: ReturnType<typeof setInterval>;
  private watchdog?: ReturnType<typeof setInterval>;
  private leaseUntil = 0;
  private readonly initialDeadline = performance.now() + 150_000;
  private readonly nodes: AudioNode[] = [];
  private readonly timers = new Set<ReturnType<typeof setTimeout>>();
  private readonly waiters = new Set<() => void>();
  private readonly unlock = () => {
    if (this.ctx?.state === "suspended") void this.ctx.resume().catch(() => {});
  };
  private readonly hide = () => this.close();

  constructor(private readonly report: (event: MediaEvent) => void) {
    window.addEventListener("pagehide", this.hide);
    document.addEventListener("pointerdown", this.unlock, { passive: true });
    document.addEventListener("keydown", this.unlock, { passive: true });
  }

  private emit(event: MediaEvent) {
    if (!this.closed) this.report(event);
  }

  private live() {
    if (this.closed) throw new Retired();
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    window.removeEventListener("pagehide", this.hide);
    document.removeEventListener("pointerdown", this.unlock);
    document.removeEventListener("keydown", this.unlock);
    clearInterval(this.meter);
    clearInterval(this.watchdog);
    for (const cancel of [...this.waiters]) cancel();
    for (const timer of this.timers) clearTimeout(timer);
    for (const track of this.stream?.getTracks() ?? []) track.stop();
    this.pc?.close();
    for (const node of this.nodes) {
      try {
        node.disconnect();
      } catch {
        // Already disconnected.
      }
    }
    if (this.ctx) void this.ctx.close().catch(() => {});
  }

  private fail(error: unknown) {
    if (error instanceof Retired || this.closed) return;
    const name = error instanceof DOMException ? error.name : "";
    const message =
      name === "NotAllowedError"
        ? "Microphone permission was denied. Allow access and start a fresh call."
        : name === "NotFoundError"
          ? "No microphone is available on this computer."
          : name === "NotSupportedError"
            ? "This browser does not support voice calls."
            : "The voice media connection failed. Start a fresh call to try again.";
    this.emit({ kind: "error", message });
    this.close();
  }

  private wait(target: EventTarget, event: string, ready: () => boolean, ms: number) {
    return new Promise<void>((resolve, reject) => {
      let finished = false;
      const done = (error?: Error) => {
        if (finished) return;
        finished = true;
        clearTimeout(timer);
        this.timers.delete(timer);
        this.waiters.delete(cancel);
        target.removeEventListener(event, check);
        if (error) reject(error);
        else resolve();
      };
      const cancel = () => done(new Retired());
      const check = () => {
        if (this.closed) done(new Retired());
        else if (ready()) done();
      };
      const timer = setTimeout(() => done(new Error("Media deadline exceeded")), ms);
      this.timers.add(timer);
      this.waiters.add(cancel);
      target.addEventListener(event, check);
      check();
    });
  }

  private analyser(stream: MediaStream): Meter {
    const source = this.ctx!.createMediaStreamSource(stream);
    const node = this.ctx!.createAnalyser();
    node.fftSize = 256;
    source.connect(node);
    this.nodes.push(source, node);
    return { node, samples: new Uint8Array(new ArrayBuffer(node.fftSize)) };
  }

  private static peak(meter?: Meter) {
    if (!meter) return 0;
    meter.node.getByteTimeDomainData(meter.samples);
    let loudness = 0;
    for (const value of meter.samples) loudness = Math.max(loudness, Math.abs(value - 128) / 128);
    return Math.min(65535, Math.round(loudness * 65535));
  }

  private applyMute() {
    this.live();
    for (const track of this.stream?.getAudioTracks() ?? []) track.enabled = this.activated && !this.muted;
    if (this.gain) this.gain.gain.value = this.activated ? 1 : 0;
  }

  /** Ask for the microphone and build the peer connection, muted and silent until activation. */
  prepare() {
    this.watchdog = setInterval(() => {
      if (!this.closed && performance.now() >= (this.leaseUntil || this.initialDeadline)) this.fail(new Error("Voice controller expired"));
    }, 1000);
    void (async () => {
      if (!window.isSecureContext || !navigator.mediaDevices?.getUserMedia || !window.RTCPeerConnection || !window.AudioContext)
        throw new DOMException("Voice unsupported", "NotSupportedError");
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
        video: false,
      });
      // Permission can resolve after cancellation; retire those tracks at once.
      for (const track of stream.getTracks()) track.enabled = false;
      if (this.closed) {
        for (const track of stream.getTracks()) track.stop();
        return;
      }
      this.stream = stream;
      this.ctx = new AudioContext();
      this.gain = this.ctx.createGain();
      this.gain.gain.value = 0;
      this.gain.connect(this.ctx.destination);
      this.nodes.push(this.gain);
      this.microphone = this.analyser(stream);
      const pc = new RTCPeerConnection({ bundlePolicy: "max-bundle" });
      this.pc = pc;
      pc.createDataChannel("oai-events");
      for (const track of stream.getAudioTracks()) {
        track.onended = () => this.fail(new Error("Microphone ended"));
        pc.addTrack(track, stream);
      }
      pc.ontrack = (event) => {
        if (this.closed) return;
        this.speaker = this.analyser(event.streams[0] ?? new MediaStream([event.track]));
        this.speaker.node.connect(this.gain!);
      };
      pc.onconnectionstatechange = () => {
        if (this.closed) return;
        if (pc.connectionState === "failed" || pc.connectionState === "closed") this.fail(new Error("Connection ended"));
        if (pc.connectionState === "disconnected") {
          const timer = setTimeout(() => {
            this.timers.delete(timer);
            if (!this.closed && pc.connectionState === "disconnected") this.fail(new Error("Connection lost"));
          }, 3000);
          this.timers.add(timer);
        }
      };
      this.emit({ kind: "prepared" });
    })().catch((error) => this.fail(error));
  }

  /** The host renewed its lease; the watchdog ends calls whose lease lapses. */
  lease() {
    if (this.closed) return;
    if (this.leaseUntil && performance.now() >= this.leaseUntil) return this.fail(new Error("Voice lease expired"));
    this.leaseUntil = performance.now() + 15_000;
  }

  setMuted(muted: boolean) {
    this.muted = muted;
    try {
      this.applyMute();
    } catch (error) {
      this.fail(error);
    }
  }

  offer() {
    void (async () => {
      const pc = this.pc!;
      await pc.setLocalDescription(await pc.createOffer());
      this.live();
      await this.wait(pc, "icegatheringstatechange", () => pc.iceGatheringState === "complete", 20_000);
      this.live();
      const sdp = pc.localDescription?.sdp;
      if (!sdp || new TextEncoder().encode(sdp).length > MAX_SDP_BYTES) throw new Error("Invalid offer");
      this.emit({ kind: "offer", sdp });
    })().catch((error) => this.fail(error));
  }

  answer(sdp: string) {
    void (async () => {
      const pc = this.pc!;
      await pc.setRemoteDescription({ type: "answer", sdp });
      this.live();
      await this.wait(pc, "connectionstatechange", () => pc.connectionState === "connected", 25_000);
      this.live();
      this.emit({ kind: "ready" });
    })().catch((error) => this.fail(error));
  }

  activate(muted: boolean) {
    void (async () => {
      this.muted = muted;
      await this.ctx!.resume();
      this.live();
      this.activated = true;
      this.applyMute();
      this.meter = setInterval(() => {
        if (!this.closed)
          this.emit({ kind: "levels", microphone: this.muted ? 0 : VoiceMedia.peak(this.microphone), speaker: VoiceMedia.peak(this.speaker) });
      }, 100);
      this.emit({ kind: "active" });
    })().catch((error) => this.fail(error));
  }
}
