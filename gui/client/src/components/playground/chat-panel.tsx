import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { ModelSelector } from "./model-selector";
import { Send, Bot, User, Lock, Zap, QrCode, Copy, Check, Clock, AlertCircle, RefreshCw, PlusCircle, AlertTriangle, ShieldCheck, Mic, MicOff, Volume2, Square } from "lucide-react";
import { SessionStatus, SessionData } from "@/types/session";
import { SatsAmount } from "@/components/satsgrid/sats-amount";
import { resolveNetworkIdentity } from "@/lib/api/network";
import { fetchNodeHealth, type NodeLightningInfo } from "@/lib/api/node";
import { parseBolt11Expiry, formatSecondsRemaining, type Bolt11ExpiryInfo } from "@/lib/api/bolt11";
import { LightningQrCode } from "@/components/satsgrid/lightning-qr-code";
import { ReceiptDialog } from "./receipt-dialog";
import { Message } from "@/types/inference";
import { useState, useEffect, KeyboardEvent, useMemo } from "react";
import { useSpeechRecognition } from "@/lib/speech/useSpeechRecognition";
import { useSpeechSynthesis } from "@/lib/speech/useSpeechSynthesis";
import { fetchNodeModels, type ModelEntry } from "@/lib/api/node";
import { estimateInferenceCost, type CostEstimateResult } from "@/lib/pricing/estimate";

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
  onStartNewSession?: () => void;
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
  onStartNewSession,
}: ChatPanelProps) {
  const [input, setInput] = useState("");
  const [manualPreimage, setManualPreimage] = useState("");
  const [selectedReceiptId, setSelectedReceiptId] = useState<string | null>(null);
  const [isReceiptDialogOpen, setIsReceiptDialogOpen] = useState(false);
  const [nwcUri, setNwcUri] = useState("");
  const [nwcError, setNwcError] = useState<string | null>(null);
  const [isPayingNwc, setIsPayingNwc] = useState(false);
  const [preimageError, setPreimageError] = useState<string | null>(null);
  const [isPayingMock, setIsPayingMock] = useState(false);
  const [nodeLightning, setNodeLightning] = useState<NodeLightningInfo | null>(null);
  const [showQr, setShowQr] = useState(true);
  const [nowSec, setNowSec] = useState(() => Math.floor(Date.now() / 1000));
  const [copyFeedback, setCopyFeedback] = useState<"idle" | "success" | "error">("idle");
  const [activeSpeechMessageIndex, setActiveSpeechMessageIndex] = useState<number | null>(null);
  const [availableModels, setAvailableModels] = useState<ModelEntry[]>([]);

  // Load models for pricing metadata
  useEffect(() => {
    let active = true;
    fetchNodeModels()
      .then((models) => {
        if (active) setAvailableModels(models);
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, []);

  const activeModelEntry = useMemo(() => {
    const selectedId = sessionData.model || "llama3.2";
    return availableModels.find((m) => m.id === selectedId) || null;
  }, [availableModels, sessionData.model]);

  const costEstimate: CostEstimateResult = useMemo(() => {
    return estimateInferenceCost({
      text: input,
      modelId: sessionData.model || "llama3.2",
      modelEntry: activeModelEntry,
    });
  }, [input, sessionData.model, activeModelEntry]);

  // Speech Recognition (Voice Input)
  const {
    isSupported: isSpeechRecSupported,
    state: speechRecState,
    startListening,
    stopListening,
    error: speechRecError,
  } = useSpeechRecognition({
    onTranscript: (newTranscript) => {
      // Voice input safety: ONLY populates input field, NEVER auto-submits
      setInput((prev) => {
        const trimmed = prev.trim();
        return trimmed ? `${trimmed} ${newTranscript}` : newTranscript;
      });
    },
  });

  // Speech Synthesis (Read-Aloud)
  const {
    isSupported: isSpeechSynthSupported,
    isSpeaking,
    speak,
    stop: stopSpeaking,
  } = useSpeechSynthesis({
    onEnd: () => setActiveSpeechMessageIndex(null),
    onError: () => setActiveSpeechMessageIndex(null),
  });

  // Stop speech if conversation or active session changes
  useEffect(() => {
    return () => {
      stopSpeaking();
      stopListening();
    };
  }, [sessionData.id, stopSpeaking, stopListening]);

  // Keep current time updated for live expiry tracking
  useEffect(() => {
    const timer = setInterval(() => {
      setNowSec(Math.floor(Date.now() / 1000));
    }, 1000);
    return () => clearInterval(timer);
  }, []);

  const expiryInfo: Bolt11ExpiryInfo | null = useMemo(() => {
    return parseBolt11Expiry(sessionData.invoice, nowSec);
  }, [sessionData.invoice, nowSec]);

  useEffect(() => {
    fetchNodeHealth()
      .then((health) => {
        if (health.lightning) {
          setNodeLightning(health.lightning);
        }
      })
      .catch(() => {
        // Node metadata unavailable; fallback to invoice heuristics
      });
  }, []);

  useEffect(() => {
    if (typeof window !== "undefined") {
      const saved = localStorage.getItem("satsgrid_nwc_uri");
      // This browser-only preference is read after hydration because localStorage is unavailable on the server.
      // eslint-disable-next-line react-hooks/set-state-in-effect
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
    } catch {
      setPreimageError("Mock payment was unavailable. The pending session is unchanged.");
    } finally {
      setIsPayingMock(false);
    }
  };

  const isInputDisabled =
    sessionStatus === "idle" ||
    sessionStatus === "creating" ||
    sessionStatus === "payment_required" ||
    sessionStatus === "authorizing" ||
    sessionStatus === "expired" ||
    sessionStatus === "exhausted" ||
    sessionStatus === "closed" ||
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

  const handleCopyInvoice = async () => {
    if (!sessionData.invoice) return;
    try {
      if (navigator.clipboard && window.isSecureContext) {
        await navigator.clipboard.writeText(sessionData.invoice);
      } else {
        // Fallback for non-https/legacy browser contexts
        const textArea = document.createElement("textarea");
        textArea.value = sessionData.invoice;
        textArea.style.position = "fixed";
        textArea.style.left = "-999999px";
        textArea.style.top = "-999999px";
        document.body.appendChild(textArea);
        textArea.focus();
        textArea.select();
        document.execCommand("copy");
        textArea.remove();
      }
      setCopyFeedback("success");
      setTimeout(() => {
        setCopyFeedback("idle");
      }, 2500);
    } catch (err) {
      console.error("Failed to copy invoice to clipboard", err);
      setCopyFeedback("error");
      setTimeout(() => setCopyFeedback("idle"), 3000);
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
      } catch {
        setNwcError("Wallet payment was cancelled or failed. The pending session is unchanged.");
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
      setManualPreimage("");
    }
  };

  const invoiceStr = sessionData.invoice || "";
  const networkInfo = resolveNetworkIdentity(nodeLightning, invoiceStr);
  const networkName = networkInfo.displayName;

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
            <div className="flex flex-col gap-3 mt-1 w-full max-w-xl">
              <div className="flex items-center justify-between">
                <span className="font-semibold text-sm">
                  {sessionStatus === "authorizing" ? "Payment or backend verification in progress" : "L402 Payment Required"}
                </span>
                <span className="text-[11px] font-mono font-medium px-2 py-0.5 rounded bg-lightning/10 text-lightning border border-lightning/20">
                  {networkName}
                </span>
              </div>

              {/* Real Lightning Payment Card */}
              <div className="rounded-xl border border-border bg-card p-5 shadow-lg flex flex-col gap-4">
                {/* Header with Amount & Expiry Timer */}
                <div className="flex items-center justify-between pb-3 border-b border-border/50">
                  <div className="flex flex-col">
                    <span className="text-xs text-muted-foreground uppercase tracking-wider">
                      Session Budget
                    </span>
                    <SatsAmount amount={sessionData.budget_sats} />
                  </div>
                  {expiryInfo && (
                    <div className="flex items-center gap-1.5 px-2.5 py-1 rounded-full text-xs font-mono border bg-background/80">
                      <Clock className={`w-3.5 h-3.5 ${expiryInfo.isExpired ? "text-destructive" : "text-lightning"}`} />
                      <span className={expiryInfo.isExpired ? "text-destructive font-semibold" : "text-muted-foreground"}>
                        {expiryInfo.isExpired ? "Invoice Expired" : `Expires in ${formatSecondsRemaining(expiryInfo.secondsRemaining)}`}
                      </span>
                    </div>
                  )}
                </div>

                {/* Payment Status State Banner */}
                <div className="flex items-center gap-2.5 p-3 rounded-lg border text-xs font-mono">
                  {sessionStatus === "authorizing" ? (
                    <>
                      <RefreshCw className="w-4 h-4 text-lightning animate-spin shrink-0" />
                      <div className="flex flex-col">
                        <span className="font-semibold text-lightning">Verifying Payment with SatsGrid Node...</span>
                        <span className="text-[10px] text-muted-foreground">Waiting for backend settlement confirmation.</span>
                      </div>
                    </>
                  ) : expiryInfo?.isExpired ? (
                    <>
                      <AlertCircle className="w-4 h-4 text-destructive shrink-0" />
                      <div className="flex flex-col">
                        <span className="font-semibold text-destructive">Invoice has expired</span>
                        <span className="text-[10px] text-muted-foreground">Create a fresh session to generate a new invoice.</span>
                      </div>
                    </>
                  ) : (
                    <>
                      <div className="w-2.5 h-2.5 rounded-full bg-lightning animate-pulse shrink-0" />
                      <div className="flex flex-col">
                        <span className="font-semibold text-foreground">Awaiting Payment</span>
                        <span className="text-[10px] text-muted-foreground">Scan with a mobile wallet or pay directly via WebLN / NWC.</span>
                      </div>
                    </>
                  )}
                </div>

                {/* QR Code Presentation */}
                {sessionData.invoice && (
                  <div className="flex flex-col items-center gap-2 p-3 bg-background/60 border border-border/60 rounded-xl">
                    <div className="flex items-center justify-between w-full px-1">
                      <span className="text-xs font-medium text-foreground flex items-center gap-1.5">
                        <QrCode className="w-3.5 h-3.5 text-lightning" />
                        Scan to Pay
                      </span>
                      <button
                        type="button"
                        onClick={() => setShowQr((v) => !v)}
                        className="text-[11px] text-muted-foreground hover:text-foreground font-mono"
                      >
                        {showQr ? "Hide QR" : "Show QR"}
                      </button>
                    </div>
                    {showQr && (
                      <div className="flex flex-col items-center py-2">
                        <LightningQrCode
                          invoice={sessionData.invoice}
                          size={190}
                          className="transition-all hover:scale-[1.02]"
                        />
                        <span className="text-[10px] font-mono text-muted-foreground mt-2">
                          Compatible with Phoenix, Zeus, Breez, Alby Go & Cash App
                        </span>
                      </div>
                    )}
                  </div>
                )}

                {/* BOLT11 Invoice Text & Copy Action */}
                <div className="flex flex-col gap-2">
                  <div className="flex items-center justify-between">
                    <span className="text-xs text-muted-foreground uppercase tracking-wider">
                      BOLT11 Invoice
                    </span>
                    <button
                      onClick={handleCopyInvoice}
                      disabled={!sessionData.invoice}
                      className="text-xs text-lightning hover:underline font-mono flex items-center gap-1 transition-colors min-h-[32px] px-2 py-1"
                      aria-label={copyFeedback === "success" ? "Invoice copied to clipboard" : "Copy BOLT11 invoice"}
                    >
                      {copyFeedback === "success" ? (
                        <>
                          <Check className="w-3.5 h-3.5 text-success" aria-hidden="true" />
                          <span className="text-success font-semibold" role="status" aria-live="polite">Copied!</span>
                        </>
                      ) : copyFeedback === "error" ? (
                        <>
                          <AlertCircle className="w-3.5 h-3.5 text-destructive" aria-hidden="true" />
                          <span className="text-destructive font-semibold" role="alert" aria-live="assertive">Copy failed</span>
                        </>
                      ) : (
                        <>
                          <Copy className="w-3.5 h-3.5" aria-hidden="true" />
                          <span>Copy Invoice</span>
                        </>
                      )}
                    </button>
                  </div>
                  <div
                    onClick={handleCopyInvoice}
                    className="p-3 bg-background border border-border rounded-lg text-[11px] font-mono text-muted-foreground break-all max-h-24 overflow-y-auto cursor-pointer hover:border-lightning/50 transition-colors select-all"
                    title="Click to copy complete invoice"
                    aria-label="BOLT11 invoice string"
                  >
                    {sessionData.invoice}
                  </div>
                </div>

                {/* WebLN Primary Payment Action */}
                <Button
                  className="w-full bg-lightning text-lightning-foreground hover:bg-lightning/90 font-bold py-2.5 flex items-center justify-center gap-2"
                  onClick={onPayInvoice}
                  disabled={sessionStatus === "authorizing" || isPayingNwc || isPayingMock || !sessionData.invoice || expiryInfo?.isExpired}
                >
                  <Zap className="w-4 h-4" />
                  {sessionStatus === "authorizing" ? (isStreaming ? "Verifying with backend..." : "Processing WebLN...") : "Pay with WebLN"}
                </Button>

                {/* 1-Click Local Mock Simulator Settlement */}
                <Button
                  variant="outline"
                  className="w-full font-mono text-xs border-success/40 text-success hover:bg-success/10 py-2.5 flex items-center justify-center gap-1.5"
                  onClick={handleMockPay}
                  disabled={isPayingMock || sessionStatus === "authorizing" || expiryInfo?.isExpired}
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
                      disabled={expiryInfo?.isExpired}
                    />
                    <Button
                      size="sm"
                      onClick={handleNwcSubmit}
                      disabled={isPayingNwc || sessionStatus === "authorizing" || expiryInfo?.isExpired}
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
                      type="password"
                      placeholder="64-character hex preimage..."
                      value={manualPreimage}
                      onChange={(e) => setManualPreimage(e.target.value)}
                      className="flex-1 bg-background border border-input rounded-md px-3 py-1.5 text-xs font-mono text-foreground placeholder:text-muted-foreground/50 focus:outline-none focus:ring-1 focus:ring-lightning"
                      disabled={expiryInfo?.isExpired}
                    />
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={handleVerifyManualPreimage}
                      disabled={expiryInfo?.isExpired}
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
            {(sessionStatus === "payment_submitted" ||
              sessionStatus === "invalid_payment" ||
              sessionStatus === "inference_error" ||
              sessionStatus === "exhausted" ||
              sessionStatus === "expired" ||
              sessionStatus === "closed") && (
              <div
                role="status"
                className={`mx-auto w-full max-w-3xl rounded-xl border p-4 text-xs shadow-sm flex flex-col gap-3 ${
                  sessionStatus === "payment_submitted"
                    ? "border-lightning/40 bg-lightning/5 text-foreground"
                    : "border-destructive/40 bg-destructive/5 text-foreground"
                }`}
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="flex items-start gap-2.5">
                    {sessionStatus === "payment_submitted" ? (
                      <RefreshCw className="w-4 h-4 text-lightning animate-spin shrink-0 mt-0.5" />
                    ) : (
                      <AlertTriangle className="w-4 h-4 text-destructive shrink-0 mt-0.5" />
                    )}
                    <div className="flex flex-col gap-1">
                      <span className="font-semibold text-sm">
                        {sessionStatus === "exhausted"
                          ? "Session Budget Exhausted"
                          : sessionStatus === "expired"
                          ? "Session Expired"
                          : sessionStatus === "closed"
                          ? "Session Closed"
                          : sessionStatus === "invalid_payment"
                          ? "Invalid Payment Proof"
                          : sessionStatus === "payment_submitted"
                          ? "Payment Awaiting Activation"
                          : "Inference Error"}
                      </span>
                      <p className="text-muted-foreground text-xs leading-relaxed">
                        {sessionStatus === "exhausted"
                          ? "Your session balance has been fully consumed. To continue prompting while keeping your conversation history intact, start a new funded session."
                          : sessionStatus === "expired"
                          ? "This session's time-to-live has elapsed. Open a new session to continue sending prompts."
                          : sessionStatus === "closed"
                          ? "This session has been finalized and closed. Create a new session to begin a fresh chat."
                          : sessionStatus === "invalid_payment"
                          ? "The SatsGrid backend rejected the payment preimage. Your session is preserved; please verify the preimage or unlock with a valid proof."
                          : sessionStatus === "payment_submitted"
                          ? "Payment proof is saved locally. Send a prompt to let the node verify the preimage and activate your session."
                          : "Inference failed before the backend confirmed this session. Your payment proof is retained; retry your message."}
                      </p>
                    </div>
                  </div>

                  {/* Contextual Action Button */}
                  {(sessionStatus === "exhausted" ||
                    sessionStatus === "expired" ||
                    sessionStatus === "closed") &&
                    onStartNewSession && (
                      <Button
                        size="sm"
                        onClick={onStartNewSession}
                        className="shrink-0 bg-primary hover:bg-primary/90 text-primary-foreground font-semibold flex items-center gap-1.5 shadow-sm"
                      >
                        <PlusCircle className="w-3.5 h-3.5" />
                        New Session
                      </Button>
                    )}
                </div>

                {/* Balance Metadata summary when available */}
                {(sessionStatus === "exhausted" ||
                  sessionStatus === "expired" ||
                  sessionStatus === "closed") && (
                  <div className="flex items-center gap-3 pt-2 border-t border-border/40 text-[11px] font-mono text-muted-foreground">
                    <span>
                      Remaining:{" "}
                      <strong className="text-foreground">
                        {sessionData.remaining_sats ?? 0} sats
                      </strong>
                    </span>
                    <span>•</span>
                    <span>
                      Total Budget:{" "}
                      <strong className="text-foreground">
                        {sessionData.budget_sats} sats
                      </strong>
                    </span>
                    <span>•</span>
                    <span>Conversation: {messages.length} messages preserved</span>
                  </div>
                )}
              </div>
            )}
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
                  <div
                    className={`leading-relaxed ${msg.role === "user" ? "bg-muted px-4 py-2 rounded-2xl rounded-tr-sm" : "text-foreground whitespace-pre-wrap"}`}
                    role={msg.role === "assistant" && isStreaming && i === messages.length - 1 ? "status" : undefined}
                    aria-live={msg.role === "assistant" && isStreaming && i === messages.length - 1 ? "polite" : undefined}
                  >
                    {msg.content}
                    {isStreaming && i === messages.length - 1 && msg.role === "assistant" && (
                      <span className="inline-block w-2 h-4 bg-primary animate-pulse ml-1 align-middle" aria-hidden="true" />
                    )}
                  </div>
                  {msg.role === "assistant" && (
                    <div className="mt-1.5 flex flex-wrap items-center gap-2">
                      {isSpeechSynthSupported && !isStreaming && (
                        <button
                          onClick={() => {
                            if (isSpeaking && activeSpeechMessageIndex === i) {
                              stopSpeaking();
                              setActiveSpeechMessageIndex(null);
                            } else {
                              setActiveSpeechMessageIndex(i);
                              speak(msg.content);
                            }
                          }}
                          className={`inline-flex items-center gap-1.5 text-[11px] font-medium px-2 py-0.5 rounded-full transition-colors cursor-pointer border ${
                            isSpeaking && activeSpeechMessageIndex === i
                              ? "bg-primary text-primary-foreground border-primary animate-pulse"
                              : "bg-secondary text-secondary-foreground hover:bg-secondary/80 border-border/50"
                          }`}
                          aria-label={
                            isSpeaking && activeSpeechMessageIndex === i
                              ? "Stop reading response aloud"
                              : "Read response aloud"
                          }
                          title={
                            isSpeaking && activeSpeechMessageIndex === i
                              ? "Stop reading aloud"
                              : "Read response aloud"
                          }
                        >
                          {isSpeaking && activeSpeechMessageIndex === i ? (
                            <>
                              <Square className="w-3 h-3 fill-current" />
                              <span>Stop reading</span>
                            </>
                          ) : (
                            <>
                              <Volume2 className="w-3 h-3" />
                              <span>Read aloud</span>
                            </>
                          )}
                        </button>
                      )}

                      {msg.receipt_id && (
                        <button
                          onClick={() => {
                            setSelectedReceiptId(msg.receipt_id!);
                            setIsReceiptDialogOpen(true);
                          }}
                          className="inline-flex items-center gap-1 text-[11px] font-medium text-primary bg-primary/10 hover:bg-primary/15 border border-primary/20 px-2 py-0.5 rounded-full transition-colors cursor-pointer"
                          title={`Inspect signed receipt ${msg.receipt_id}`}
                          aria-label={`Inspect signed receipt ${msg.receipt_id}`}
                        >
                          <ShieldCheck className="w-3 h-3 text-primary" />
                          <span>Signed Receipt: <span className="font-mono">{msg.receipt_id.slice(0, 14)}...</span></span>
                        </button>
                      )}
                    </div>
                  )}
                </div>
              </div>
            ))}
          </>
        )}
      </div>

      <ReceiptDialog
        receiptId={selectedReceiptId}
        open={isReceiptDialogOpen}
        onOpenChange={setIsReceiptDialogOpen}
      />

      {/* Input Area */}
      <div className="p-4 bg-background border-t border-border/40">
        <div className="max-w-3xl mx-auto relative rounded-xl border border-border bg-card shadow-sm focus-within:ring-1 focus-within:ring-ring">
          <label htmlFor="playground-chat-input" className="sr-only">
            Chat message input
          </label>
          <Textarea 
            id="playground-chat-input"
            aria-label="Chat message input"
            aria-describedby="playground-privacy-notice"
            placeholder={
              sessionStatus === "closed"
                ? "This session is closed. Start a new session to continue chatting."
                : isInputDisabled
                ? "Awaiting active session..."
                : "Send a message (press Enter to send)..."
            }
            disabled={isInputDisabled}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            className="min-h-[80px] w-full resize-none border-0 bg-transparent py-4 pl-4 pr-24 focus-visible:ring-0 rounded-xl disabled:opacity-50 disabled:cursor-not-allowed text-sm"
          />

          <div className="absolute right-3 bottom-3 flex items-center gap-1.5">
            {/* Microphone Voice Input Button */}
            {isSpeechRecSupported ? (
              <Button
                type="button"
                size="icon"
                variant={speechRecState === "listening" ? "destructive" : "ghost"}
                onClick={() => {
                  if (speechRecState === "listening") {
                    stopListening();
                  } else {
                    startListening();
                  }
                }}
                disabled={isInputDisabled}
                className={`h-8 w-8 rounded-md transition-all ${
                  speechRecState === "listening" ? "animate-pulse ring-2 ring-destructive/50" : ""
                }`}
                aria-label={
                  speechRecState === "listening"
                    ? "Stop voice input listening"
                    : "Start voice input dictation"
                }
                title={
                  speechRecState === "listening"
                    ? "Listening... Click to stop"
                    : "Dictate message with voice"
                }
              >
                {speechRecState === "listening" ? (
                  <MicOff className="w-4 h-4 text-destructive-foreground" />
                ) : (
                  <Mic className="w-4 h-4 text-muted-foreground hover:text-foreground" />
                )}
              </Button>
            ) : null}

            {/* Send Button */}
            <Button 
              size="icon" 
              onClick={handleSend}
              className="h-8 w-8 rounded-md min-h-[32px] min-w-[32px]"
              disabled={isInputDisabled || !input.trim()}
              aria-label="Send message"
              title="Send message"
            >
              <Send className="w-4 h-4" />
            </Button>
          </div>
        </div>

        {/* Real-time Pre-Request Cost Estimator Banner */}
        <div className="max-w-3xl mx-auto mt-2 flex flex-wrap items-center justify-between gap-2 px-1 text-[11px] text-muted-foreground">
          <div className="flex items-center gap-2">
            <span className="flex items-center gap-1 font-mono">
              <span className="text-foreground font-medium">Est. Cost:</span>{" "}
              <span className="text-primary font-semibold">~{costEstimate.minEstimatedSats} sats</span>
            </span>
            <span className="text-border">·</span>
            <span className="font-mono">
              {costEstimate.estimatedPromptTokens} prompt tokens
            </span>
            {costEstimate.rates.isModelSpecific ? (
              <span className="text-[10px] px-1.5 py-0.2 rounded bg-primary/10 text-primary border border-primary/20 font-medium">
                Model Pricing
              </span>
            ) : (
              <span className="text-[10px] text-muted-foreground/80">
                (Base fee: {costEstimate.rates.flatRateSats} sats)
              </span>
            )}
          </div>
          <span className="text-[10px] text-muted-foreground/60 italic" title="Final charge is determined by exact token generation and recorded in the signed inference receipt">
            Hold estimate · Actual charge signed on receipt
          </span>
        </div>

        {/* Voice Recognition Status Feedback */}
        {speechRecState === "listening" && (
          <div
            role="status"
            aria-live="polite"
            className="max-w-3xl mx-auto mt-2 flex items-center justify-between text-xs text-primary bg-primary/10 border border-primary/20 px-3 py-1.5 rounded-lg"
          >
            <div className="flex items-center gap-2">
              <span className="w-2 h-2 rounded-full bg-primary animate-ping" />
              <span>Listening to your voice... (Text will appear above. You must click Send to submit)</span>
            </div>
            <button
              onClick={stopListening}
              className="text-[11px] underline hover:no-underline font-medium cursor-pointer"
            >
              Stop
            </button>
          </div>
        )}

        {speechRecError && (
          <div
            role="alert"
            aria-live="assertive"
            className="max-w-3xl mx-auto mt-2 text-xs text-destructive bg-destructive/10 border border-destructive/20 px-3 py-1.5 rounded-lg"
          >
            {speechRecError}
          </div>
        )}
        
        {/* Privacy Indicator */}
        <div id="playground-privacy-notice" className="flex flex-col items-center justify-center mt-3 gap-1">
          <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <Lock className="w-3 h-3" aria-hidden="true" />
            <span className="font-medium">Privacy by default</span>
          </div>
          <p className="text-[10px] text-muted-foreground/60 uppercase tracking-wider text-center">
            No account · No API key · Prompts are not logged · Voice dictation never auto-submits
          </p>
        </div>
      </div>
    </div>
  );
}
