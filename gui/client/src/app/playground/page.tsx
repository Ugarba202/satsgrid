"use client";

import { useState, useEffect } from "react";
import { Navbar } from "@/components/layout/navbar";
import { ChatPanel } from "@/components/playground/chat-panel";
import { ProtocolPanel } from "@/components/playground/protocol-panel";
import { SessionStatus, SessionData } from "@/types/session";
import { createSession } from "@/lib/api/sessions";
import { L402Error } from "@/types/api";
import { streamChatCompletion } from "@/lib/api/inference";
import { Message } from "@/types/inference";
import { 
  saveStoredSession, 
  updateStoredSessionBudget, 
  updateStoredSessionMessages,
  getStoredSessionById,
  getStoredSessions,
  setActiveSessionId,
  getActiveSessionId,
  clearActiveSessionId,
  StoredSession 
} from "@/lib/storage/sessions";

export default function PlaygroundPage() {
  const [sessionStatus, setSessionStatus] = useState<SessionStatus>("idle");
  const [sessionData, setSessionData] = useState<SessionData>({ budget_sats: 100, model: "llama3.2" });
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [isStreaming, setIsStreaming] = useState(false);

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
        setSessionData({
          id: stored.id,
          model: stored.model || "llama3.2",
          capability: stored.capability || "inference",
          budget_sats: stored.budget_sats,
          remaining_sats: stored.remaining_sats,
          macaroon: stored.macaroon,
          preimage: stored.preimage,
          invoice: stored.invoice,
        });
        if (stored.messages && stored.messages.length > 0) {
          setMessages(stored.messages);
        }
        setSessionStatus(
          stored.status === "active"
            ? "active"
            : stored.status === "closed"
            ? "closed"
            : "idle"
        );
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
        macaroon: latestActive.macaroon,
        preimage: latestActive.preimage,
        invoice: latestActive.invoice,
      });
      if (latestActive.messages && latestActive.messages.length > 0) {
        setMessages(latestActive.messages);
      }
      setSessionStatus("active");
    }
  }, []);

  const handleBudgetChange = (budget: number) => {
    setSessionData((prev) => ({ ...prev, budget_sats: budget, remaining_sats: budget }));
    setErrorMessage(null);
  };

  const handleResetSession = () => {
    clearActiveSessionId();
    setSessionStatus("idle");
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
    } catch (e: any) {
      if (e instanceof L402Error) {
        setSessionStatus("payment_required");
        setSessionData((prev) => ({
          ...prev,
          invoice: e.challenge.invoice,
          macaroon: e.challenge.macaroon,
          id: e.challenge.parsedCaveats?.session,
          capability: e.challenge.parsedCaveats?.capability,
          budget_sats: parseInt(e.challenge.parsedCaveats?.budget || `${sessionData.budget_sats}`, 10),
          remaining_sats: parseInt(e.challenge.parsedCaveats?.budget || `${sessionData.budget_sats}`, 10),
        }));
      } else {
        setSessionStatus("error");
        setErrorMessage(e?.message || "Failed to create session on SatsGrid node.");
      }
    }
  };

  const activateSessionWithPreimage = (preimage: string) => {
    setSessionData((prev) => {
      const updated = { ...prev, preimage };
      const sessionId = updated.id || `sess_${Date.now()}`;
      const sessionRecord: StoredSession = {
        id: sessionId,
        status: "active",
        model: updated.model || "llama3.2",
        capability: updated.capability || "inference",
        budget_sats: updated.budget_sats || 100,
        remaining_sats: updated.remaining_sats ?? updated.budget_sats ?? 100,
        macaroon: updated.macaroon || "",
        preimage,
        invoice: updated.invoice,
        created_at: new Date().toISOString(),
        requests_count: 0,
        messages: [],
      };
      saveStoredSession(sessionRecord);
      setActiveSessionId(sessionId);
      return { ...updated, id: sessionId };
    });
    setSessionStatus("active");
  };

  const handlePayInvoice = async () => {
    setSessionStatus("authorizing");
    try {
      const invoiceStr = sessionData.invoice as string;
      const { requestProvider } = await import('@getalby/bitcoin-connect');
      const webln = await requestProvider();
      const paymentResponse = await webln.sendPayment(invoiceStr);
      activateSessionWithPreimage(paymentResponse.preimage);
    } catch (err: any) {
      console.error("WebLN Payment failed:", err);
      setSessionStatus("payment_required");
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
    } catch (err: any) {
      console.error("NWC Payment failed:", err);
      setSessionStatus("payment_required");
      throw err;
    }
  };

  const handleManualPreimage = (preimage: string) => {
    activateSessionWithPreimage(preimage);
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

    try {
      // Add a placeholder assistant message
      setMessages((prev) => [...prev, { role: "assistant", content: "" }]);

      let accumulatedContent = "";

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
              updated[updated.length - 1] = { ...last, content: last.content + chunk };
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
          setSessionData((prev) => {
            if (prev.id) {
              updateStoredSessionBudget(prev.id, remaining, false);
            }
            return {
              ...prev,
              remaining_sats: remaining
            };
          });
        }
      );

      // Persist completed conversation
      if (sessionData.id) {
        const finalized = [...newMessages, { role: "assistant" as const, content: accumulatedContent }];
        updateStoredSessionMessages(sessionData.id, finalized);
      }
    } catch (e) {
      console.error("Inference Error:", e);
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
          />
        </section>

        {/* Right Pane: Protocol Activity */}
        <aside className="w-full md:w-80 lg:w-96 flex-shrink-0 flex flex-col bg-card/30 overflow-y-auto">
          <ProtocolPanel 
            sessionStatus={sessionStatus} 
            sessionData={sessionData}
            errorMessage={errorMessage}
            onBudgetChange={handleBudgetChange}
            onStartSession={handleStartSession}
            onResetSession={handleResetSession}
          />
        </aside>
      </main>
    </div>
  );
}



