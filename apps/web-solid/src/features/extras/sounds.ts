// The desktop's recorded cues (crates/ui/assets/sounds) for the connection warning and a voice call's two ends.
// The bytes are fetched early (on idle): the connection cue plays exactly when the server that hosts them is gone.
import attentionUrl from "@repo/crates/ui/assets/sounds/attention.wav?url";
import voiceEndUrl from "@repo/crates/ui/assets/sounds/voice-end.wav?url";
import voiceStartUrl from "@repo/crates/ui/assets/sounds/voice-start.wav?url";

let context: AudioContext | undefined;
const bytes = new Map<string, Promise<ArrayBuffer | null>>();
const buffers = new Map<string, Promise<AudioBuffer | null>>();

function fetchBytes(url: string) {
  let pending = bytes.get(url);
  if (!pending) {
    pending = fetch(url)
      .then((response) => (response.ok ? response.arrayBuffer() : null))
      .catch(() => null);
    bytes.set(url, pending);
    // A failed fetch may succeed later.
    void pending.then((found) => found || bytes.delete(url));
  }
  return pending;
}

// Off the first-load path: idle time comes long before the connection cue's 10 second delay could elapse.
if (typeof window !== "undefined") {
  const prefetch = () => {
    for (const url of [attentionUrl, voiceStartUrl, voiceEndUrl]) void fetchBytes(url);
  };
  if ("requestIdleCallback" in window) requestIdleCallback(prefetch, { timeout: 5000 });
  else setTimeout(prefetch, 3000);
}

function audio() {
  if (!context && typeof AudioContext !== "undefined") context = new AudioContext();
  if (context?.state === "suspended") void context.resume().catch(() => {});
  return context;
}

// Browsers start audio suspended until a gesture; resume on the first one.
if (typeof document !== "undefined") {
  const unlock = () => {
    try {
      audio();
    } catch {
      // No audio device: cues stay silent.
    }
  };
  document.addEventListener("pointerdown", unlock, { passive: true });
  document.addEventListener("keydown", unlock, { passive: true });
}

function playFile(url: string) {
  const ctx = audio();
  if (!ctx) return;
  let buffer = buffers.get(url);
  if (!buffer) {
    // decodeAudioData detaches its input, so decode a copy and keep the bytes for a retry.
    buffer = fetchBytes(url).then((data) => (data ? ctx.decodeAudioData(data.slice(0)) : null)).catch(() => null);
    buffers.set(url, buffer);
    void buffer.then((decoded) => decoded || buffers.delete(url));
  }
  void buffer.then((decoded) => {
    if (!decoded) return;
    const source = ctx.createBufferSource();
    source.buffer = decoded;
    source.connect(ctx.destination);
    source.onended = () => source.disconnect();
    source.start();
  });
}

export function playSound(sound: "attention") {
  try {
    if (sound === "attention") playFile(attentionUrl);
  } catch {
    // Audio is a courtesy; never let it break the session flow.
  }
}

/** A voice call's two ends sound, never in between. */
export function playVoiceCue(start: boolean) {
  try {
    playFile(start ? voiceStartUrl : voiceEndUrl);
  } catch {
    // As above.
  }
}
