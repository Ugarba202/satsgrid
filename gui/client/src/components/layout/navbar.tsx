"use client";

import { useState, useEffect } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import dynamic from "next/dynamic";
import { Zap, Server, User, ExternalLink, HardDrive } from "lucide-react";
import { Button } from "@/components/ui/button";
import { fetchNodeHealth, type NodeLightningInfo } from "@/lib/api/node";
import { resolveNetworkIdentity } from "@/lib/api/network";

const ConnectButton = dynamic(
  () => import("@getalby/bitcoin-connect-react").then((mod) => mod.Button),
  { ssr: false }
);

export function Navbar() {
  const pathname = usePathname();
  const isOperatorView = pathname.startsWith("/node");
  const [network, setNetwork] = useState<"regtest" | "testnet" | "mainnet">("regtest");
  const [nodeLightning, setNodeLightning] = useState<NodeLightningInfo | null>(null);

  useEffect(() => {
    import("@getalby/bitcoin-connect").then(({ init }) => {
      init({ appName: "SatsGrid" });
    });

    fetchNodeHealth()
      .then((health) => {
        if (health.lightning) {
          setNodeLightning(health.lightning);
          const net = health.lightning.network.toLowerCase();
          if (net === "mainnet" || net === "testnet" || net === "regtest") {
            setNetwork(net);
          }
        }
      })
      .catch(() => {
        // Node metadata unavailable; keep default
      });
  }, []);

  const authoritativeNet = resolveNetworkIdentity(nodeLightning);

  return (
    <header className="sticky top-0 z-50 w-full border-b border-border/40 bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/60">
      <div className="container mx-auto flex h-16 max-w-6xl items-center justify-between px-4 sm:px-6">
        {/* Brand & Portal Mode Badge */}
        <div className="flex items-center gap-3">
          <Link href="/" className="flex items-center gap-2 transition-opacity hover:opacity-80">
            <div className={`p-1.5 rounded-md ${isOperatorView ? "bg-warning/10 text-warning" : "bg-primary/10 text-primary"}`}>
              <Zap className="h-5 w-5" />
            </div>
            <span className="font-semibold tracking-tight text-lg">SATSGRID</span>
          </Link>

          {isOperatorView ? (
            <span className="hidden sm:inline-flex items-center gap-1 px-2 py-0.5 rounded text-[10px] font-mono font-semibold uppercase tracking-wider bg-warning/10 text-warning border border-warning/30">
              <Server className="w-3 h-3" /> Operator Console
            </span>
          ) : (
            <span className="hidden sm:inline-flex items-center gap-1 px-2 py-0.5 rounded text-[10px] font-mono font-semibold uppercase tracking-wider bg-primary/10 text-primary border border-primary/30">
              <User className="w-3 h-3" /> Caller Portal
            </span>
          )}
        </div>
        
        {/* Role-Specific Navigation */}
        <nav className="hidden md:flex items-center gap-6 text-sm font-medium text-muted-foreground">
          {isOperatorView ? (
            <>
              <Link
                href="/node"
                className={`transition-colors ${pathname === "/node" ? "text-foreground font-semibold" : "hover:text-foreground"}`}
              >
                Node Dashboard
              </Link>
              <Link
                href="https://github.com/Ugarba202/satsgrid#readme"
                target="_blank"
                className="hover:text-foreground transition-colors flex items-center gap-1 text-xs text-muted-foreground"
              >
                Operator Docs <ExternalLink className="w-3 h-3" />
              </Link>
            </>
          ) : (
            <>
              <Link
                href="/playground"
                className={`transition-colors ${pathname === "/playground" ? "text-foreground font-semibold" : "hover:text-foreground"}`}
              >
                Playground
              </Link>
              <Link
                href="/sessions"
                className={`transition-colors ${pathname === "/sessions" ? "text-foreground font-semibold" : "hover:text-foreground"}`}
              >
                My Sessions
              </Link>
              <Link
                href="https://github.com/Ugarba202/satsgrid#readme"
                target="_blank"
                className="hover:text-foreground transition-colors flex items-center gap-1 text-xs text-muted-foreground"
              >
                Docs <ExternalLink className="w-3 h-3" />
              </Link>
            </>
          )}
        </nav>

        {/* Right Action Bar */}
        <div className="flex items-center gap-3">
          {isOperatorView ? (
            /* Operator Controls: Port status and Switch to Caller button */
            <div className="flex items-center gap-3">
              <div className="hidden sm:flex items-center gap-1.5 px-2.5 py-1 rounded-full border border-border/60 bg-card text-xs font-mono text-muted-foreground">
                <HardDrive className="w-3 h-3 text-warning" />
                <span>Port 8080</span>
              </div>

              <Link href="/playground">
                <Button variant="outline" size="sm" className="font-mono text-xs gap-1.5 border-border hover:bg-card hover:border-primary/50 text-foreground">
                  <User className="w-3.5 h-3.5 text-primary" />
                  <span className="hidden sm:inline">Switch to</span> Caller Portal ↗
                </Button>
              </Link>
            </div>
          ) : (
            /* Caller Controls: Network selector, WebLN Connect button, and Switch to Operator button */
            <>
              <div className="flex items-center gap-1.5 px-2.5 py-1 rounded-full border border-border/60 bg-card text-xs font-mono shadow-sm">
                <div
                  className={`w-2 h-2 rounded-full ${
                    nodeLightning?.backend === "mock"
                      ? "bg-muted-foreground"
                      : network === "mainnet"
                      ? "bg-lightning"
                      : network === "testnet"
                      ? "bg-primary"
                      : "bg-success"
                  }`}
                />
                {nodeLightning ? (
                  <span
                    className="text-xs font-mono font-medium text-muted-foreground"
                    title={`Authoritative Node Backend: ${nodeLightning.backend}, Network: ${nodeLightning.network}`}
                  >
                    {authoritativeNet.displayName}
                  </span>
                ) : (
                  <select
                    value={network}
                    onChange={(e) => setNetwork(e.target.value as "regtest" | "testnet" | "mainnet")}
                    className="bg-transparent text-xs font-mono font-medium focus:outline-none cursor-pointer text-muted-foreground hover:text-foreground"
                  >
                    <option value="regtest" className="bg-popover text-popover-foreground">
                      Regtest (Polar)
                    </option>
                    <option value="testnet" className="bg-popover text-popover-foreground">
                      Testnet / Signet
                    </option>
                    <option value="mainnet" className="bg-popover text-popover-foreground">
                      Bitcoin Mainnet
                    </option>
                  </select>
                )}
              </div>

              <ConnectButton />
              
              <Link href="/node">
                <Button
                  variant="ghost"
                  size="sm"
                  className="hidden md:flex font-mono text-xs gap-1.5 text-muted-foreground hover:text-foreground border border-border/40 hover:border-warning/50 hover:bg-warning/10"
                  title="Switch to Operator Dashboard"
                >
                  <Server className="w-3.5 h-3.5 text-warning" />
                  Operator Console
                </Button>
              </Link>
            </>
          )}
        </div>
      </div>
    </header>
  );
}
