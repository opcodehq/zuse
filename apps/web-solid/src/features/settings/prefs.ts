// Browser-local preferences owned by Settings, stored with the rest of view.preferences.
// extras/notifications reads sessionSounds, attentionSound, browserNotifications and notifyInBackgroundOnly too.
import { setView, view } from "@/core/view";

export const PRESETS = ["chime", "soft", "pop", "bell", "rise", "bloom"] as const;
export type CompletionPreset = (typeof PRESETS)[number];
export type ReduceMotion = "system" | "on" | "off";

export interface SettingsPrefs {
  /** Master switch for every session sound (desktop "Session sounds"). */
  sessionSounds: boolean;
  completionSound: boolean;
  completionPreset: CompletionPreset;
  inputSound: boolean;
  attentionSound: boolean;
  /** System notifications through the browser's Notification API. */
  browserNotifications: boolean;
  notifyInBackgroundOnly: boolean;
  /** Desktop's `agent_update_notifications`. */
  providerUpdateNotifications: boolean;
  reduceMotion: ReduceMotion;
}

const DEFAULTS: SettingsPrefs = {
  sessionSounds: true,
  completionSound: true,
  completionPreset: "chime",
  inputSound: true,
  attentionSound: true,
  browserNotifications: false,
  notifyInBackgroundOnly: true,
  providerUpdateNotifications: false,
  reduceMotion: "system",
};

const CHOICES: Partial<Record<keyof SettingsPrefs, readonly string[]>> = {
  completionPreset: PRESETS,
  reduceMotion: ["system", "on", "off"],
};

export function pref<K extends keyof SettingsPrefs>(key: K): SettingsPrefs[K] {
  const value = view.preferences[key];
  const choices = CHOICES[key];
  const valid = choices ? choices.includes(value as string) : typeof value === typeof DEFAULTS[key];
  return (valid ? value : DEFAULTS[key]) as SettingsPrefs[K];
}

export function setPref<K extends keyof SettingsPrefs>(key: K, value: SettingsPrefs[K]) {
  setView("preferences", key, value as never);
}

export const PRESET_LABELS: Record<CompletionPreset, string> = {
  chime: "Chime",
  soft: "Soft",
  pop: "Pop",
  bell: "Bell",
  rise: "Rise",
  bloom: "Bloom",
};

export const PERMISSION_MODES = [
  { value: "workspace-write", label: "Ask for approval", description: "Asks before restricted actions." },
  { value: "auto", label: "Approve for me", description: "Reviews actions, asks when needed." },
  { value: "danger-full-access", label: "Full access", description: "Runs actions without approval." },
] as const;
