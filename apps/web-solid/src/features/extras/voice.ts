// Browser voice call: host signaling and call ownership (port of crates/viewport/src/voice.rs),
// with media in VoiceMedia. Each step has a deadline; anything from an older attempt is ignored.
import { createEffect, createRoot, on } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import type { Subscription } from "@/core/rpc";
import { state } from "@/core/store";
import { view } from "@/core/view";
import { playVoiceCue } from "./sounds";
import { harnessName } from "./util";
import { VoiceMedia, type MediaEvent } from "./voice-media";

const CAPABILITY = "voice-client-media-v1";
const HEARTBEAT_MS = 5000;
const CAPTION_TURNS = 8;
const MAX_TRANSCRIPT_BYTES = 32_768;
/** The voice runtime is provided by this harness on the execution computer. */
const VOICE_HARNESS = "codex";

type Step = "capabilities" | "media" | "preparing" | "owner" | "offer" | "negotiating" | "answer" | "confirming" | "activating" | "active";
export type VoicePhase = "closed" | "checking" | "starting" | "active" | "stopping" | "failed";
type Work = "idle" | "working" | "awaitingInput";
export type OrbState = "connecting" | "composing" | "solving" | "working" | "breathing" | "listening";

interface Lease {
  hostDeviceId: string;
  voice: { sessionId: string; generation: number; token: string };
}
interface Snapshot {
  sessionId: string;
  chatId: string;
  generation: number;
  phase: VoicePhase;
  muted: boolean;
  playing: boolean;
  work: Work;
  reason?: string | null;
}
type VoiceEvent =
  | { type: "snapshot"; snapshot: Snapshot }
  | { type: "partial"; generation: number; itemId?: string | null; text: string }
  | { type: "final"; transcript: { sessionId: string; itemId: string; role: "user" | "assistant"; text: string } }
  | { type: "invalidatePlayout"; generation: number; itemId: string }
  | { type: "closed"; generation: number; reason?: string | null }
  | { type: string };

interface CaptionTurn {
  item: string | null;
  text: string;
  role: "user" | "assistant" | null;
}

/** What the call shows. Signaling internals live in `call` below, outside the store. */
export interface VoiceView {
  step: Step;
  phase: VoicePhase;
  snapshot: Snapshot | null;
  muted: boolean;
  microphone: number;
  speaker: number;
  caption: string;
}

const [voice, setVoice] = createStore<{ view: VoiceView | null; notice: string | null; stageOpen: boolean }>({
  view: null,
  notice: null,
  stageOpen: false,
});
export { voice };
export const voiceCall = () => voice.view;

interface Call {
  key: string;
  host: string;
  config: Record<string, unknown>;
  deadline: number;
  lease: Lease | null;
  negotiation: string | null;
  owner: Subscription | null;
  reportAt: number;
  reportDeadline: number | null;
  sequence: number;
  media: VoiceMedia;
  turns: CaptionTurn[];
  speakerPlaying: boolean;
  lastLoud: number;
  timer: ReturnType<typeof setInterval>;
}
let call: Call | null = null;

const agent = () => harnessName(VOICE_HARNESS);

function voiceError(error: string) {
  if (error.includes("ChatgptRequired")) return `Sign in to ${agent()} with its subscription account on the execution computer to use voice.`;
  if (error.includes("NativeRuntimeUnavailable")) return `Install or update ${agent()} with its official installer on the execution computer to use voice.`;
  if (error.includes("Busy")) return "Another window already owns the voice session.";
  if (error.includes("Unsupported")) return `Update ${agent()} on the execution computer to use voice.`;
  if (error.includes("InvalidLease")) return "Voice ownership expired. Start a fresh call.";
  return `Voice could not connect. Check ${agent()} and its account on the execution computer, then start a fresh call.`;
}

const TIMED_OUT = "Voice connection timed out. Start a fresh call to try again.";
const INVALID = "The host returned an invalid voice response.";

function setStep(step: Step, seconds: number) {
  if (!call) return;
  call.deadline = Date.now() + seconds * 1000;
  setVoice("view", "step", step);
}

const expired = (c: Call) => (voice.view?.step !== "active" && Date.now() >= c.deadline) || (c.reportDeadline !== null && Date.now() >= c.reportDeadline);

function request<T>(method: string, payload: unknown) {
  const c = call!;
  return rpc.call<T>(method, { targetDeviceId: c.host, payload }).then(
    (value) => ({ current: call === c && !expired(c), value, error: null as string | null }),
    (error: unknown) => ({ current: call === c, value: null as T | null, error: error instanceof Error ? error.message : String(error) }),
  );
}

export function startVoice() {
  if (call || state.connection.kind !== "connected") return;
  const chat = view.selectedChatId ? state.chats[view.selectedChatId] : undefined;
  const host = chat?.deviceId ?? state.localDeviceId;
  if (!host) return;
  const config =
    chat?.config && chat.config.harness === VOICE_HARNESS
      ? { ...chat.config }
      : { harness: VOICE_HARNESS, model: null, reasoning: null, sandbox: "workspace-write" };
  const key = crypto.randomUUID();
  const media = new VoiceMedia((event) => receiveMedia(key, event));
  call = {
    key,
    host,
    config,
    deadline: Date.now() + 8000,
    lease: null,
    negotiation: null,
    owner: null,
    reportAt: Date.now(),
    reportDeadline: null,
    sequence: 0,
    media,
    turns: [],
    speakerPlaying: false,
    lastLoud: 0,
    timer: setInterval(tick, 1000),
  };
  setVoice({
    notice: null,
    stageOpen: true,
    view: { step: "capabilities", phase: "checking", snapshot: null, muted: false, microphone: 0, speaker: 0, caption: "" },
  });
  playVoiceCue(true);
  void request<{ protocol?: string; clientWebrtc?: boolean }>(M.VOICE_CAPABILITIES_V2, {}).then(({ current, value, error }) => {
    if (!current) return expireIfCurrent(key);
    if (error) return endVoice(voiceError(error));
    if (!value || value.protocol !== CAPABILITY || value.clientWebrtc !== true) return endVoice(INVALID);
    setStep("media", 60);
    call!.media.prepare();
  });
}

function expireIfCurrent(key: string) {
  if (call?.key === key && expired(call)) endVoice(TIMED_OUT);
}

export function endVoice(reason: string | null = null) {
  const c = call;
  if (!c) return;
  call = null;
  // Local tracks stop synchronously, before host cleanup.
  c.media.close();
  clearInterval(c.timer);
  c.owner?.cancel();
  if (rpc.connected) {
    if (c.lease) void rpc.call(M.STOP_VOICE_V2, { targetDeviceId: c.host, payload: c.lease }).catch(() => {});
    void rpc.call(M.CANCEL_VOICE_ATTEMPT_V2, { targetDeviceId: c.host, payload: { attemptKey: c.key } }).catch(() => {});
  }
  // Desktop: a hang-up or a connected call ending sounds; a failure to connect only explains itself.
  if (reason === null || voice.view?.step === "active") playVoiceCue(false);
  setVoice({ view: null, notice: reason, stageOpen: false });
}

export function toggleVoiceMute() {
  if (!call || !voice.view) return;
  const muted = !voice.view.muted;
  setVoice("view", produce((v) => {
    if (!v) return;
    v.muted = muted;
    if (v.snapshot) v.snapshot.muted = muted;
  }));
  call.media.setMuted(muted);
}

export const setVoiceStageOpen = (open: boolean) => setVoice("stageOpen", open);
export const dismissVoiceNotice = () => setVoice({ notice: null, stageOpen: false });

function tick() {
  const c = call;
  if (!c) return;
  if (expired(c)) return endVoice(TIMED_OUT);
  const step = voice.view?.step;
  if (c.lease && c.owner && step !== "owner" && c.reportDeadline === null && Date.now() >= c.reportAt) {
    const sequence = ++c.sequence;
    c.reportDeadline = Date.now() + 10_000;
    void request<{ sequence?: number }>(M.REPORT_VOICE_MEDIA_V2, { lease: c.lease, sequence, muted: voice.view?.muted ?? false, state: "ready" }).then(
      ({ current, value, error }) => {
        if (!current) return expireIfCurrent(c.key);
        if (error) return endVoice(voiceError(error));
        if (value?.sequence !== sequence) return endVoice(INVALID);
        c.reportDeadline = null;
        c.reportAt = Date.now() + HEARTBEAT_MS;
        c.media.lease();
      },
    );
  }
}

function receiveMedia(key: string, event: MediaEvent) {
  const c = call;
  if (!c || c.key !== key) return;
  if (expired(c)) return endVoice(TIMED_OUT);
  if (event.kind === "error") return endVoice(event.message);
  const step = voice.view?.step;
  if (step === "media" && event.kind === "prepared") {
    setStep("preparing", 65);
    void request<{ lease?: Lease }>(M.PREPARE_VOICE_V2, { attemptKey: c.key, config: c.config, voice: null }).then(({ current, value, error }) => {
      if (!current) return expireIfCurrent(key);
      if (error) return endVoice(voiceError(error));
      const lease = value?.lease;
      if (!lease || lease.hostDeviceId !== c.host) return endVoice(INVALID);
      c.lease = lease;
      setStep("owner", 8);
      c.owner = rpc.subscribe<VoiceEvent>(
        M.OWN_VOICE_V2,
        { targetDeviceId: c.host, payload: lease },
        (item) => receiveOwner(c, item),
        () => {
          if (call === c) endVoice("Voice ownership ended. Start a fresh call to reconnect.");
        },
      );
    });
  } else if (step === "offer" && event.kind === "offer") {
    const negotiation = crypto.randomUUID();
    c.negotiation = negotiation;
    setStep("negotiating", 95);
    void request<{ negotiationId?: string; answer?: string }>(M.NEGOTIATE_VOICE_V2, { lease: c.lease, negotiationId: negotiation, offer: event.sdp }).then(
      ({ current, value, error }) => {
        if (!current) return expireIfCurrent(key);
        if (error) return endVoice(voiceError(error));
        if (!value?.answer || value.negotiationId !== c.negotiation) return endVoice(INVALID);
        setStep("answer", 30);
        c.media.answer(value.answer);
      },
    );
  } else if (step === "answer" && event.kind === "ready") {
    setStep("confirming", 8);
    void request<Snapshot>(M.CONFIRM_VOICE_MEDIA_V2, { lease: c.lease, negotiationId: c.negotiation, muted: voice.view?.muted ?? false }).then(
      ({ current, value, error }) => {
        if (!current) return expireIfCurrent(key);
        if (error) return endVoice(voiceError(error));
        const lease = c.lease;
        if (!value || !lease || value.sessionId !== lease.voice.sessionId || value.generation !== lease.voice.generation || value.phase !== "active")
          return endVoice(INVALID);
        reduce(c, { type: "snapshot", snapshot: value });
        setStep("activating", 10);
        c.media.activate(voice.view?.muted ?? false);
      },
    );
  } else if (step === "activating" && event.kind === "active") {
    setVoice("view", produce((v) => {
      if (!v) return;
      v.step = "active";
      v.phase = "active";
    }));
  } else if (step === "active" && event.kind === "levels") {
    levels(c, event.microphone, event.speaker);
  }
}

function receiveOwner(c: Call, event: VoiceEvent) {
  if (call !== c) return;
  if (expired(c)) return endVoice(TIMED_OUT);
  const lease = c.lease;
  if (!lease) return;
  let valid: boolean;
  switch (event.type) {
    case "snapshot": {
      const s = (event as { snapshot: Snapshot }).snapshot;
      valid = s?.sessionId === lease.voice.sessionId && s.generation === lease.voice.generation;
      break;
    }
    case "final":
      valid = (event as { transcript: { sessionId: string } }).transcript?.sessionId === lease.voice.sessionId;
      break;
    case "partial":
    case "closed":
    case "invalidatePlayout":
      valid = (event as { generation: number }).generation === lease.voice.generation;
      break;
    default:
      return endVoice("The host sent unsupported voice media.");
  }
  if (!valid) return;
  if (event.type === "closed") {
    const reason = (event as { reason?: string | null }).reason;
    return endVoice(reason ? voiceError(reason) : null);
  }
  if (event.type === "snapshot") {
    const s = (event as { snapshot: Snapshot }).snapshot;
    if (s.phase === "closed" || s.phase === "failed") return endVoice(s.reason ? voiceError(s.reason) : null);
  }
  reduce(c, event);
  if (event.type === "snapshot" && voice.view?.step === "owner") {
    setStep("offer", 30);
    c.media.offer();
  }
}

/** VoiceView::reduce from crates/voice-session/src/view.rs. */
function reduce(c: Call, event: VoiceEvent) {
  const v = voice.view;
  if (!v) return;
  const current = (generation: number) => v.snapshot?.generation === generation;
  if (event.type === "snapshot") {
    const next = { ...(event as { snapshot: Snapshot }).snapshot };
    if (v.snapshot) {
      if (v.snapshot.generation !== next.generation || v.snapshot.sessionId !== next.sessionId) return;
      next.playing = v.snapshot.playing;
    }
    next.muted = v.muted;
    setVoice("view", produce((view) => {
      if (!view) return;
      view.snapshot = next;
      view.phase = next.phase;
    }));
  } else if (event.type === "partial") {
    const e = event as { generation: number; itemId?: string | null; text: string };
    if (!current(e.generation)) return;
    partial(c, e.itemId ?? null, e.text);
  } else if (event.type === "final") {
    const t = (event as { transcript: { sessionId: string; itemId: string; role: "user" | "assistant"; text: string } }).transcript;
    if (v.snapshot?.sessionId !== t.sessionId) return;
    complete(c, t.itemId, t.role, t.text);
  } else {
    return;
  }
  setVoice("view", "caption", c.turns[c.turns.length - 1]?.text ?? "");
}

function pushTurn(c: Call, turn: CaptionTurn) {
  c.turns.push(turn);
  if (c.turns.length > CAPTION_TURNS) c.turns.splice(0, c.turns.length - CAPTION_TURNS);
}

const bytes = (text: string) => new TextEncoder().encode(text).length;

function partial(c: Call, item: string | null, delta: string) {
  let index = item !== null ? c.turns.map((t) => t.item).lastIndexOf(item) : c.turns.length - 1;
  if (item === null && (index < 0 || c.turns[index]!.role !== null)) index = -1;
  if (index < 0) {
    pushTurn(c, { item, text: "", role: null });
    index = c.turns.length - 1;
  }
  const turn = c.turns[index]!;
  if (turn.role !== null) return;
  // A runaway utterance shows its tail rather than grow.
  if (bytes(turn.text) + bytes(delta) > MAX_TRANSCRIPT_BYTES) turn.text = "";
  turn.text += delta;
}

function complete(c: Call, item: string, role: "user" | "assistant", text: string) {
  let index = c.turns.map((t) => t.item).lastIndexOf(item);
  const last = c.turns[c.turns.length - 1];
  if (index < 0 && last && last.item === null && last.role === null) index = c.turns.length - 1;
  if (index >= 0) {
    Object.assign(c.turns[index]!, { item, text, role });
    return;
  }
  const turn = { item, text, role };
  // A turn that completes while another streams is older; it never takes the caption.
  if (last && last.role === null) {
    c.turns.splice(c.turns.length - 1, 0, turn);
    if (c.turns.length > CAPTION_TURNS) c.turns.splice(0, c.turns.length - CAPTION_TURNS);
  } else {
    pushTurn(c, turn);
  }
}

/** Client media has no host meter, so playing is derived from playout peaks with hysteresis. */
function levels(c: Call, microphone: number, speaker: number) {
  const now = Date.now();
  if (speaker >= (c.speakerPlaying ? 328 : 655)) {
    c.speakerPlaying = true;
    c.lastLoud = now;
  } else if (now - c.lastLoud >= 250) {
    c.speakerPlaying = false;
  }
  setVoice("view", produce((v) => {
    if (!v) return;
    v.microphone = microphone;
    v.speaker = speaker;
    if (v.snapshot) v.snapshot.playing = c.speakerPlaying;
  }));
}

export function orbState(v: VoiceView): OrbState {
  if (v.phase === "checking" || v.phase === "starting" || v.phase === "stopping" || v.step !== "active") return "connecting";
  const s = v.snapshot;
  if (!s) return "breathing";
  if (s.playing) return "composing";
  if (s.work === "awaitingInput") return "solving";
  if (s.work === "working") return "working";
  return v.muted ? "breathing" : "listening";
}

// A dropped connection retires the call; ownership never silently resumes.
createRoot(() => {
  createEffect(
    on(
      () => state.epoch,
      () => {
        if (call) endVoice("Voice stopped after the connection changed. Start a fresh call to reconnect.");
      },
      { defer: true },
    ),
  );
  createEffect(
    on(
      () => state.connection.kind,
      (kind) => {
        if (kind !== "connected" && call) endVoice("Voice stopped after the connection changed. Start a fresh call to reconnect.");
      },
      { defer: true },
    ),
  );
});
