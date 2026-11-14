import { useState, useEffect, type FormEvent } from "react";
import Link from "next/link";
import { SatsAmount } from "@/components/satsgrid/sats-amount";
import { StatusBadge } from "@/components/satsgrid/status-badge";
import { NodeStatus } from "@/components/satsgrid/node-status";
import { Card } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { Button } from "@/components/ui/button";
import { ShieldCheck, Zap, Activity, ChevronDown, ChevronRight, Cpu, AlertTriangle, Coins, PlusCircle } from "lucide-react";
import { SessionStatus, SessionData } from "@/types/session";
import { fetchNodeHealth } from "@/lib/api/node";
import { parseMacaroonCaveats } from "@/lib/api/client";
import { developmentRecoveryEnabled, validateRecoveryCredentials, type RecoveryCredentials } from "@/lib/session-lifecycle";

interface ProtocolPanelProps {
  sessionStatus: SessionStatus;
  sessionData: SessionData;
  balanceSyncStatus?: "syncing" | "confirmed" | "unavailable";
  errorMessage?: string | null;
  onBudgetChange?: (budget: number) => void;
  onStartSession: () => void;
  onResetSession?: () => void;
  onRecoverSession?: (credentials: RecoveryCredentials) => boolean;
}

export function ProtocolPanel({
  sessionStatus,
  sessionData,
  balanceSyncStatus = "confirmed",
  errorMessage,
  onBudgetChange,
  onStartSession,
  onResetSession,
  onRecoverSession,
}: ProtocolPanelProps) {
  const [l402Expanded, setL402Expanded] = useState(false);
  const [isNodeOnline, setIsNodeOnline] = useState<boolean | null>(null);
  const [recoveryOpen, setRecoveryOpen] = useState(false);
  const [recoveryError, setRecoveryError] = useState<string | null>(null);
  const [recoveryInput, setRecoveryInput] = useState({ sessionId: "", invoice: "", macaroon: "", preimage: "" });
  const canRecover = developmentRecoveryEnabled(process.env.NODE_ENV);

  const handleRecoverySubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setRecoveryError(null);
    const caveats = parseMacaroonCaveats(recoveryInput.macaroon.trim());
    const credentials: RecoveryCredentials = {
      sessionId: recoveryInput.sessionId.trim(),
      invoice: recoveryInput.invoice.trim(),
      macaroon: recoveryInput.macaroon.trim(),
      preimage: recoveryInput.preimage.trim(),
      budgetSats: Number(caveats.budget),
      capability: caveats.capability || "",
    };
    const macaroonSession = caveats.session ?? caveats.session_id;
    if (!validateRecoveryCredentials(credentials) || macaroonSession !== credentials.sessionId) {
      setRecoveryError("The session details or credentials do not match a valid inference capability.");
      return;
    }
    if (onRecoverSession?.(credentials)) {
      setRecoveryInput({ sessionId: "", invoice: "", macaroon: "", preimage: "" });
      setRecoveryOpen(false);
    } else {
      setRecoveryError("The session could not be restored. Existing session data was preserved.");
    }
  };

  useEffect(() => {
    let isMounted = true;
    async function checkHealth() {
      try {
        await fetchNodeHealth();
        if (isMounted) setIsNodeOnline(true);
      } catch {
        if (isMounted) setIsNodeOnline(false);
      }
    }
    checkHealth();
    const interval = setInterval(checkHealth, 10000);
    return () => {
      isMounted = false;
      clearInterval(interval);
    };
  }, []);

  return (
    <div className="flex flex-col h-full">
      <div className="h-14 border-b border-border/40 flex items-center px-4 bg-card/50 sticky top-0 z-10">
        <h2 className="font-semibold text-sm">Protocol Activity</h2>
      </div>

      <div className="p-4 flex flex-col gap-6">
        {errorMessage && !["idle", "creating", "error"].includes(sessionStatus) && (
          <div role="alert" className="p-3 rounded-lg bg-destructive/10 border border-destructive/20 text-xs text-destructive break-words">
            {errorMessage}
          </div>
        )}
        {/* Node Status */}
        <NodeStatus isOnline={isNodeOnline ?? true} address="127.0.0.1:8080" />

        {/* Session Card */}
        <Card className="p-4 flex flex-col gap-4 bg-background">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2">
              <Activity className="w-4 h-4 text-primary" />
              <h3 className="font-medium text-sm">Active Session</h3>
            </div>
            {sessionStatus === "active" ? (
              <StatusBadge variant="success">Ready</StatusBadge>
            ) : sessionStatus === "payment_required" ? (
              <StatusBadge variant="warning">402 Pending</StatusBadge>
            ) : sessionStatus === "payment_submitted" || sessionStatus === "authorizing" ? (
              <StatusBadge variant="warning">{sessionStatus === "authorizing" ? "Verifying" : "Payment submitted"}</StatusBadge>
            ) : sessionStatus === "invalid_payment" ? (
              <StatusBadge variant="error">Invalid proof</StatusBadge>
            ) : sessionStatus === "exhausted" ? (
              <StatusBadge variant="error">Exhausted</StatusBadge>
            ) : sessionStatus === "expired" ? (
              <StatusBadge variant="error">Expired</StatusBadge>
            ) : sessionStatus === "creating" ? (
              <StatusBadge variant="default">Creating...</StatusBadge>
            ) : sessionStatus === "error" ? (
              <StatusBadge variant="error">Error</StatusBadge>
            ) : (
              <StatusBadge variant="default">Idle</StatusBadge>
            )}
          </div>
          
          {sessionStatus === "idle" || sessionStatus === "creating" || sessionStatus === "error" ? (
            <div className="flex flex-col gap-4">
              {errorMessage ? (
                <div className="p-3 rounded-lg bg-destructive/10 border border-destructive/20 text-xs flex flex-col gap-1.5 text-destructive">
                  <div className="font-semibold flex items-center gap-1.5">
                    <AlertTriangle className="w-3.5 h-3.5 shrink-0" />
                    Session Failed
                  </div>
                  <p className="font-mono text-[11px] leading-relaxed break-words">{errorMessage}</p>
                  {errorMessage.toLowerCase().includes("lnd") && (
                    <div className="text-[10px] text-muted-foreground mt-1 border-t border-destructive/20 pt-1.5 leading-relaxed">
                      💡 <strong>Diagnosis:</strong> The node cannot connect to LND at port 8081. Verify your Polar network is started or set <code className="bg-muted px-1 rounded">backend = mock</code> in <code className="bg-muted px-1 rounded">config/node.toml</code>.
                    </div>
                  )}
                </div>
              ) : (
                <p className="text-xs text-muted-foreground leading-relaxed">
                  Open an L402 inference session. Set your spend budget below before connecting.
                </p>
              )}

              {/* Set Budget Section */}
              <div className="flex flex-col gap-2 pt-1 border-t border-border/40">
                <div className="flex items-center justify-between text-xs">
                  <span className="font-medium text-foreground flex items-center gap-1">
                    <Coins className="w-3.5 h-3.5 text-lightning" /> Set Session Budget:
                  </span>
                  <span className="font-mono text-lightning font-semibold">
                    {sessionData.budget_sats} SATS
                  </span>
                </div>

                <div className="relative flex items-center">
                  <input
                    type="number"
                    min="1"
                    max="1000000"
                    value={sessionData.budget_sats || ""}
                    onChange={(e) => {
                      const val = parseInt(e.target.value, 10);
                      if (onBudgetChange) {
                        onBudgetChange(isNaN(val) ? 0 : val);
                      }
                    }}
                    disabled={sessionStatus === "creating"}
                    placeholder="Enter budget in sats..."
                    className="w-full h-9 px-3 pr-12 text-sm bg-card border border-border/80 rounded-md font-mono focus:outline-none focus:ring-1 focus:ring-primary focus:border-primary transition-all text-foreground"
                  />
                  <span className="absolute right-3 text-xs font-mono text-muted-foreground pointer-events-none">
                    sats
                  </span>
                </div>

                {/* Preset Chips */}
                <div className="grid grid-cols-4 gap-1.5 pt-0.5">
                  {[50, 100, 250, 500].map((preset) => (
                    <button
                      key={preset}
                      type="button"
                      onClick={() => onBudgetChange && onBudgetChange(preset)}
                      disabled={sessionStatus === "creating"}
                      className={`py-1 text-xs font-mono rounded border transition-all ${
                        sessionData.budget_sats === preset
                          ? "bg-primary/20 border-primary text-primary font-semibold shadow-sm"
                          : "border-border/50 bg-card/40 hover:bg-card text-muted-foreground hover:text-foreground"
                      }`}
                    >
                      {preset}
                    </button>
                  ))}
                </div>
              </div>

              <Button 
                onClick={onStartSession} 
                className="w-full font-medium mt-1" 
                disabled={sessionStatus === "creating" || !sessionData.budget_sats}
                variant={sessionStatus === "error" ? "destructive" : "default"}
              >
                {sessionStatus === "creating"
                  ? "Creating Session..."
                  : sessionStatus === "error"
                  ? "Retry Session"
                  : `Open Session (${sessionData.budget_sats} Sats)`}
              </Button>
              {canRecover && (
                <div className="border-t border-border/40 pt-3">
                  <button
                    type="button"
                    className="text-xs text-muted-foreground underline hover:text-foreground"
                    onClick={() => { setRecoveryOpen((open) => !open); setRecoveryError(null); }}
                  >
                    {recoveryOpen ? "Hide development recovery" : "Restore existing funded session (development)"}
                  </button>
                  {recoveryOpen && (
                    <form onSubmit={handleRecoverySubmit} className="mt-3 flex flex-col gap-2">
                      <p className="text-[11px] text-muted-foreground">Enter the existing session credentials locally. The backend verifies them on your first inference request.</p>
                      <input aria-label="Existing session ID" autoComplete="off" value={recoveryInput.sessionId} onChange={(e) => setRecoveryInput((v) => ({ ...v, sessionId: e.target.value }))} placeholder="Session UUID" className="h-9 rounded-md border border-input bg-background px-3 text-xs font-mono" />
                      <input aria-label="Original BOLT11 invoice" autoComplete="off" value={recoveryInput.invoice} onChange={(e) => setRecoveryInput((v) => ({ ...v, invoice: e.target.value }))} placeholder="Original BOLT11 invoice" className="h-9 rounded-md border border-input bg-background px-3 text-xs font-mono" />
                      <input aria-label="L402 macaroon" autoComplete="off" type="password" value={recoveryInput.macaroon} onChange={(e) => setRecoveryInput((v) => ({ ...v, macaroon: e.target.value }))} placeholder="L402 macaroon" className="h-9 rounded-md border border-input bg-background px-3 text-xs font-mono" />
                      <input aria-label="Payment preimage" autoComplete="off" type="password" value={recoveryInput.preimage} onChange={(e) => setRecoveryInput((v) => ({ ...v, preimage: e.target.value }))} placeholder="64-character payment preimage" className="h-9 rounded-md border border-input bg-background px-3 text-xs font-mono" />
                      {recoveryError && <p role="alert" className="text-[11px] text-destructive">{recoveryError}</p>}
                      <Button type="submit" size="sm" variant="outline">Restore session locally</Button>
                    </form>
                  )}
                </div>
              )}
            </div>
          ) : (
            <div className="flex flex-col gap-4">
              <div className="flex flex-col gap-1">
                <span className="text-xs text-muted-foreground uppercase tracking-wider">ID</span>
                <span className="font-mono text-xs text-foreground truncate" title={sessionData.id}>
                  {sessionData.id || "sess_..."}
                </span>
              </div>

              {/* Budget Meter */}
              <div className="flex flex-col gap-2 pt-2 border-t border-border/50">
                {balanceSyncStatus === "confirmed" ? (
                  <div className="flex justify-between items-end">
                    <SatsAmount amount={sessionData.remaining_sats ?? sessionData.budget_sats} label="Remaining" />
                    <div className="text-right">
                      <span className="text-xs text-muted-foreground">/ {sessionData.budget_sats} sats</span>
                    </div>
                  </div>
                ) : (
                  <p role="status" className="text-xs text-muted-foreground">
                    {balanceSyncStatus === "syncing" ? "Syncing persisted session balance…" : "Balance is unverified; reconnect to synchronize."}
                  </p>
                )}
                <Progress
                  value={balanceSyncStatus === "confirmed" ? (
                    sessionData.budget_sats > 0
                      ? ((sessionData.remaining_sats ?? sessionData.budget_sats) / sessionData.budget_sats) * 100
                      : 0
                  ) : 0}
                  className="h-2"
                />
              </div>

              {/* Session Actions */}
              <div className="flex items-center justify-between pt-2 border-t border-border/40 text-xs">
                <Link href="/sessions" className="text-muted-foreground hover:text-foreground text-[11px] underline">
                  All Sessions →
                </Link>
                {onResetSession && (
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={onResetSession}
                    className="h-7 text-xs flex items-center gap-1 hover:bg-primary/10 hover:text-primary"
                  >
                    <PlusCircle className="w-3 h-3" /> New Session
                  </Button>
                )}
              </div>
            </div>
          )}
        </Card>

        {(sessionStatus === "payment_required" || sessionStatus === "payment_submitted" || sessionStatus === "active" || sessionStatus === "invalid_payment") && (
          <>
            {/* L402 Challenge Inspector (Collapsible) */}
            <div className="flex flex-col rounded-lg border border-border bg-card overflow-hidden">
              <button 
                onClick={() => setL402Expanded(!l402Expanded)}
                className="flex items-center justify-between p-3 bg-card hover:bg-card/80 transition-colors"
              >
                <div className="flex items-center gap-2">
                  <Zap className="w-4 h-4 text-lightning" />
                  <h3 className="font-semibold text-xs text-foreground uppercase tracking-wider">L402 Challenge</h3>
                </div>
                {l402Expanded ? <ChevronDown className="w-4 h-4 text-muted-foreground" /> : <ChevronRight className="w-4 h-4 text-muted-foreground" />}
              </button>
              
              {l402Expanded && (
                <div className="p-3 pt-0 border-t border-border/50 bg-background/50 flex flex-col gap-3 font-mono text-[10px]">
                  <div className="flex flex-col gap-1 mt-3">
                    <span className="text-muted-foreground uppercase">HTTP Status</span>
                    <span className="text-warning">402 Payment Required</span>
                  </div>
                  <div className="flex flex-col gap-1">
                    <span className="text-muted-foreground uppercase">Invoice</span>
                    <span className="text-foreground break-all">{sessionData.invoice || "Awaiting request..."}</span>
                  </div>
                  <div className="flex flex-col gap-1">
                    <span className="text-muted-foreground uppercase">Capability</span>
                    <span className="text-foreground">{sessionData.capability || "inference"}</span>
                  </div>
                </div>
              )}
            </div>

            {/* Capability / Auth Log */}
            <div className="flex flex-col gap-3 pt-2">
              <h3 className="text-xs font-semibold text-muted-foreground uppercase tracking-wider">Authorization</h3>
              
              <div className="flex flex-col gap-2">
                <div className={`flex items-center gap-3 text-sm ${sessionStatus === 'active' ? '' : 'opacity-50'}`}>
                  <ShieldCheck className={`w-4 h-4 ${sessionStatus === 'active' ? 'text-success' : ''}`} />
                  <span className="text-muted-foreground">Payment proof {sessionStatus === 'active' ? 'verified' : 'pending'}</span>
                </div>
                <span className="text-xs text-muted-foreground pl-7">
                  {sessionStatus === "active" ? "Backend authorization confirmed" : sessionStatus === "invalid_payment" ? "Backend rejected the payment proof; it remains available for retry" : "Backend verification occurs with the first authenticated inference request"}
                </span>
              </div>
              <div className={`flex items-center gap-3 text-sm ${sessionStatus === 'active' ? '' : 'opacity-50'}`}>
                <Cpu className={`w-4 h-4 ${sessionStatus === 'active' ? 'text-success' : ''}`} />
                <span className="text-muted-foreground font-mono text-xs">Capability: {sessionData.capability || "inference"}</span>
              </div>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
