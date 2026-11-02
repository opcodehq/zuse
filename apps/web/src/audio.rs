//! Web Audio is a local presentation action, independent of engine connection.
use wasm_bindgen::prelude::*;
use zuse_protocol::completion_sound::CompletionSoundPreset;
#[wasm_bindgen(inline_js = r#"
let completionContext;
function context() {
  if (!completionContext && globalThis.AudioContext) completionContext = new AudioContext();
  return completionContext;
}
export function installCompletionAudio() {
  const prepare = () => { try { const ctx = context(); if (ctx?.state === 'suspended') void ctx.resume().catch(() => {}); } catch {} };
  document.addEventListener('pointerdown', prepare, {passive: true});
  document.addEventListener('keydown', prepare, {passive: true});
}
export function playCompletionAudio(serialized) {
  void (async () => {
    try {
      const ctx = context(); if (!ctx) return;
      if (ctx.state === 'suspended') await ctx.resume();
      const now = ctx.currentTime;
      for (const note of JSON.parse(serialized)) {
        const oscillator = ctx.createOscillator(); const envelope = ctx.createGain();
        const start = now + note.offset;
        oscillator.type = note.wave; oscillator.frequency.setValueAtTime(note.frequency, start);
        envelope.gain.setValueAtTime(0.0001, start);
        envelope.gain.exponentialRampToValueAtTime(note.gain, start + 0.012);
        envelope.gain.exponentialRampToValueAtTime(0.0001, start + note.duration);
        oscillator.connect(envelope); envelope.connect(ctx.destination);
        oscillator.onended = () => { oscillator.disconnect(); envelope.disconnect(); };
        oscillator.start(start); oscillator.stop(start + note.duration + 0.02);
      }
    } catch {}
  })();
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = installCompletionAudio)]
    pub fn install();
    #[wasm_bindgen(js_name = playCompletionAudio)]
    fn play_notes(serialized: &str);
}
pub fn play(preset: CompletionSoundPreset) {
    if let Ok(notes) = serde_json::to_string(preset.tones()) {
        play_notes(&notes);
    }
}
