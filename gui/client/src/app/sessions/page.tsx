"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { Navbar } from "@/components/layout/navbar";
import { Card } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { StatusBadge } from "@/components/satsgrid/status-badge";
import { SatsAmount } from "@/components/satsgrid/sats-amount";
import { Progress } from "@/components/ui/progress";
import { Activity, Key, Clock, Database, ArrowRight, Trash2, PlusCircle, MessageSquare, XCircle, AlertTriangle } from "lucide-react";
import { 
  getStoredSessions, 
  deleteStoredSession, 
  markStoredSessionClosed,
  getActiveSessionId, 
  setActiveSessionId, 
  clearActiveSessionId, 
  StoredSession 
} from "@/lib/storage/sessions";
import { closeSession } from "@/lib/api/sessions";

export default function SessionsPage() {
  const [sessions, setSessions] = useState<StoredSession[]>([]);
  const [activeSessionId, setActiveSessionIdState] = useState<string | null>(null);
  const [isLoaded, setIsLoaded] = useState(false);
  const [closingSession, setClosingSession] = useState<StoredSession | null>(null);
  const [isClosing, setIsClosing] = useState(false);
  const [closeError, setCloseError] = useState<string | null>(null);

  useEffect(() => {
    setSessions(getStoredSessions());
    setActiveSessionIdState(getActiveSessionId());
    setIsLoaded(true);
  }, []);

  const handleDelete = (id: string) => {
    deleteStoredSession(id);
    const updated = getStoredSessions();
    setSessions(updated);
    setActiveSessionIdState(getActiveSessionId());
  };

  const handleSelectSession = (id: string) => {
    setActiveSessionId(id);
    setActiveSessionIdState(id);
  };

  const handleConfirmClose = async () => {
    if (!closingSession) return;
    setIsClosing(true);
    setCloseError(null);

    try {
      await closeSession(
        { session_id: closingSession.id, reason: "user_closed_from_gui" },
        {
          macaroon: closingSession.macaroon,
          preimage: closingSession.preimage,
        }
      );

      markStoredSessionClosed(closingSession.id, Date.now(), "user_closed_from_gui");
      setSessions(getStoredSessions());
      setClosingSession(null);
    } catch (err: any) {
      console.error("Failed to close session:", err);
      setCloseError(err.message || "Failed to close session on backend");
    } finally {
      setIsClosing(false);
    }
  };

  const totalCapability = sessions.reduce((acc, s) => acc + (s.budget_sats || 0), 0);
  const totalSpent = sessions.reduce(
    (acc, s) => acc + Math.max(0, (s.budget_sats || 0) - (s.remaining_sats ?? s.budget_sats ?? 0)),
    0
  );

  const formatDate = (dateStr: string) => {
    try {
      const d = new Date(dateStr);
      if (isNaN(d.getTime())) return dateStr;
      return d.toLocaleDateString(undefined, {
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
    } catch {
      return dateStr;
    }
  };

  return (
    <div className="flex flex-col min-h-screen bg-background">
      <Navbar />

      <main className="flex-1 p-6 md:p-12 max-w-6xl mx-auto w-full flex flex-col gap-8">
        <div className="flex flex-col md:flex-row md:items-end justify-between gap-4">
          <div>
            <h1 className="text-2xl font-bold mb-2 text-foreground">My Sessions</h1>
            <p className="text-muted-foreground">
              Manage your real L402 capabilities, session budgets, and authorization proofs.
            </p>
          </div>

          <div className="flex flex-wrap items-center gap-4">
            <div className="flex items-center gap-4 text-sm bg-card/60 border border-border/50 px-4 py-2 rounded-lg">
              <div className="flex flex-col">
                <span className="text-muted-foreground uppercase text-[10px] tracking-wider">
                  Total Capability
                </span>
                <span className="font-semibold text-lightning">{totalCapability} SATS</span>
              </div>
              <div className="w-px h-8 bg-border/50"></div>
              <div className="flex flex-col">
                <span className="text-muted-foreground uppercase text-[10px] tracking-wider">
                  Total Spent
                </span>
                <span className="font-semibold text-foreground">{totalSpent} SATS</span>
              </div>
            </div>

            <Link href="/playground" onClick={() => clearActiveSessionId()}>
              <Button className="font-medium flex items-center gap-1.5 shadow-sm">
                <PlusCircle className="w-4 h-4" /> Start New Session
              </Button>
            </Link>
          </div>
        </div>

        {isLoaded && sessions.length === 0 ? (
          <Card className="p-12 text-center flex flex-col items-center justify-center gap-4 border border-dashed border-border bg-card/30">
            <div className="w-12 h-12 rounded-full bg-primary/10 flex items-center justify-center text-primary">
              <Key className="w-6 h-6" />
            </div>
            <div className="space-y-1">
              <h3 className="font-semibold text-lg text-foreground">No Active Sessions</h3>
              <p className="text-sm text-muted-foreground max-w-md">
                You haven't opened any inference sessions yet. Launch a session in the Playground to
                interact with models and track real sat expenditures.
              </p>
            </div>
            <Link href="/playground" onClick={() => clearActiveSessionId()}>
              <Button className="mt-2 font-medium flex items-center gap-2">
                <PlusCircle className="w-4 h-4" /> Open Playground
              </Button>
            </Link>
          </Card>
        ) : (
          <div className="flex flex-col gap-4">
            {sessions.map((session) => {
              const isCurrent = session.id === activeSessionId;
              return (
                <Card
                  key={session.id}
                  className={`p-6 border transition-all ${
                    isCurrent
                      ? "border-primary/60 bg-card shadow-md ring-1 ring-primary/30"
                      : session.status === "active"
                      ? "border-border/80 bg-card hover:border-primary/40 shadow-sm"
                      : "border-border/40 bg-background/50 opacity-80"
                  }`}
                >
                  <div className="flex flex-col lg:flex-row gap-6 lg:items-center justify-between">
                    {/* Identity & Status */}
                    <div className="flex items-start gap-4 lg:w-1/3">
                      <div
                        className={`w-10 h-10 rounded-full flex items-center justify-center shrink-0 ${
                          session.status === "active"
                            ? "bg-success/20 text-success"
                            : "bg-muted text-muted-foreground"
                        }`}
                      >
                        <Key className="w-5 h-5" />
                      </div>
                      <div className="flex flex-col gap-1 min-w-0">
                        <div className="flex items-center gap-2 flex-wrap">
                          <span className="font-mono text-sm font-semibold truncate max-w-[170px]" title={session.id}>
                            {session.id}
                          </span>
                          {session.status === "active" && <StatusBadge variant="success">Active</StatusBadge>}
                          {session.status === "exhausted" && (
                            <StatusBadge variant="warning">Exhausted</StatusBadge>
                          )}
                          {session.status === "expired" && <StatusBadge variant="error">Expired</StatusBadge>}
                          {session.status === "closed" && <StatusBadge variant="secondary">Closed</StatusBadge>}
                          {isCurrent && (
                            <span className="px-2 py-0.5 rounded text-[10px] font-semibold bg-primary/20 text-primary border border-primary/30">
                              ● Current
                            </span>
                          )}
                        </div>
                        <span className="text-xs text-muted-foreground flex items-center gap-1 font-mono">
                          <Clock className="w-3 h-3" /> {formatDate(session.created_at)}
                        </span>
                      </div>
                    </div>

                    {/* Scope & Capability */}
                    <div className="flex flex-col gap-1 lg:w-1/5">
                      <span className="text-[10px] text-muted-foreground uppercase tracking-widest">
                        Scope
                      </span>
                      <div className="flex items-center gap-2 text-sm">
                        <Database className="w-4 h-4 text-muted-foreground" />
                        <span className="font-medium">{session.model}</span>
                        <ArrowRight className="w-3 h-3 text-muted-foreground/50" />
                        <span className="font-mono text-xs text-muted-foreground">{session.capability}</span>
                      </div>
                    </div>

                    {/* Budget Tracker */}
                    <div className="flex flex-col gap-2 lg:w-1/4">
                      <div className="flex justify-between items-end">
                        <div className="flex flex-col">
                          <span className="text-[10px] text-muted-foreground uppercase tracking-widest">
                            Remaining
                          </span>
                          <SatsAmount amount={session.remaining_sats} />
                        </div>
                        <span className="text-xs text-muted-foreground font-mono">
                          / {session.budget_sats} sats
                        </span>
                      </div>
                      <Progress
                        value={
                          session.budget_sats > 0
                            ? (session.remaining_sats / session.budget_sats) * 100
                            : 0
                        }
                        className={`h-2 ${
                          session.status === "active"
                            ? "[&>div]:bg-success"
                            : "[&>div]:bg-muted-foreground"
                        }`}
                      />
                      <div className="flex items-center justify-between mt-1">
                        <div className="flex items-center gap-1">
                          <Activity className="w-3 h-3 text-muted-foreground" />
                          <span className="text-[10px] text-muted-foreground font-mono">
                            {session.requests_count || 0} requests
                          </span>
                        </div>
                        {session.messages && session.messages.length > 0 && (
                          <span className="text-[10px] text-muted-foreground font-mono">
                            {session.messages.length} messages
                          </span>
                        )}
                      </div>
                    </div>

                    {/* Actions: Resume, Close & Delete */}
                    <div className="flex items-center gap-3 lg:justify-end">
                      {session.status === "active" ? (
                        <>
                          <Link 
                            href={`/playground?session=${session.id}`}
                            onClick={() => handleSelectSession(session.id)}
                          >
                            <Button
                              size="sm"
                              variant="default"
                              className="font-medium flex items-center gap-1.5 shadow-sm"
                            >
                              <MessageSquare className="w-3.5 h-3.5" />
                              Resume Chat
                              <ArrowRight className="w-3.5 h-3.5 ml-0.5" />
                            </Button>
                          </Link>

                          <Button
                            size="sm"
                            variant="outline"
                            onClick={() => {
                              setClosingSession(session);
                              setCloseError(null);
                            }}
                            className="font-medium flex items-center gap-1 text-destructive hover:text-destructive hover:bg-destructive/10 border-destructive/30"
                            title="Close session permanently"
                          >
                            <XCircle className="w-3.5 h-3.5" />
                            Close
                          </Button>
                        </>
                      ) : (
                        <Link 
                          href={`/playground?session=${session.id}`}
                          onClick={() => handleSelectSession(session.id)}
                        >
                          <Button
                            size="sm"
                            variant="outline"
                            className="font-medium flex items-center gap-1.5 shadow-sm"
                          >
                            <MessageSquare className="w-3.5 h-3.5" />
                            View Chat
                            <ArrowRight className="w-3.5 h-3.5 ml-0.5" />
                          </Button>
                        </Link>
                      )}

                      <button
                        onClick={() => handleDelete(session.id)}
                        className="text-muted-foreground hover:text-destructive text-xs transition-colors p-2 rounded-md hover:bg-destructive/10"
                        title="Delete session"
                      >
                        <Trash2 className="w-4 h-4" />
                      </button>
                    </div>
                  </div>
                </Card>
              );
            })}
          </div>
        )}

        {/* Close Session Confirmation Modal */}
        {closingSession && (
          <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-xs p-4 animate-in fade-in-0">
            <Card className="max-w-md w-full p-6 border-border shadow-xl bg-card flex flex-col gap-4">
              <div className="flex items-center gap-3 text-warning">
                <AlertTriangle className="w-6 h-6 shrink-0" />
                <h3 className="text-lg font-bold text-foreground">Close Session Permanently?</h3>
              </div>

              <div className="flex flex-col gap-2 text-sm text-muted-foreground">
                <p>
                  You are about to close session <span className="font-mono text-foreground font-semibold">{closingSession.id.slice(0, 8)}...</span>.
                </p>
                <div className="p-3 bg-background rounded-md border border-border/50 text-xs flex flex-col gap-1.5 font-mono">
                  <div className="flex justify-between">
                    <span>Initial Budget:</span>
                    <span className="text-foreground">{closingSession.budget_sats} sats</span>
                  </div>
                  <div className="flex justify-between">
                    <span>Remaining Budget:</span>
                    <span className="text-lightning font-semibold">{closingSession.remaining_sats} sats</span>
                  </div>
                </div>
                <p className="text-xs text-warning/90 mt-1">
                  Closing is final. Any remaining balance will be forfeited and no further inference requests will be admitted.
                </p>

                {closeError && (
                  <div className="p-2.5 rounded bg-destructive/10 border border-destructive/20 text-destructive text-xs">
                    {closeError}
                  </div>
                )}
              </div>

              <div className="flex items-center justify-end gap-3 pt-2">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => {
                    setClosingSession(null);
                    setCloseError(null);
                  }}
                  disabled={isClosing}
                >
                  Cancel
                </Button>
                <Button
                  variant="default"
                  size="sm"
                  onClick={handleConfirmClose}
                  disabled={isClosing}
                  className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                >
                  {isClosing ? "Closing..." : "Yes, Close Session"}
                </Button>
              </div>
            </Card>
          </div>
        )}
      </main>
    </div>
  );
}
