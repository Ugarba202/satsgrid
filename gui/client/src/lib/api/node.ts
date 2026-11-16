export interface NodeLightningInfo {
  backend: string; // "mock" | "lnd_rest" | "nwc"
  network: string; // "mainnet" | "testnet" | "regtest" | "signet"
}

export interface NodeHealthResponse {
  status: string;
  service: string;
  version?: string;
  lightning?: NodeLightningInfo;
}

export interface ModelEntry {
  id: string;
  object: string;
  created: number;
  owned_by: string;
}

export interface NodeStatsResponse {
  total_requests: number;
  active_sessions: number;
  total_sats_earned: number;
}

export interface NodeConfigResponse {
  server: {
    host: string;
    port: number;
  };
  lightning: {
    backend: string;
    network: string;
  };
  pricing?: {
    flat_rate_sats?: number;
    prompt_rate_sats_per_1k?: number;
    completion_rate_sats_per_1k?: number;
  };
}

export async function fetchNodeHealth(): Promise<NodeHealthResponse> {
  const res = await fetch("/health", { cache: "no-store" });
  if (!res.ok) {
    throw new Error(`Node health failed: ${res.status}`);
  }
  return res.json();
}

export async function fetchNodeModels(): Promise<ModelEntry[]> {
  const res = await fetch("/v1/models", { cache: "no-store" });
  if (!res.ok) {
    throw new Error(`Node models failed: ${res.status}`);
  }
  const data = await res.json();
  return Array.isArray(data.data) ? data.data : [];
}

export async function fetchNodeStats(adminToken: string): Promise<NodeStatsResponse> {
  const headers: Record<string, string> = {
    Authorization: `Bearer ${adminToken}`,
  };
  const res = await fetch("/v1/node/stats", { headers, cache: "no-store" });
  if (!res.ok) {
    throw new Error(`Node stats failed: ${res.status}`);
  }
  return res.json();
}

export async function fetchNodeConfig(adminToken: string): Promise<NodeConfigResponse> {
  const headers: Record<string, string> = {
    Authorization: `Bearer ${adminToken}`,
  };
  const res = await fetch("/v1/node/config", { headers, cache: "no-store" });
  if (!res.ok) {
    throw new Error(`Node config failed: ${res.status}`);
  }
  return res.json();
}

export async function updateNodePricing(
  adminToken: string,
  pricing: NonNullable<NodeConfigResponse["pricing"]>
): Promise<{ status: string }> {
  const headers: Record<string, string> = {
    Authorization: `Bearer ${adminToken}`,
    "Content-Type": "application/json",
  };
  const res = await fetch("/v1/node/config", {
    method: "POST",
    headers,
    body: JSON.stringify(pricing),
  });
  if (!res.ok) {
    throw new Error(`Update pricing failed: ${res.status}`);
  }
  return res.json();
}
