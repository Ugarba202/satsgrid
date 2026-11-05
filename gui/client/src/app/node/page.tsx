"use client";

import { useEffect, useState } from "react";
import { Navbar } from "@/components/layout/navbar";
import { Card } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Activity, Cpu, Server, Network, Shield, Settings2, Users, Coins, Lock, KeyRound } from "lucide-react";
import { StatusBadge } from "@/components/satsgrid/status-badge";
import {
  fetchNodeHealth,
  fetchNodeModels,
  fetchNodeStats,
  fetchNodeConfig,
  NodeHealthResponse,
  ModelEntry,
  NodeStatsResponse,
  NodeConfigResponse,
} from "@/lib/api/node";

export default function NodeDashboardPage() {
  const [health, setHealth] = useState<NodeHealthResponse | null>(null);
  const [models, setModels] = useState<ModelEntry[]>([]);
  const [stats, setStats] = useState<NodeStatsResponse | null>(null);
  const [config, setConfig] = useState<NodeConfigResponse | null>(null);
  const [adminToken, setAdminToken] = useState("");
  const [adminInput, setAdminInput] = useState("");
  const [authError, setAuthError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(true);

  // Load public node health and models
  useEffect(() => {
    let isMounted = true;
    async function fetchPublicData() {
      try {
        const [healthRes, modelsRes] = await Promise.all([
          fetchNodeHealth().catch(() => null),
          fetchNodeModels().catch(() => []),
        ]);
        if (isMounted) {
          setHealth(healthRes);
          setModels(modelsRes);
        }
      } catch (e) {
        console.error("Failed to fetch public node data", e);
      } finally {
        if (isMounted) setIsLoading(false);
      }
    }

    if (typeof window !== "undefined") {
      const savedToken = localStorage.getItem("satsgrid_admin_token");
      if (savedToken) {
        setAdminToken(savedToken);
        setAdminInput(savedToken);
      }
    }

    fetchPublicData();
    return () => {
      isMounted = false;
    };
  }, []);

  // Load authenticated operator stats and configuration
  useEffect(() => {
    if (!adminToken) {
      setStats(null);
      setConfig(null);
      return;
    }

    let isMounted = true;
    async function fetchAdminData() {
      setAuthError(null);
      try {
        const [statsRes, configRes] = await Promise.all([
          fetchNodeStats(adminToken),
          fetchNodeConfig(adminToken),
        ]);
        if (isMounted) {
          setStats(statsRes);
          setConfig(configRes);
        }
      } catch (e: any) {
        if (isMounted) {
          setAuthError("Unauthorized or invalid admin token.");
          setStats(null);
          setConfig(null);
        }
      }
    }

    fetchAdminData();
    return () => {
      isMounted = false;
    };
  }, [adminToken]);

  const handleSaveToken = () => {
    const cleaned = adminInput.trim();
    setAdminToken(cleaned);
    if (typeof window !== "undefined") {
      if (cleaned) {
        localStorage.setItem("satsgrid_admin_token", cleaned);
      } else {
        localStorage.removeItem("satsgrid_admin_token");
      }
    }
  };

  return (
    <div className="flex flex-col min-h-screen bg-background">
      <Navbar />

      <main className="flex-1 p-6 md:p-12 max-w-7xl mx-auto w-full flex flex-col gap-8">
        <div className="flex flex-col md:flex-row md:items-end justify-between gap-4">
          <div>
            <h1 className="text-2xl font-bold mb-2 text-foreground">Node Dashboard</h1>
            <p className="text-muted-foreground">
              Real-time operational metrics and pricing for your SatsGrid inference node.
            </p>
          </div>

          {/* Admin Token Auth Bar */}
          <div className="flex flex-col gap-1.5 w-full md:w-auto">
            <div className="flex items-center gap-2">
              <input
                type="password"
                placeholder="Enter operator admin token..."
                value={adminInput}
                onChange={(e) => setAdminInput(e.target.value)}
                className="bg-card border border-input rounded-md px-3 py-1.5 text-xs font-mono text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-primary w-full md:w-64"
              />
              <Button size="sm" onClick={handleSaveToken} className="text-xs font-medium shrink-0">
                <KeyRound className="w-3.5 h-3.5 mr-1" />
                {adminToken ? "Update" : "Authenticate"}
              </Button>
            </div>
            {authError && <p className="text-[11px] text-destructive font-mono">{authError}</p>}
          </div>
        </div>

        {/* Top 4 Metrics Cards */}
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
          {/* Node Status */}
          <Card className="p-4 flex flex-col gap-2 bg-card">
            <div className="flex items-center gap-2 text-muted-foreground mb-2">
              <Activity className="w-4 h-4" />
              <span className="text-xs uppercase tracking-wider font-semibold">Node Status</span>
            </div>
            {isLoading ? (
              <div className="h-8 animate-pulse bg-muted rounded w-1/2"></div>
            ) : health ? (
              <div>
                <StatusBadge variant="success">Online</StatusBadge>
                <div className="text-xs text-muted-foreground mt-2 font-mono">
                  {health.service} v{health.version}
                </div>
              </div>
            ) : (
              <div>
                <StatusBadge variant="error">Offline</StatusBadge>
                <div className="text-xs text-muted-foreground mt-2 font-mono">
                  Backend unreachable
                </div>
              </div>
            )}
          </Card>

          {/* Active Models */}
          <Card className="p-4 flex flex-col gap-2 bg-card">
            <div className="flex items-center gap-2 text-muted-foreground mb-2">
              <Cpu className="w-4 h-4" />
              <span className="text-xs uppercase tracking-wider font-semibold">Hosted Models</span>
            </div>
            {isLoading ? (
              <div className="h-8 animate-pulse bg-muted rounded w-1/2"></div>
            ) : (
              <div>
                <span className="text-2xl font-bold">{models.length}</span>
                <div className="text-xs text-muted-foreground mt-1">
                  {models.length > 0 ? "Ready for inference" : "No models detected in Ollama"}
                </div>
              </div>
            )}
          </Card>

          {/* Active Sessions */}
          <Card className="p-4 flex flex-col gap-2 bg-card">
            <div className="flex items-center gap-2 text-muted-foreground mb-2">
              <Users className="w-4 h-4" />
              <span className="text-xs uppercase tracking-wider font-semibold">Active Sessions</span>
            </div>
            {stats ? (
              <div>
                <span className="text-2xl font-bold">{stats.active_sessions}</span>
                <div className="text-xs text-muted-foreground mt-1">
                  {stats.total_requests} total requests served
                </div>
              </div>
            ) : (
              <div className="flex items-center gap-1.5 text-muted-foreground py-2 text-xs">
                <Lock className="w-3.5 h-3.5" />
                <span>Admin token required</span>
              </div>
            )}
          </Card>

          {/* Sats Earned / Lightning Backend */}
          <Card className="p-4 flex flex-col gap-2 bg-card">
            <div className="flex items-center gap-2 text-muted-foreground mb-2">
              <Coins className="w-4 h-4 text-lightning" />
              <span className="text-xs uppercase tracking-wider font-semibold">Total Revenue</span>
            </div>
            {stats ? (
              <div>
                <span className="text-2xl font-bold text-lightning">{stats.total_sats_earned} SATS</span>
                <div className="text-xs text-muted-foreground mt-1">
                  Settled via {config?.lightning.backend.toUpperCase() || "Lightning"}
                </div>
              </div>
            ) : (
              <div className="flex items-center gap-1.5 text-muted-foreground py-2 text-xs">
                <Lock className="w-3.5 h-3.5" />
                <span>Admin token required</span>
              </div>
            )}
          </Card>
        </div>

        {/* Detailed Grid */}
        <div className="grid grid-cols-1 lg:grid-cols-3 gap-8">
          <div className="lg:col-span-2 flex flex-col gap-6">
            {/* Hosted Models List */}
            <Card className="p-6 bg-card border border-border">
              <h2 className="text-lg font-semibold mb-6 flex items-center gap-2">
                <Server className="w-5 h-5 text-primary" />
                Hosted Models & Pricing
              </h2>

              <div className="flex flex-col gap-4">
                {isLoading ? (
                  <div className="h-16 animate-pulse bg-muted rounded-xl"></div>
                ) : models.length > 0 ? (
                  models.map((m) => (
                    <div
                      key={m.id}
                      className="flex items-center justify-between p-4 rounded-xl border border-border/50 bg-background"
                    >
                      <div className="flex flex-col gap-1">
                        <span className="font-semibold text-sm">{m.id}</span>
                        <span className="text-xs text-muted-foreground">Provider: {m.owned_by}</span>
                      </div>
                      <div className="flex flex-col items-end gap-1">
                        <span className="text-sm font-medium text-lightning font-mono">
                          {config?.pricing?.flat_rate_sats != null
                            ? `${config.pricing.flat_rate_sats} sats / req`
                            : "Per-node pricing"}
                        </span>
                        <StatusBadge variant="success">Available</StatusBadge>
                      </div>
                    </div>
                  ))
                ) : (
                  <div className="p-8 text-center text-muted-foreground border border-dashed border-border rounded-xl">
                    No models currently reported by the upstream engine (Ollama/vLLM).
                  </div>
                )}
              </div>
            </Card>

            {/* Live Node Configuration */}
            <Card className="p-6 bg-card border border-border">
              <h2 className="text-lg font-semibold mb-6 flex items-center gap-2">
                <Settings2 className="w-5 h-5 text-primary" />
                Node Infrastructure Configuration
              </h2>
              {config ? (
                <div className="grid grid-cols-1 md:grid-cols-2 gap-x-12 gap-y-6">
                  <div className="flex flex-col gap-1 border-b border-border/40 pb-4">
                    <span className="text-xs text-muted-foreground uppercase tracking-wider">Host</span>
                    <span className="text-sm font-mono text-foreground">
                      {config.server.host}:{config.server.port}
                    </span>
                  </div>
                  <div className="flex flex-col gap-1 border-b border-border/40 pb-4">
                    <span className="text-xs text-muted-foreground uppercase tracking-wider">
                      Lightning Backend
                    </span>
                    <span className="text-sm font-mono text-success uppercase">
                      {config.lightning.backend}
                    </span>
                  </div>
                  <div className="flex flex-col gap-1 border-b border-border/40 pb-4">
                    <span className="text-xs text-muted-foreground uppercase tracking-wider">
                      Bitcoin Network
                    </span>
                    <span className="text-sm font-mono text-foreground uppercase">
                      {config.lightning.network}
                    </span>
                  </div>
                  <div className="flex flex-col gap-1 border-b border-border/40 pb-4">
                    <span className="text-xs text-muted-foreground uppercase tracking-wider">
                      Flat Rate Pricing
                    </span>
                    <span className="text-sm font-mono text-lightning">
                      {config.pricing?.flat_rate_sats ?? 0} sats / call
                    </span>
                  </div>
                </div>
              ) : (
                <div className="p-8 text-center text-muted-foreground border border-dashed border-border rounded-xl flex flex-col items-center gap-2">
                  <Lock className="w-6 h-6 text-muted-foreground/60" />
                  <p className="text-sm">
                    Enter your node operator admin token above to view live infrastructure parameters.
                  </p>
                </div>
              )}
            </Card>
          </div>

          {/* Security & Cryptographic Gate Info */}
          <div className="flex flex-col gap-6">
            <Card className="p-6 bg-card border border-border flex flex-col gap-6">
              <h2 className="text-lg font-semibold flex items-center gap-2">
                <Shield className="w-5 h-5 text-primary" />
                Cryptographic Gate
              </h2>

              <div className="flex items-start gap-4">
                <div className="w-8 h-8 rounded-full bg-success/20 flex items-center justify-center shrink-0">
                  <Shield className="w-4 h-4 text-success" />
                </div>
                <div className="flex flex-col gap-1">
                  <span className="text-sm font-medium">L402 Gate Active</span>
                  <p className="text-xs text-muted-foreground leading-relaxed">
                    Unauthenticated requests receive HTTP 402 with BOLT11 payment challenges. Inference
                    is unlocked solely upon valid cryptographic preimage verification.
                  </p>
                </div>
              </div>

              <div className="flex items-start gap-4">
                <div className="w-8 h-8 rounded-full bg-primary/20 flex items-center justify-center shrink-0">
                  <Network className="w-4 h-4 text-primary" />
                </div>
                <div className="flex flex-col gap-1">
                  <span className="text-sm font-medium">NWC & WebLN Settlement</span>
                  <p className="text-xs text-muted-foreground leading-relaxed">
                    Supports instant invoice settlement via Nostr Wallet Connect (NIP-47) relays and
                    WebLN browser extensions.
                  </p>
                </div>
              </div>

              <div className="flex items-start gap-4">
                <div className="w-8 h-8 rounded-full bg-lightning/20 flex items-center justify-center shrink-0">
                  <Coins className="w-4 h-4 text-lightning" />
                </div>
                <div className="flex flex-col gap-1">
                  <span className="text-sm font-medium">Atomic Budgeting</span>
                  <p className="text-xs text-muted-foreground leading-relaxed">
                    Session budgets are debited in real time on every token generation chunk with
                    cryptographic macaroon attenuation.
                  </p>
                </div>
              </div>
            </Card>
          </div>
        </div>
      </main>
    </div>
  );
}
