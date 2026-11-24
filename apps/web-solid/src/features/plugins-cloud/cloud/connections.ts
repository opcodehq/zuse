export const connectionProviders = { claude: "Claude", codex: "Codex", grok: "Grok" } as const;
export type ConnectionAgent = keyof typeof connectionProviders;
export const connectionMethods = { "device-code": "Device sign-in", "setup-token": "Setup token", "api-key": "API key" } as const;
export type ConnectionMethod = keyof typeof connectionMethods;

export interface LoginOperation {
  operationId: string;
  connectionId: string;
  agent: ConnectionAgent;
  state: "queued" | "authorizing" | "connected" | "expired" | "cancelled" | "error";
  verificationUrl?: string | null;
  verificationCode?: string | null;
  errorCode?: string | null;
}

export interface AgentConnection {
  connectionId: string;
  agent: ConnectionAgent;
  method: ConnectionMethod;
  state: "pending" | "connected" | "reconnect-required" | "revoked" | "error";
  accountLabel?: string | null;
  alias?: string | null;
  enabled?: boolean;
  position?: number;
  errorCode?: string | null;
  credentialUpdate?: LoginOperation | null;
  pendingLoginOperationId?: string | null;
  connectionScope?: "personal" | "organization";
}

export interface ConnectionList {
  connections: AgentConnection[];
  personalConnections?: AgentConnection[];
  supportsDeviceReconnect?: boolean;
  supportsCredentialReconfiguration?: boolean;
}

export const connectionName = (row: AgentConnection) =>
  row.alias || row.accountLabel || connectionProviders[row.agent];

export const orderedConnections = (rows: AgentConnection[]) =>
  rows.filter((row) => row.state !== "revoked").sort((a, b) => (a.position ?? Number.MAX_SAFE_INTEGER) - (b.position ?? Number.MAX_SAFE_INTEGER));

export const launchConnections = (rows: AgentConnection[], harness: string | undefined) =>
  orderedConnections(rows).filter((row) =>
    row.agent === (harness === "claude-code" ? "claude" : harness)
    && row.connectionScope !== "organization" && row.enabled !== false && row.state === "connected",
  );

export const operationActive = (operation: LoginOperation | null | undefined) =>
  operation?.state === "queued" || operation?.state === "authorizing";
