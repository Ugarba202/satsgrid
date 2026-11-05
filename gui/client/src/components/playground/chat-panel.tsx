import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { ModelSelector } from "./model-selector";
import { Send, Bot, User, Lock, Zap } from "lucide-react";
import { StatusBadge } from "@/components/satsgrid/status-badge";
import { SessionStatus, SessionData } from "@/types/session";
import { SatsAmount } from "@/components/satsgrid/sats-amount";
import { Message } from "@/types/inference";
import { useState, useEffect, KeyboardEvent } from "react";

interface ChatPanelProps {
  sessionStatus: SessionStatus;
  sessionData: SessionData;
  messages: Message[];
  isStreaming: boolean;
  onPayInvoice: () => void;
  onPayWithNwc?: (nwcUri: string) => Promise<void>;
  onManualPreimage?: (preimage: string) => void;
  onSendMessage: (content: string) => void;
  onSelectModel?: (model: string) => void;
}

export function ChatPanel({
  sessionStatus,
  sessionData,
  messages,
  isStreaming,
  onPayInvoice,
  onPayWithNwc,
  onManualPreimage,
  onSendMessage,
  onSelectModel,
}: ChatPanelProps) {
  const [input, setInput] = useState("");
  const [manualPreimage, setManualPreimage] = useState("");
  const [nwcUri, setNwcUri] = useState("");
  const [nwcError, setNwcError] = useState<string | null>(null);
  const [isPayingNwc, setIsPayingNwc] = useState(false);
  const [copied, setCopied] = useState(false);
  const [preimageError, setPreimageError] = useState<string | null>(null);
  const [isPayingMock, setIsPayingMock] = useState(false);

  useEffect(() => {
    if (typeof window !== "undefined") {
      const saved = localStorage.getItem("satsgrid_nwc_uri");
      if (saved) setNwcUri(saved);
    }
  }, []);

  const handleMockPay = async () => {
    if (!sessionData.invoice) return;
    setIsPayingMock(true);
    setPreimageError(null);
    try {
      const res = await fetch("/internal/mock/pay", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ invoice: sessionData.invoice }),
      });
      if (!res.ok) {
        throw new Error("Mock payment endpoint rejected. Make sure backend = 'mock' in config/node.toml");
      }
      const data = await res.json();
      if (data.preimage && onManualPreimage) {
        onManualPreimage(data.preimage);
      }
    } catch (err: any) {
      console.error("Mock payment error:", err);
      setPreimageError(err.message || "Failed to settle mock payment");
    } finally {
      setIsPayingMock(false);
    }
  };

  const isInputDisabled =
    sessionStatus === "idle" ||
    sessionStatus === "creating" ||
    sessionStatus === "payment_required" ||
    sessionStatus === "authorizing" ||
    sessionStatus === "error" ||
    isStreaming;

  const handleSend = () => {
    if (!input.trim() || isInputDisabled) return;
    onSendMessage(input);
    setInput("");
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  const handleCopyInvoice = () => {
    if (sessionData.invoice) {
      navigator.clipboard.writeText(sessionData.invoice);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  };

  const handleNwcSubmit = async () => {
    setNwcError(null);
    const cleaned = nwcUri.trim();
    if (!cleaned) {
      setNwcError("Please enter your Nostr Wallet Connect (NWC) pairing URI.");
      return;
    }
    if (!cleaned.startsWith("nostr+walletconnect://")) {
      setNwcError("Invalid URI. Must start with nostr+walletconnect://");
      return;
    }
    if (typeof window !== "undefined") {
      localStorage.setItem("satsgrid_nwc_uri", cleaned);
    }
    if (onPayWithNwc) {
      try {
        setIsPayingNwc(true);
        await onPayWithNwc(cleaned);
      } catch (err: any) {
        setNwcError(err?.message || "Failed to pay invoice via NWC.");
      } finally {
        setIsPayingNwc(false);
      }
    }
  };

  const handleVerifyManualPreimage = () => {
    setPreimageError(null);
    const cleaned = manualPreimage.trim().toLowerCase();
    if (!cleaned) {
      setPreimageError("Please enter a payment preimage.");
      return;
    }
    if (cleaned.length !== 64 || !/^[0-9a-fA-F]{64}$/.test(cleaned)) {
      setPreimageError("Preimage must be a 64-character hex string.");
      return;
    }
    if (onManualPreimage) {
      onManualPreimage(cleaned);
    }
  };

  const invoiceStr = sessionData.invoice || "";
  const networkName = invoiceStr.startsWith("lnbcrt")
    ? "Bitcoin Regtest (Polar)"
    : invoiceStr.startsWith("lntb")
    ? "Bitcoin Testnet"
    : invoiceStr.startsWith("lnbc")
    ? "Bitcoin Mainnet"
    : "Lightning Network";

  return (
    <div className="flex-1 flex flex-col h-full relative">
      {/* Chat Header */}
      <div className="h-14 border-b border-border/40 flex items-center px-4 justify-between bg-background/95 z-10">
        <h2 className="font-semibold text-sm">AI Playground</h2>
        <ModelSelector value={sessionData.model || "llama3.2"} onChange={onSelectModel} />
      </div>

      {/* Status Bar */}
      {sessionStatus === "active" && (
        <div className="flex items-center justify-center gap-4 py-2 border-b border-border/30 bg-card/50 text-[10px] font-mono tracking-widest uppercase text-muted-foreground">
          <span className="flex items-center gap-1.5">
            <div className="w-1.5 h-1.5 rounded-full bg-success" /> Session Active
          </span>
          <span className="text-border">|</span>
          <span className="flex items-center gap-1 text-lightning">
            ⚡ {sessionData.budget_sats} SATS
          </span>
          <span className="text-border">|</span>
          <span>Model {sessionData.model || "llama3.2"}</span>
        </div>
      )}

      <div className="flex-1 overflow-y-auto p-4 flex flex-col gap-6">
        {sessionStatus === "idle" || sessionStatus === "creating" ? (
          <div className="flex-1 flex flex-col items-center justify-center text-center opacity-50 space-y-4">
            <Bot className="w-12 h-12 text-muted-foreground" />
            <div className="space-y-1">
              <h3 className="font-medium text-foreground">SatsGrid Ready</h3>
              <p className="text-sm text-muted-foreground max-w-sm">
                Create a session in the Protocol Activity panel to begin permissionless inference.
              </p>
            </div>
          </div>
        ) : sessionStatus === "error" ? (
          <div className="flex-1 flex flex-col items-center justify-center text-center opacity-80 space-y-4">
            <div className="w-12 h-12 rounded-full bg-destructive/20 flex items-center justify-center">
              <span className="text-destructive font-bold text-xl">!</span>
            </div>
            <div className="space-y-1">
              <h3 className="font-medium text-destructive">Connection Error</h3>
              <p className="text-sm text-muted-foreground max-w-sm">
                Failed to connect to the SatsGrid node. Is the Rust backend running?
              </p>
            </div>
          </div>
        ) : sessionStatus === "payment_required" || sessionStatus === "authorizing" ? (
          <div className="flex items-start gap-4 max-w-3xl mx-auto w-full">
            <div className="w-8 h-8 rounded-full flex items-center justify-center shrink-0 bg-lightning/20">
              <Zap className="w-4 h-4 text-lightning" />
            </div>
            <div className="flex flex-col gap-3 mt-1 w-full max-w-lg">
              <div className="flex items-center justify-between">
                <span className="font-semibold text-sm">L402 Payment Required</span>
                <span className="text-[11px] font-mono font-medium px-2 py-0.5 rounded bg-lightning/10 text-lightning border border-lightning/20">
                  {networkName}
                </span>
              </div>

              {/* Real Lightning Payment Card */}
              <div className="rounded-xl border border-border bg-card p-5 shadow-lg flex flex-col gap-4">
                <div className="flex items-center justify-between pb-3 border-b border-border/50">
                  <span className="text-xs text-muted-foreground uppercase tracking-wider">
                    Session Budget
                  </span>
                  <SatsAmount amount={sessionData.budget_sats} />
                </div>

                <div className="flex flex-col gap-2">
                  <div className="flex items-center justify-between">
                    <span className="text-xs text-muted-foreground uppercase tracking-wider">
                      BOLT11 Invoice
                    </span>
                    <button
                      onClick={handleCopyInvoice}
                      className="text-xs text-lightning hover:underline font-mono"
                    >
                      {copied ? "✓ Copied" : "Copy Invoice"}
                    </button>
                  </div>
                  <div
                    onClick={handleCopyInvoice}
                    className="p-3 bg-background border border-border rounded-lg text-[11px] font-mono text-muted-foreground break-all max-h-24 overflow-y-auto cursor-pointer hover:border-lightning/50 transition-colors"
                    title="Click to copy invoice"
                  >
                    {sessionData.invoice}
                  </div>
                </div>

                <Button
                  className="w-full bg-lightning text-lightning-foreground hover:bg-lightning/90 font-bold py-2.5"
                  onClick={onPayInvoice}
                  disabled={sessionStatus === "authorizing" || isPayingNwc || isPayingMock}
                >
                  {sessionStatus === "authorizing" ? "Processing WebLN..." : "Pay with WebLN"}
                </Button>

                {/* 1-Click Local Mock Simulator Settlement */}
                <Button
                  variant="outline"
                  className="w-full font-mono text-xs border-success/40 text-success hover:bg-success/10 py-2.5 flex items-center justify-center gap-1.5"
                  onClick={handleMockPay}
                  disabled={isPayingMock || sessionStatus === "authorizing"}
                >
                  <Zap className="w-3.5 h-3.5 text-success" />
                  {isPayingMock ? "Settling Mock Payment..." : "Settle via Local Mock Simulator (1-Click)"}
                </Button>

                <div className="relative flex items-center justify-center my-1">
                  <div className="absolute inset-0 flex items-center">
                    <div className="w-full border-t border-border/50" />
                  </div>
                  <span className="relative bg-card px-2 text-[10px] uppercase tracking-widest text-muted-foreground">
                    Or Nostr Wallet Connect (NWC)
                  </span>
                </div>

                {/* Direct NWC Pairing & Payment (Alby Hub, Mutiny, Umbrel) */}
                <div className="flex flex-col gap-2 bg-background/50 p-3 rounded-lg border border-border/50">
                  <div className="flex items-center justify-between">
                    <span className="text-[11px] font-medium text-foreground">
                      Connect remote NWC wallet:
                    </span>
                    <span className="text-[10px] text-muted-foreground font-mono">NIP-47</span>
                  </div>
                  <div className="flex flex-col sm:flex-row gap-2">
                    <input
                      type="password"
                      placeholder="nostr+walletconnect://..."
                      value={nwcUri}
                      onChange={(e) => setNwcUri(e.target.value)}
                      className="flex-1 bg-background border border-input rounded-md px-3 py-1.5 text-xs font-mono text-foreground placeholder:text-muted-foreground/50 focus:outline-none focus:ring-1 focus:ring-lightning"
                    />
                    <Button
                      size="sm"
                      onClick={handleNwcSubmit}
                      disabled={isPayingNwc || sessionStatus === "authorizing"}
                      className="text-xs font-semibold bg-primary hover:bg-primary/90"
                    >
                      {isPayingNwc ? "Paying via NWC..." : "Pay with NWC"}
                    </Button>
                  </div>
                  {nwcError && (
                    <p className="text-[11px] text-destructive">{nwcError}</p>
                  )}
                </div>

                <div className="relative flex items-center justify-center my-1">
                  <div className="absolute inset-0 flex items-center">
                    <div className="w-full border-t border-border/50" />
                  </div>
                  <span className="relative bg-card px-2 text-[10px] uppercase tracking-widest text-muted-foreground">
                    Or external wallet
                  </span>
                </div>

                {/* External Wallet Preimage Input (Polar LND / Zeus / Phoenix) */}
                <div className="flex flex-col gap-2 bg-background/50 p-3 rounded-lg border border-border/50">
                  <span className="text-[11px] text-muted-foreground">
                    Paid via Polar LND or external wallet? Enter the payment preimage:
                  </span>
                  <div className="flex gap-2">
                    <input
                      type="text"
                      placeholder="64-character hex preimage..."
                      value={manualPreimage}
                      onChange={(e) => setManualPreimage(e.target.value)}
                      className="flex-1 bg-background border border-input rounded-md px-3 py-1.5 text-xs font-mono text-foreground placeholder:text-muted-foreground/50 focus:outline-none focus:ring-1 focus:ring-lightning"
                    />
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={handleVerifyManualPreimage}
                      className="text-xs font-semibold"
                    >
                      Unlock
                    </Button>
                  </div>
                  {preimageError && (
                    <p className="text-[11px] text-destructive">{preimageError}</p>
                  )}
                </div>
              </div>
            </div>
          </div>
        ) : (
          <>
            {messages.length === 0 && (
              <div className="flex items-start gap-4 max-w-3xl mx-auto w-full">
                <div className="w-8 h-8 rounded-full bg-primary/20 flex items-center justify-center shrink-0">
                  <Bot className="w-4 h-4 text-primary" />
                </div>
                <div className="flex flex-col gap-1 mt-1">
                  <span className="font-medium text-sm">SatsGrid</span>
                  <p className="text-muted-foreground leading-relaxed">
                    Session established. My capability is currently bounded to {sessionData.budget_sats} sats. How can I help you?
                  </p>
                </div>
              </div>
            )}
            
            {messages.map((msg, i) => (
              <div key={i} className={`flex items-start gap-4 max-w-3xl mx-auto w-full ${msg.role === "user" ? "flex-row-reverse" : ""}`}>
                <div className={`w-8 h-8 rounded-full flex items-center justify-center shrink-0 ${msg.role === "user" ? "bg-muted" : "bg-primary/20"}`}>
                  {msg.role === "user" ? <User className="w-4 h-4 text-muted-foreground" /> : <Bot className="w-4 h-4 text-primary" />}
                </div>
                <div className={`flex flex-col gap-1 mt-1 ${msg.role === "user" ? "items-end" : ""}`}>
                  <span className="font-medium text-sm">{msg.role === "user" ? "You" : "SatsGrid"}</span>
                  <div className={`leading-relaxed ${msg.role === "user" ? "bg-muted px-4 py-2 rounded-2xl rounded-tr-sm" : "text-foreground whitespace-pre-wrap"}`}>
                    {msg.content}
                    {isStreaming && i === messages.length - 1 && msg.role === "assistant" && (
                      <span className="inline-block w-2 h-4 bg-primary animate-pulse ml-1 align-middle" />
                    )}
                  </div>
                </div>
              </div>
            ))}
          </>
        )}
      </div>

      {/* Input Area */}
      <div className="p-4 bg-background border-t border-border/40">
        <div className="max-w-3xl mx-auto relative rounded-xl border border-border bg-card shadow-sm focus-within:ring-1 focus-within:ring-ring">
          <Textarea 
            placeholder={isInputDisabled ? "Awaiting active session..." : "Send a message..."}
            disabled={isInputDisabled}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            className="min-h-[80px] w-full resize-none border-0 bg-transparent py-4 pl-4 pr-14 focus-visible:ring-0 rounded-xl disabled:opacity-50 disabled:cursor-not-allowed"
          />
          <Button 
            size="icon" 
            onClick={handleSend}
            className="absolute right-3 bottom-3 h-8 w-8 rounded-md"
            disabled={isInputDisabled || !input.trim()}
          >
            <Send className="w-4 h-4" />
          </Button>
        </div>
        
        {/* Privacy Indicator */}
        <div className="flex flex-col items-center justify-center mt-3 gap-1">
          <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <Lock className="w-3 h-3" />
            <span className="font-medium">Privacy by default</span>
          </div>
          <p className="text-[10px] text-muted-foreground/60 uppercase tracking-wider">
            No account · No API key · Prompts are not logged
          </p>
        </div>
      </div>
    </div>
  );
}

