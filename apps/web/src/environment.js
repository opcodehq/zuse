// Presentation-only signals; no engine calls or per-frame polling.
export function observeEnvironment(report) {
  const motion = window.matchMedia('(prefers-reduced-motion: reduce)');
  let previous;
  const changed = () => {
    const value = JSON.stringify({environment: {
      reduced: motion.matches,
      visible: document.visibilityState !== 'hidden',
      focused: document.hasFocus(),
      languages: Array.from(navigator.languages),
    }});
    if (value !== previous) { previous = value; report(value); }
  };
  motion.addEventListener('change', changed);
  document.addEventListener('visibilitychange', changed);
  window.addEventListener('focus', changed);
  window.addEventListener('blur', changed);
  window.addEventListener('languagechange', () => {
    report(JSON.stringify({localization: {languages: Array.from(navigator.languages)}}));
  });
  window.addEventListener('storage', (event) => {
    if (event.key === 'zuse.language.v1' || event.key === null) {
      let preference = 'system';
      try { preference = localStorage.getItem('zuse.language.v1') || 'system'; } catch {}
      report(JSON.stringify({localization: {preference}}));
    }
  });
  let preference = 'system';
  try { preference = localStorage.getItem('zuse.language.v1') || 'system'; } catch {}
  report(JSON.stringify({localization: {preference, languages: Array.from(navigator.languages)}}));
  changed();
  // Software WebGPU keeps every pixel on the CPU; large windows can stall it.
  navigator.gpu?.requestAdapter({powerPreference: 'high-performance'}).then((adapter) => {
    if (adapter?.info?.isFallbackAdapter) report(JSON.stringify({softwareRenderer: true}));
  }).catch(() => {});
}
