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

export interface ModelPricing {
  flat_rate_sats?: number | null;
  sats_per_prompt_token?: number | null;
  sats_per_completion_token?: number | null;
}

export interface ModelEntry {
  id: string;
  object: string;
  created: number;
  owned_by: string;
  description?: string | null;
  context_length?: number | null;
  pricing?: ModelPricing | null;
}

export interface ReconciliationWorkerStats {
  last_scan_timestamp: number | null;
  scans_completed: number;
  invoices_checked: number;
  invoices_settled: number;
  invoices_expired: number;
  items_reconciled: number;
  last_error: string | null;
}

export interface ModelActivityItem {
  model: string;
  completed_requests: number;
  sats_earned: number;
}

export interface RecentRequestItem {
  id: string;
  model: string;
  status: string;
  charged_sats: number;
  created_at: number;
}

export interface NodeStatsResponse {
  total_requests: number;
  active_sessions?: number;
  total_sats_earned: number;
  reserved_sats?: number;
  requests_breakdown?: {
    completed: number;
    failed: number;
    reconciliation_required: number;
  };
  model_breakdown?: ModelActivityItem[];
  recent_requests?: RecentRequestItem[];
  invoices?: {
    pending: number;
    settled: number;
    expired?: number;
    cancelled?: number;
  };
  reconciliation?: {
    unresolved_items: number;
    worker?: ReconciliationWorkerStats | null;
  };
}

export interface ReconciliationItem {
  id: string;
  item_type: "invoice" | "inference_request" | "session_late_funding";
  reference_id: string;
  payment_hash?: string | null;
  session_id?: string | null;
  status: "unresolved" | "in_progress" | "resolved" | "ignored";
  reason: string;
  error_details?: string | null;
  attempt_count: number;
  first_detected_at: number;
  last_attempt_at?: number | null;
  next_retry_at?: number | null;
  resolved_at?: number | null;
  reserved_sats?: number | null;
}

export interface ReconciliationListResponse {
  items: ReconciliationItem[];
  count: number;
}


export interface NodeConfigResponse {
  server?: {
    host: string;
    port: number;
  };
  lightning?: {
    backend: string;
    network: string;
  };
  pricing?: {
    default_price_sats?: number;
    flat_rate_sats?: number;
    prompt_rate_sats_per_1k?: number;
    completion_rate_sats_per_1k?: number;
  };
  upstream?: {
    url: string;
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

export interface DiscoveredProvider {
  pubkey: string;
  name: string;
  endpoint: string;
  models: string[];
  pricing_sats: number;
  network: string;
  version: string;
  receipt_key_id?: string | null;
  status: "discovered" | "reachable" | "healthy" | "offline" | "error";
  last_seen_at: number;
  last_checked_at?: number | null;
  last_error?: string | null;
}

export interface ProviderListResponse {
  providers: DiscoveredProvider[];
  count: number;
}

export async function fetchProviders(): Promise<DiscoveredProvider[]> {
  try {
    const res = await fetch("/v1/providers", { cache: "no-store" });
    if (!res.ok) return [];
    const data: ProviderListResponse = await res.json();
    return data.providers || [];
  } catch {
    return [];
  }
}

export async function fetchProvider(pubkey: string): Promise<DiscoveredProvider | null> {
  try {
    const res = await fetch(`/v1/providers/${encodeURIComponent(pubkey)}`, { cache: "no-store" });
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
}

export async function fetchReconciliationItems(
  adminToken: string,
  status?: string,
  limit?: number
): Promise<ReconciliationListResponse> {
  const headers: Record<string, string> = {
    Authorization: `Bearer ${adminToken}`,
  };
  const params = new URLSearchParams();
  if (status) params.append("status", status);
  if (limit) params.append("limit", limit.toString());

  const url = `/v1/node/reconciliation${params.toString() ? `?${params.toString()}` : ""}`;
  const res = await fetch(url, { headers, cache: "no-store" });
  if (!res.ok) {
    throw new Error(`Fetch reconciliation items failed: ${res.status}`);
  }
  return res.json();
}

