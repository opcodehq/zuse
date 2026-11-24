export function percentLeft(usedFraction: number | undefined) {
  if (usedFraction === undefined || !Number.isFinite(usedFraction)) return null;
  return Math.round((1 - Math.min(1, Math.max(0, usedFraction))) * 100);
}

export function usageResetLabel(resetsAt: number, now: number) {
  if (!Number.isFinite(resetsAt)) return "Reset unavailable";
  if (resetsAt <= now) return "Reset time passed";
  const minutes = Math.floor((resetsAt - now) / 60_000);
  if (minutes >= 1440) return `Resets in ${Math.floor(minutes / 1440)}d ${Math.floor(minutes / 60) % 24}h`;
  if (minutes >= 60) return `Resets in ${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, "0")}m`;
  return minutes ? `Resets in ${minutes}m` : "Resets in <1m";
}
