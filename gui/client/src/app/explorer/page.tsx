"use client";

import { useEffect, useState, useCallback } from "react";
import { Navbar } from "@/components/layout/navbar";
import { Footer } from "@/components/layout/footer";
import { fetchProviders, fetchNodeModels, type DiscoveredProvider, type ModelEntry } from "@/lib/api/node";
import { Server, Cpu, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";

export default function ExplorerPage() {
  const [tab, setTab] = useState<"providers" | "models">("providers");
  const [providers, setProviders] = useState<DiscoveredProvider[]>([]);
  const [models, setModels] = useState<ModelEntry[]>([]);
  const [selectedProvider, setSelectedProvider] = useState<DiscoveredProvider | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);

  const loadData = useCallback(async () => {
    try {
      const [provs, mods] = await Promise.all([fetchProviders(), fetchNodeModels()]);
      setProviders(provs);
      setModels(mods);
      if (provs.length > 0) {
        setSelectedProvider((prev) => prev ?? provs[0]);
      }
    } catch {
      // keep existing state
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const [provs, mods] = await Promise.all([fetchProviders(), fetchNodeModels()]);
        if (active) {
          setProviders(provs);
          setModels(mods);
          if (provs.length > 0) {
            setSelectedProvider((prev) => prev ?? provs[0]);
          }
        }
      } catch {
        // ignore
      } finally {
        if (active) {
          setLoading(false);
          setRefreshing(false);
        }
      }
    })();

    return () => {
      active = false;
    };
  }, []);

  const handleRefresh = () => {
    setRefreshing(true);
    void loadData();
  };

  return (
    <div className="flex min-h-screen flex-col bg-background text-foreground">
      <Navbar />

      <main className="flex-1 container mx-auto max-w-6xl px-4 py-8 sm:px-6">
        <div className="flex flex-col sm:flex-row items-start sm:items-center justify-between gap-4 border-b border-border/40 pb-6 mb-8">
          <div>
            <h1 className="text-2xl sm:text-3xl font-bold tracking-tight">Provider & Model Explorer</h1>
            <p className="text-sm text-muted-foreground mt-1">
              Decentralized inference network directory: inspect verified Nostr compute providers and model catalogs.
            </p>
          </div>

          <div className="flex items-center gap-2">
            <div className="flex rounded-lg border border-border p-1 bg-card">
              <button
                onClick={() => setTab("providers")}
                className={`px-3 py-1 text-xs font-medium rounded-md transition-colors ${
                  tab === "providers" ? "bg-primary text-primary-foreground" : "text-muted-foreground hover:text-foreground"
                }`}
              >
                Providers ({providers.length})
              </button>
              <button
                onClick={() => setTab("models")}
                className={`px-3 py-1 text-xs font-medium rounded-md transition-colors ${
                  tab === "models" ? "bg-primary text-primary-foreground" : "text-muted-foreground hover:text-foreground"
                }`}
              >
                Models ({models.length})
              </button>
            </div>

            <Button
              variant="outline"
              size="sm"
              onClick={handleRefresh}
              disabled={refreshing}
              className="text-xs h-8"
            >
              <RefreshCw className={`w-3.5 h-3.5 mr-1.5 ${refreshing ? "animate-spin" : ""}`} />
              Refresh
            </Button>
          </div>
        </div>

        {loading ? (
          <div className="py-24 flex flex-col items-center justify-center gap-3 text-muted-foreground">
            <RefreshCw className="w-8 h-8 animate-spin text-primary" />
            <p className="text-sm">Querying verified provider registry...</p>
          </div>
        ) : tab === "providers" ? (
          <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
            {/* Providers list */}
            <div className="lg:col-span-2 space-y-4">
              {providers.length === 0 ? (
                <div className="p-8 border border-border/60 rounded-xl bg-card text-center text-muted-foreground space-y-3">
                  <Server className="w-10 h-10 mx-auto text-muted-foreground/60" />
                  <p className="text-sm font-medium">No external providers registered yet</p>
                  <p className="text-xs max-w-md mx-auto">
                    Nodes broadcasting Nostr Kind 31990 announcements will appear here once discovered and cryptographically verified.
                  </p>
                </div>
              ) : (
                providers.map((p) => {
                  const isSelected = selectedProvider?.pubkey === p.pubkey;
                  return (
                    <div
                      key={p.pubkey}
                      onClick={() => setSelectedProvider(p)}
                      className={`p-4 rounded-xl border transition-all cursor-pointer bg-card ${
                        isSelected
                          ? "border-primary/80 ring-1 ring-primary/50 shadow-sm"
                          : "border-border/60 hover:border-border"
                      }`}
                    >
                      <div className="flex items-start justify-between gap-3">
                        <div className="min-w-0">
                          <div className="flex items-center gap-2">
                            <span className="font-semibold text-base truncate">{p.name}</span>
                            <span
                              className={`text-[10px] px-2 py-0.5 rounded-full font-medium capitalize ${
                                p.status === "healthy" || p.status === "reachable"
                                  ? "bg-green-500/10 text-green-500 border border-green-500/20"
                                  : "bg-amber-500/10 text-amber-500 border border-amber-500/20"
                              }`}
                            >
                              {p.status}
                            </span>
                          </div>
                          <span className="font-mono text-xs text-muted-foreground block truncate mt-0.5">
                            {p.endpoint}
                          </span>
                        </div>

                        <div className="text-right shrink-0">
                          <span className="text-sm font-semibold text-primary">{p.pricing_sats} sats</span>
                          <span className="text-[11px] text-muted-foreground block uppercase font-mono">
                            {p.network}
                          </span>
                        </div>
                      </div>

                      <div className="mt-3 flex flex-wrap gap-1.5 items-center">
                        <span className="text-[11px] text-muted-foreground mr-1">Models:</span>
                        {p.models.map((m) => (
                          <span
                            key={m}
                            className="text-[11px] font-mono px-2 py-0.5 rounded bg-muted/60 text-foreground border border-border/40"
                          >
                            {m}
                          </span>
                        ))}
                      </div>
                    </div>
                  );
                })
              )}
            </div>

            {/* Provider detail panel */}
            <div className="lg:col-span-1">
              {selectedProvider ? (
                <div className="sticky top-24 p-5 rounded-xl border border-border/60 bg-card space-y-4">
                  <div className="border-b border-border/40 pb-3">
                    <h3 className="font-semibold text-lg">{selectedProvider.name}</h3>
                    <span className="text-xs text-muted-foreground font-mono block truncate">
                      {selectedProvider.endpoint}
                    </span>
                  </div>

                  <div className="space-y-3 text-xs">
                    <div>
                      <span className="text-muted-foreground block text-[11px]">Nostr Identity (Pubkey):</span>
                      <span className="font-mono break-all bg-muted/40 p-1.5 rounded block text-[11px]">
                        {selectedProvider.pubkey}
                      </span>
                    </div>

                    {selectedProvider.receipt_key_id && (
                      <div>
                        <span className="text-muted-foreground block text-[11px]">Receipt Signing Key:</span>
                        <span className="font-mono break-all bg-muted/40 p-1.5 rounded block text-[11px]">
                          {selectedProvider.receipt_key_id}
                        </span>
                      </div>
                    )}

                    <div className="grid grid-cols-2 gap-2 bg-muted/20 p-2.5 rounded-lg border border-border/30">
                      <div>
                        <span className="text-muted-foreground block text-[11px]">Status:</span>
                        <span className="font-medium capitalize text-green-500">{selectedProvider.status}</span>
                      </div>
                      <div>
                        <span className="text-muted-foreground block text-[11px]">Network:</span>
                        <span className="font-medium font-mono uppercase">{selectedProvider.network}</span>
                      </div>
                      <div>
                        <span className="text-muted-foreground block text-[11px]">Pricing:</span>
                        <span className="font-semibold text-primary">{selectedProvider.pricing_sats} sats</span>
                      </div>
                      <div>
                        <span className="text-muted-foreground block text-[11px]">Version:</span>
                        <span className="font-mono">{selectedProvider.version}</span>
                      </div>
                    </div>

                    <div>
                      <span className="text-muted-foreground block text-[11px] mb-1">Served Models:</span>
                      <div className="space-y-1">
                        {selectedProvider.models.map((m) => (
                          <div
                            key={m}
                            className="flex items-center justify-between bg-muted/30 px-2 py-1 rounded font-mono text-[11px]"
                          >
                            <span>{m}</span>
                            <span className="text-green-500 text-[10px]">Ready</span>
                          </div>
                        ))}
                      </div>
                    </div>
                  </div>
                </div>
              ) : (
                <div className="p-6 rounded-xl border border-dashed border-border/60 bg-card/40 text-center text-xs text-muted-foreground">
                  Select a provider to inspect verified metadata.
                </div>
              )}
            </div>
          </div>
        ) : (
          /* Models Tab */
          <div className="space-y-4">
            {models.length === 0 ? (
              <div className="p-8 border border-border/60 rounded-xl bg-card text-center text-muted-foreground space-y-2">
                <Cpu className="w-10 h-10 mx-auto text-muted-foreground/60" />
                <p className="text-sm font-medium">No models available in registry</p>
              </div>
            ) : (
              <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
                {models.map((m) => (
                  <div key={m.id} className="p-4 rounded-xl border border-border/60 bg-card space-y-2">
                    <div className="flex items-center justify-between">
                      <span className="font-semibold text-base font-mono">{m.id}</span>
                      <span className="text-[10px] px-2 py-0.5 rounded-full bg-green-500/10 text-green-500 border border-green-500/20 font-medium">
                        Available
                      </span>
                    </div>
                    <span className="text-xs text-muted-foreground block">
                      Owner: <span className="font-mono">{m.owned_by}</span>
                    </span>
                    <div className="pt-2 flex items-center justify-between text-xs border-t border-border/40">
                      <span className="text-muted-foreground">Object: {m.object}</span>
                      <span className="text-[11px] text-primary font-medium">Inference Ready</span>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </main>

      <Footer />
    </div>
  );
}
