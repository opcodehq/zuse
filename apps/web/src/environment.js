// Presentation-only signals; no engine calls or per-frame polling.
export function observeEnvironment(report) {
  const motion = window.matchMedia('(prefers-reduced-motion: reduce)');
  let previous;
  const changed = () => {
    const value = JSON.stringify({environment: {
      reduced: motion.matches,
      visible: document.visibilityState !== 'hidden',
      focused: document.hasFocus(),
    }});
    if (value !== previous) { previous = value; report(value); }
  };
  motion.addEventListener('change', changed);
  document.addEventListener('visibilitychange', changed);
  window.addEventListener('focus', changed);
  window.addEventListener('blur', changed);
  changed();
}
