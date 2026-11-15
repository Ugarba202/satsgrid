"use client";

import { useState, useEffect, useCallback } from "react";
import { Navbar } from "@/components/layout/navbar";
import { ChatPanel } from "@/components/playground/chat-panel";
import { ProtocolPanel } from "@/components/playground/protocol-panel";
import { SessionStatus, SessionData } from "@/types/session";
import { createSession, getSession } from "@/lib/api/sessions";
import { L402Error } from "@/types/api";
import { streamChatCompletion } from "@/lib/api/inference";
import { InferenceRequestError } from "@/lib/api/inference";
import { Message } from "@/types/inference";
import { attachPaymentProof, inferenceFailureStatus, markBackendConfirmed, recoverySession, restoredStatus, sessionFromChallenge, walletFailureStatus, type RecoveryCredentials } from "@/lib/session-lifecycle";
import { useSpokenAnnouncements } from "@/lib/speech/useSpokenAnnouncements";
import { 
  saveStoredSession, 
  updateStoredSessionBudget, 
  updateStoredSessionMessages,
  getStoredSessionById,
  getStoredSessions,
  savePendingStoredSession,
  setActiveSessionId,
  getActiveSessionId,
  clearActiveSessionId,
} from "@/lib/storage/sessions";
import type { StoredSession } from "@/lib/storage/sessions";

export default function PlaygroundPage() {
  const [sessionStatus, setSessionStatus] = useState<SessionStatus>("idle");
  const [sessionData, setSessionData] = useState<SessionData>({ budget_sats: 100, model: "llama3.2" });
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [isStreaming, setIsStreaming] = useState(false);
  const [balanceSyncStatus, setBalanceSyncStatus] = useState<"syncing" | "confirmed" | "unavailable">("unavailable");
  const [voiceAnnouncementsEnabled, setVoiceAnnouncementsEnabled] = useState(false);

  // Spoken Confirmation System
  const { announce } = useSpokenAnnouncements({
    enabled: voiceAnnouncementsEnabled,
  });

  const synchronizeSession = useCallback(async (stored: StoredSession) => {
    if (!stored.preimage || !stored.macaroon) {
      setBalanceSyncStatus("unavailable");
      return;
    }
    setBalanceSyncStatus("syncing");
    try {
      const snapshot = await getSession(stored.id, {
        macaroon: stored.macaroon,
        preimage: stored.preimage,
      });
      const updated: StoredSession = {
        ...stored,
        status: snapshot.status,
        budget_sats: snapshot.total_budget_sats,
        remaining_sats: snapshot.remaining_sats,
        reserved_sats: snapshot.reserved_sats,
        spent_sats: snapshot.spent_sats,
        requests_count: typeof snapshot.requests_count === "number" ? snapshot.requests_count : stored.requests_count,
        expires_at: snapshot.expires_at,
        closed_at: snapshot.closed_at ?? undefined,
      };
      const saved = saveStoredSession(updated);
      if (getActiveSessionId() !== snapshot.session_id) return;
      setSessionData((current) => current.id === snapshot.session_id ? {
        ...current,
        budget_sats: snapshot.total_budget_sats,
        remaining_sats: snapshot.remaining_sats,
        reserved_sats: snapshot.reserved_sats,
        spent_sats: snapshot.spent_sats,
        expires_at: snapshot.expires_at,
        backend_status: snapshot.status,
      } : current);
      setSessionStatus(snapshot.status === "pending_payment" ? "payment_submitted" : snapshot.status);
      setBalanceSyncStatus("confirmed");
      setErrorMessage(saved ? null : "The backend balance is current, but the local session could not be synchronized.");
    } catch (error) {
      setBalanceSyncStatus("unavailable");
      setErrorMessage(error instanceof Error ? error.message : "Could not synchronize the persisted session balance.");
    }
  }, []);

  // Restore active or requested session on page mount
  useEffect(() => {
    if (typeof window === "undefined") return;
    const params = new URLSearchParams(window.location.search);
    const querySessionId = params.get("session");
    const targetId = querySessionId || getActiveSessionId();

    if (targetId) {
      const stored = getStoredSessionById(targetId);
      if (stored) {
        setActiveSessionId(stored.id);
        // Restore browser-only session state after hydration to avoid server/client storage access.
        // eslint-disable-next-line react-hooks/set-state-in-effect
        setSessionData({
          id: stored.id,
          model: stored.model || "llama3.2",
          capability: stored.capability || "inference",
          budget_sats: stored.budget_sats,
          remaining_sats: stored.remaining_sats,
          reserved_sats: stored.reserved_sats,
          spent_sats: stored.spent_sats,
          expires_at: stored.expires_at,
          macaroon: stored.macaroon,
          preimage: stored.preimage,
          invoice: stored.invoice,
        });
        if (stored.messages && stored.messages.length > 0) {
          setMessages(stored.messages);
        }
        setSessionStatus(restoredStatus(stored));
        setBalanceSyncStatus("syncing");
        void synchronizeSession(stored);
        return;
      }
    }

    // Fallback: check if there is an active stored session with budget remaining
    const all = getStoredSessions();
    const latestActive = all.find((s) => s.status === "active");
    if (latestActive) {
      setActiveSessionId(latestActive.id);
      setSessionData({
        id: latestActive.id,
        model: latestActive.model || "llama3.2",
        capability: latestActive.capability || "inference",
        budget_sats: latestActive.budget_sats,
        remaining_sats: latestActive.remaining_sats,
        reserved_sats: latestActive.reserved_sats,
        spent_sats: latestActive.spent_sats,
        expires_at: latestActive.expires_at,
        macaroon: latestActive.macaroon,
        preimage: latestActive.preimage,
        invoice: latestActive.invoice,
      });
      if (latestActive.messages && latestActive.messages.length > 0) {
        setMessages(latestActive.messages);
      }
      setSessionStatus("active");
      setBalanceSyncStatus("syncing");
      void synchronizeSession(latestActive);
    }
  }, [synchronizeSession]);

  const handleBudgetChange = (budget: number) => {
    setSessionData((prev) => ({ ...prev, budget_sats: budget, remaining_sats: budget }));
    setErrorMessage(null);
  };

  const handleResetSession = () => {
    clearActiveSessionId();
    setSessionStatus("idle");
    setBalanceSyncStatus("unavailable");
    setSessionData({ budget_sats: 100, model: "llama3.2" });
    setMessages([]);
    setErrorMessage(null);
    if (typeof window !== "undefined" && window.history.replaceState) {
      window.history.replaceState({}, document.title, window.location.pathname);
    }
  };

  const handleStartSession = async () => {
    try {
      setErrorMessage(null);
      setSessionStatus("creating");
      await createSession(sessionData.budget_sats);
    } catch (e: unknown) {
      if (e instanceof L402Error) {
        const pending = sessionFromChallenge(e.challenge, sessionData.model || "llama3.2");
        if (!pending) {
          setSessionStatus("error");
          setErrorMessage("The node returned an incomplete or invalid L402 session challenge.");
          return;
        }
        if (!savePendingStoredSession(pending)) {
          setSessionStatus("error");
          setErrorMessage("Could not save this pending session locally. Existing session data was preserved.");
          return;
        }
        setActiveSessionId(pending.id);
        setSessionStatus("payment_required");
        announce("invoice_issued");
        setSessionData((prev) => ({
          ...prev,
          invoice: pending.invoice,
          macaroon: pending.macaroon,
          id: pending.id,
          capability: pending.capability,
          budget_sats: pending.budget_sats,
          remaining_sats: pending.remaining_sats,
        }));
      } else {
        setSessionStatus("error");
        setErrorMessage(e instanceof Error ? e.message : "Failed to create session on SatsGrid node.");
      }
    }
  };

  const activateSessionWithPreimage = (preimage: string) => {
    const sessionId = sessionData.id;
    if (!sessionId || !sessionData.macaroon || !sessionData.invoice || !/^[0-9a-f]{64}$/i.test(preimage)) {
      setErrorMessage("Payment proof or session details are incomplete.");
      setSessionStatus(walletFailureStatus());
      return;
    }
    const stored = getStoredSessionById(sessionId);
    const pending = stored && attachPaymentProof(stored, preimage);
    if (!pending) {
      setErrorMessage("The saved session could not be updated; existing session data was preserved.");
      setSessionStatus("payment_required");
      return;
    }
    if (!savePendingStoredSession(pending)) {
      // Keep the returned proof in memory so a storage quota failure cannot force
      // the user to pay this already-paid invoice again in the current tab.
      setSessionData((prev) => ({ ...prev, preimage: pending.preimage }));
      setErrorMessage("Payment proof is available for this tab, but local storage could not save it. Keep this tab open while you retry inference.");
      setSessionStatus("payment_submitted");
      return;
    }
    setActiveSessionId(sessionId);
    setSessionData((prev) => ({ ...prev, preimage }));
    setErrorMessage(null);
    // A wallet result is not backend verification. Keep the session pending until
    // the first authenticated inference response is accepted.
    setSessionStatus("payment_submitted");
  };

  const handlePayInvoice = async () => {
    setSessionStatus("authorizing");
    try {
      const invoiceStr = sessionData.invoice as string;
      const { requestProvider } = await import('@getalby/bitcoin-connect');
      const webln = await requestProvider();
      const paymentResponse = await webln.sendPayment(invoiceStr);
      activateSessionWithPreimage(paymentResponse.preimage);
    } catch {
      setErrorMessage("Wallet payment was cancelled or failed. The pending invoice and session are unchanged.");
      setSessionStatus(walletFailureStatus());
    }
  };

  const handlePayWithNwc = async (nwcUri: string) => {
    setSessionStatus("authorizing");
    try {
      const invoiceStr = sessionData.invoice as string;
      const { connectNWC, requestProvider } = await import('@getalby/bitcoin-connect');
      connectNWC(nwcUri);
      const webln = await requestProvider();
      const paymentResponse = await webln.sendPayment(invoiceStr);
      activateSessionWithPreimage(paymentResponse.preimage);
    } catch {
      setErrorMessage("Wallet payment was cancelled or failed. The pending invoice and session are unchanged.");
      setSessionStatus(walletFailureStatus());
      throw new Error("Wallet payment was cancelled or failed.");
    }
  };

  const handleManualPreimage = (preimage: string) => {
    activateSessionWithPreimage(preimage);
  };

  const handleRecoverSession = (credentials: RecoveryCredentials) => {
    const recovered = recoverySession(credentials, sessionData.model || "llama3.2");
    if (!recovered) {
      setErrorMessage("Recovery details are incomplete or invalid.");
      return false;
    }
    if (!savePendingStoredSession(recovered)) {
      setErrorMessage("A session with this ID already exists with different credentials or state.");
      return false;
    }
    setActiveSessionId(recovered.id);
    setSessionData({
      id: recovered.id,
      model: recovered.model,
      capability: recovered.capability,
      budget_sats: recovered.budget_sats,
      remaining_sats: recovered.remaining_sats,
      macaroon: recovered.macaroon,
      preimage: recovered.preimage,
      invoice: recovered.invoice,
    });
    setMessages([]);
    setErrorMessage(null);
    setSessionStatus("payment_submitted");
    setBalanceSyncStatus("syncing");
    void synchronizeSession(recovered);
    return true;
  };

  const handleSelectModel = (model: string) => {
    setSessionData((prev) => ({ ...prev, model }));
  };

  const handleSendMessage = async (content: string) => {
    if (!content.trim() || isStreaming) return;
    
    const newUserMessage: Message = { role: "user", content };
    const newMessages = [...messages, newUserMessage];
    setMessages(newMessages);
    if (sessionData.id) {
      updateStoredSessionMessages(sessionData.id, newMessages);
    }
    setIsStreaming(true);
    setBalanceSyncStatus("unavailable");
    const wasBackendConfirmed = sessionStatus === "active";
    if (!wasBackendConfirmed) setSessionStatus("authorizing");
    let backendConfirmed = wasBackendConfirmed;
    let observedRemaining = sessionData.remaining_sats ?? sessionData.budget_sats;

    try {
      // Add a placeholder assistant message
      setMessages((prev) => [...prev, { role: "assistant", content: "" }]);

      let accumulatedContent = "";
      let capturedReceiptId: string | undefined = undefined;

      await streamChatCompletion(
        {
          model: sessionData.model || "llama3.2",
          messages: newMessages,
        },
        sessionData.macaroon || "",
        sessionData.preimage || "",
        (chunk) => {
          accumulatedContent += chunk;
          setMessages((prev) => {
            const updated = [...prev];
            const last = updated[updated.length - 1];
            if (last && last.role === "assistant") {
              updated[updated.length - 1] = {
                ...last,
                content: last.content + chunk,
                receipt_id: capturedReceiptId ?? last.receipt_id,
              };
            }
            return updated;
          });
        },
        (sats) => {
          setSessionData((prev) => {
            const nextRemaining = Math.max(0, (prev.remaining_sats || prev.budget_sats || 0) - sats);
            if (prev.id) {
              updateStoredSessionBudget(prev.id, nextRemaining, true);
            }
            return {
              ...prev,
              remaining_sats: nextRemaining
            };
          });
        },
        (remaining) => {
          observedRemaining = remaining;
          if (remaining <= 0) setSessionStatus("exhausted");
          setSessionData((prev) => {
            if (prev.id) {
              updateStoredSessionBudget(prev.id, remaining, false);
            }
            return {
              ...prev,
              remaining_sats: remaining
            };
          });
        },
        () => {
          backendConfirmed = true;
          // Authoritative payment security rule: announce payment confirmation ONLY when backend authorizes
          announce("payment_confirmed");
          const id = sessionData.id;
          if (!id) return;
          const stored = getStoredSessionById(id);
          if (!stored) return;
          const confirmed = markBackendConfirmed(stored);
          if (!confirmed || !saveStoredSession(confirmed)) {
            setErrorMessage("The backend authorized this request, but the session status could not be saved locally.");
          }
          setSessionStatus("active");
          if (confirmed) setErrorMessage(null);
        },
        async () => {
          announce("inference_completed");
          if (!sessionData.id) return;
          const stored = getStoredSessionById(sessionData.id);
          if (stored) await synchronizeSession(stored);
        },
        (receiptId) => {
          capturedReceiptId = receiptId;
          announce("receipt_available");
          setMessages((prev) => {
            const updated = [...prev];
            const last = updated[updated.length - 1];
            if (last && last.role === "assistant") {
              updated[updated.length - 1] = { ...last, receipt_id: receiptId };
            }
            return updated;
          });
        }
      );

      // Persist completed conversation
      if (sessionData.id) {
        const finalized = [
          ...newMessages,
          {
            role: "assistant" as const,
            content: accumulatedContent,
            receipt_id: capturedReceiptId,
          },
        ];
        updateStoredSessionMessages(sessionData.id, finalized);
      }
    } catch (e) {
      const message = e instanceof Error ? e.message : "Inference request failed.";
      setErrorMessage(message);
      announce("inference_failed", "Check the displayed error message.");
      if (backendConfirmed) {
        setSessionStatus(observedRemaining <= 0 ? "exhausted" : "active");
      } else {
        const httpStatus = e instanceof InferenceRequestError ? e.status : undefined;
        setSessionStatus(inferenceFailureStatus(httpStatus, message, Boolean(sessionData.preimage)));
      }
      setMessages((prev) => {
        const updated = [...prev];
        const last = updated[updated.length - 1];
        if (last && last.role === "assistant") {
          updated[updated.length - 1] = { ...last, content: last.content + "\n\n**[Connection to SatsGrid node failed or was unauthorized.]**" };
        }
        if (sessionData.id) {
          updateStoredSessionMessages(sessionData.id, updated);
        }
        return updated;
      });
    } finally {
      setIsStreaming(false);
    }
  };

  return (
    <div className="flex flex-col h-screen overflow-hidden bg-background">
      <Navbar />
      
      <main className="flex-1 flex flex-col md:flex-row overflow-hidden">
        {/* Left Pane: Chat Area */}
        <section className="flex-1 flex flex-col min-w-0 border-r border-border/40">
          <ChatPanel 
            sessionStatus={sessionStatus} 
            sessionData={sessionData}
            messages={messages}
            isStreaming={isStreaming}
            onPayInvoice={handlePayInvoice}
            onPayWithNwc={handlePayWithNwc}
            onManualPreimage={handleManualPreimage}
            onSendMessage={handleSendMessage}
            onSelectModel={handleSelectModel}
            onStartNewSession={handleResetSession}
          />
        </section>

        {/* Right Pane: Protocol Activity */}
        <aside className="w-full md:w-80 lg:w-96 flex-shrink-0 flex flex-col bg-card/30 overflow-y-auto">
          <ProtocolPanel 
            sessionStatus={sessionStatus} 
            sessionData={sessionData}
            balanceSyncStatus={balanceSyncStatus}
            errorMessage={errorMessage}
            voiceAnnouncementsEnabled={voiceAnnouncementsEnabled}
            onToggleVoiceAnnouncements={setVoiceAnnouncementsEnabled}
            onBudgetChange={handleBudgetChange}
            onStartSession={handleStartSession}
            onResetSession={handleResetSession}
            onRecoverSession={handleRecoverSession}
          />
        </aside>
      </main>
    </div>
  );
}
