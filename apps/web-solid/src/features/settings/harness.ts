// Static agent metadata from crates/protocol and crates/ui: names, marks, CLI commands and sign-in copy.

export interface HarnessDescriptor {
  id: string;
  name: string;
  installed?: boolean;
  canInstall?: boolean;
  enabled?: boolean | null;
  [key: string]: unknown;
}

export type UpdatePhase =
  | "dormant"
  | "checking"
  | "current"
  | "available"
  | "waiting-for-idle"
  | "preparing"
  | "downloading"
  | "installing"
  | "verifying"
  | "updated"
  | "manual-action-required"
  | "failed";

export type UpdatePolicy = "notify" | "auto-when-idle" | "off";

export interface HarnessUpdateStatus {
  harness: string;
  installedVersion?: string;
  latestVersion?: string;
  policy?: UpdatePolicy;
  phase: UpdatePhase;
  progress?: { completedBytes?: number; totalBytes?: number; message?: string };
  checkedAt?: number;
  error?: { message: string; retryable?: boolean };
  canApply?: boolean;
  manualCommand?: string;
}

export { harnessIcon as harnessMark, harnessTint } from "@/ui/harness";
export const isCustomAcp = (id: string) => id.startsWith("acp-");

const CLI: Record<string, string> = {
  "claude-code": "claude",
  codex: "codex",
  cursor: "cursor-agent",
  devin: "devin",
  grok: "grok",
  hermes: "hermes",
  pi: "pi",
  muse: "muse",
  opencode: "opencode",
  antigravity: "Antigravity",
  gemini: "gemini",
  kiro: "kiro-cli",
  zuse: "Zuse",
  mock: "mock",
};
export const cliName = (id: string) => CLI[id] ?? "configured ACP executable";

const INSTALL: Record<string, string> = {
  "claude-code": "curl -fsSL https://claude.ai/install.sh | bash",
  codex: "npm install -g @openai/codex",
  cursor: "curl https://cursor.com/install -fsS | bash",
  opencode: "npm install -g @opencode/cli",
  pi: "npm install -g --ignore-scripts @earendil-works/pi-coding-agent",
  grok: "npm install -g @xai-official/grok",
  hermes: "curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash",
  devin: "curl -fsSL https://cli.devin.ai/install.sh | bash",
  gemini: "npm install -g @google/gemini-cli",
  muse: "curl -fsSL https://dev.meta.ai/install.sh | bash",
};
export const installCommand = (id: string): string | undefined => INSTALL[id];

const LOGIN: Record<string, string> = {
  "claude-code": "claude /login",
  codex: "codex login",
  cursor: "cursor-agent login",
  gemini: "gemini /auth",
  grok: "grok login",
  devin: "devin auth login",
  opencode: "opencode auth login",
  pi: "pi",
  hermes: "hermes auth add",
  kiro: "kiro-cli login",
  muse: "muse login",
};
export const loginCommand = (id: string): string | undefined => LOGIN[id];

/** Agents with an Accounts section and an in-app sign-in, in display order. */
export const ACCOUNT_PROVIDERS: Record<string, string> = {
  "claude-code": "Claude Code",
  codex: "Codex",
  cursor: "Cursor",
  antigravity: "Antigravity",
  grok: "Grok",
  devin: "Devin",
  opencode: "OpenCode",
  pi: "Pi",
  hermes: "Hermes",
};
export const signsIn = (id: string) => id in ACCOUNT_PROVIDERS;
export const providerName = (id: string) => ACCOUNT_PROVIDERS[id] ?? "this provider";
export const reportsUsage = (id: string) => id !== "antigravity";
export const keepsOneLogin = (id: string) => id === "antigravity";
export const switchesAccounts = (id: string) => id !== "hermes" && id !== "antigravity";

export function providerNote(id: string): string | undefined {
  return id === "hermes"
    ? "Hermes manages its own credential pool and rotates through it. Accounts added here go through `hermes auth add`; remove one with `hermes auth remove`."
    : undefined;
}

export interface LoginOption {
  provider?: string;
  label: string;
}

export function loginOptions(id: string): LoginOption[] {
  switch (id) {
    case "opencode":
      return [
        { provider: "openai", label: "ChatGPT" },
        { provider: "github-copilot", label: "GitHub Copilot" },
      ];
    case "pi":
      return [{ provider: "openai-codex", label: "ChatGPT" }];
    case "hermes":
      return [
        { provider: "openai-codex", label: "ChatGPT" },
        { provider: "nous", label: "Nous Portal" },
      ];
    default:
      return signsIn(id) ? [{ label: providerName(id) }] : [];
  }
}

export function addOptionLabel(id: string, option: LoginOption, empty: boolean) {
  if (!option.provider) return empty ? `Connect a ${providerName(id)} account` : "Add account";
  return empty ? `Connect ${option.label}` : `Add ${option.label} account`;
}

export function loginCopy(id: string, provider?: string): string {
  const saved = "The new login is saved next to your current one — nothing changes until you switch.";
  switch (id) {
    case "claude-code":
      return `Finish signing in to Claude in your browser. ${saved}`;
    case "codex":
      return `Finish signing in to ChatGPT in your browser. ${saved}`;
    case "cursor":
      return "Finish signing in to Cursor in your browser. This mints a zuse-named API key you can revoke any time from Cursor's dashboard.";
    case "antigravity":
      return "Finish signing in to Google in your browser. Antigravity keeps one login on this device; if it is already signed in, this just confirms it.";
    case "grok":
      return `Finish signing in to Grok in your browser — approve the code shown below. ${saved}`;
    case "devin":
      return `Finish signing in to Devin in your browser. ${saved}`;
    case "opencode":
      if (provider === "github-copilot") return `Finish signing in to GitHub in your browser — enter the code shown below. ${saved}`;
      return "Finish signing in to ChatGPT in your browser. The agent gets its own login, saved next to any current one — nothing changes until you switch.";
    case "pi":
      return "Finish signing in to ChatGPT in your browser. The agent gets its own login, saved next to any current one — nothing changes until you switch.";
    case "hermes":
      return "Finish signing in in your browser — enter the code shown below. Hermes adds the login to its own credential pool and rotates through it itself.";
    default:
      return "Finish signing in in your browser.";
  }
}

/** Only web sign-in pages and loopback callbacks may be opened; never file:// or custom schemes. */
export function isOpenableLoginUrl(url: string) {
  try {
    const parsed = new URL(url);
    if (parsed.protocol === "https:") return true;
    return parsed.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(parsed.hostname);
  } catch {
    return false;
  }
}

export function installHint(id: string, enabled: boolean, canInstall: boolean): string {
  if (id === "antigravity") return canInstall ? "Install Antigravity to enable" : "Set ANTIGRAVITY_ACP_EXECUTABLE to enable Antigravity";
  const hint = enabled ? `${cliName(id)} CLI not installed — turn it off or install it` : `Install the ${cliName(id)} CLI to enable`;
  const command = installCommand(id);
  return !canInstall && command ? `${hint}. Install with \`${command}\`` : hint;
}

/** A descriptor's effective enabled flag; older engines omit it and follow detection. */
export const descriptorEnabled = (d: HarnessDescriptor) => d.enabled ?? (!!(d.installed ?? true) && d.id !== "mock");

export const UPDATE_POLICIES: { value: UpdatePolicy; label: string; description: string }[] = [
  { value: "notify", label: "Notify", description: "Install only when you choose Update." },
  { value: "auto-when-idle", label: "Auto when idle", description: "Install automatically after active runs finish." },
  { value: "off", label: "Off", description: "Don't check for new versions." },
];

export function updateDetail(status: HarnessUpdateStatus): string | undefined {
  const latest = status.latestVersion && status.installedVersion !== status.latestVersion ? status.latestVersion : undefined;
  switch (status.phase) {
    case "available":
      return latest ? `v${latest} available` : "Update available";
    case "waiting-for-idle":
      return "Waiting for agent to be idle";
    case "preparing":
      return "Preparing update…";
    case "downloading":
      return "Downloading…";
    case "installing":
      return "Installing…";
    case "verifying":
      return "Verifying…";
    case "updated":
      return "Updated";
    case "failed":
      return "Couldn't check for updates";
    default:
      return undefined;
  }
}

export const updateInProgress = (phase: UpdatePhase) =>
  phase === "waiting-for-idle" || phase === "preparing" || phase === "downloading" || phase === "installing" || phase === "verifying";

/** The engine applies an update the host can install itself, including one its check couldn't version. */
export function canStartUpdate(s: HarnessUpdateStatus) {
  return !!s.canApply && (s.phase === "available" || s.phase === "manual-action-required");
}

export function checkedLabel(checkedAt: number | undefined, now = Date.now()) {
  if (!checkedAt) return "Checked";
  const minutes = Math.floor(Math.max(0, now - checkedAt) / 60_000);
  if (minutes === 0) return "Checked just now";
  if (minutes < 60) return `Checked ${minutes}m ago`;
  if (minutes < 1440) return `Checked ${Math.floor(minutes / 60)}h ago`;
  return `Checked ${Math.floor(minutes / 1440)}d ago`;
}

export { maskedEmail } from "@/core/util";
