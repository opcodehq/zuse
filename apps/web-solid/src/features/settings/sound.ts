// Completion cues synthesised with Web Audio from crates/protocol/src/completion_sound.rs; other cues are desktop's WAVs.
import attentionUrl from "@repo/crates/ui/assets/sounds/attention.wav?url";
import requestUrl from "@repo/crates/ui/assets/sounds/request.wav?url";
import type { CompletionPreset } from "./prefs";

type Tone = [offset: number, frequency: number, duration: number, gain: number, wave?: OscillatorType];

const TONES: Record<CompletionPreset, Tone[]> = {
  chime: [[0, 587.33, 0.18, 0.055], [0.12, 783.99, 0.28, 0.046]],
  soft: [[0, 523.25, 0.24, 0.048], [0.12, 659.25, 0.32, 0.04]],
  pop: [[0, 740, 0.12, 0.06, "triangle"], [0.08, 987.77, 0.14, 0.034, "triangle"]],
  bell: [[0, 880, 0.22, 0.052], [0.04, 1320, 0.42, 0.024]],
  rise: [[0, 440, 0.16, 0.04], [0.11, 554.37, 0.18, 0.044], [0.23, 659.25, 0.24, 0.04]],
  bloom: [[0, 392, 0.34, 0.034], [0.04, 493.88, 0.38, 0.035], [0.11, 659.25, 0.34, 0.032]],
};

let context: AudioContext | null = null;
function audio() {
  context ??= new AudioContext();
  if (context.state === "suspended") void context.resume().catch(() => {});
  return context;
}

// Creating the context costs tens of milliseconds; do it while idle after the first gesture (which audio needs anyway)
// rather than as a turn finishes.
if (typeof window !== "undefined") {
  const warm = () => {
    const idle = window.requestIdleCallback ?? ((run: () => void) => setTimeout(run, 200));
    idle(() => {
      try {
        audio();
      } catch {
        // No audio device; cues stay silent.
      }
    });
  };
  for (const type of ["pointerdown", "keydown"]) window.addEventListener(type, warm, { once: true, capture: true });
}

export function playCompletion(preset: CompletionPreset) {
  try {
    const ctx = audio();
    const start = ctx.currentTime + 0.01;
    for (const [offset, frequency, duration, gain, wave = "sine"] of TONES[preset]) {
      const osc = ctx.createOscillator();
      const env = ctx.createGain();
      osc.type = wave;
      osc.frequency.value = frequency;
      const at = start + offset;
      env.gain.setValueAtTime(0.0001, at);
      env.gain.exponentialRampToValueAtTime(gain, at + 0.012);
      env.gain.exponentialRampToValueAtTime(0.0001, at + duration);
      osc.connect(env).connect(ctx.destination);
      osc.onended = () => {
        osc.disconnect();
        env.disconnect();
      };
      osc.start(at);
      osc.stop(at + duration + 0.02);
    }
  } catch {
    // Audio is optional; a blocked or missing output device must never break the page.
  }
}

const clips = new Map<string, HTMLAudioElement>();
function playClip(url: string) {
  try {
    let clip = clips.get(url);
    if (!clip) {
      clip = new Audio(url);
      clips.set(url, clip);
    }
    clip.currentTime = 0;
    void clip.play().catch(() => {});
  } catch {
    // As above: cues are a courtesy.
  }
}

export const playRequest = () => playClip(requestUrl);
export const playAttention = () => playClip(attentionUrl);
