"use client";

import { useEffect, useState, use } from "react";
import Link from "next/link";
import { Navbar } from "@/components/layout/navbar";
import { Footer } from "@/components/layout/footer";
import { getReceipt, type SignedReceipt } from "@/lib/api/receipts";
import {
  verifySignedReceipt,
  type ReceiptVerificationResult,
} from "@/lib/crypto/receipt-verify";
import {
  ShieldCheck,
  ShieldAlert,
  ShieldX,
  Copy,
  Check,
  Terminal,
  Loader2,
  FileText,
  Key,
  Calendar,
  Coins,
  Cpu,
  Info,
} from "lucide-react";
import { Button } from "@/components/ui/button";

interface PageProps {
  params: Promise<{ receipt_id: string }>;
}

export default function ReceiptVerifyPage({ params }: PageProps) {
  const resolvedParams = use(params);
  const receiptId = resolvedParams.receipt_id;

  const [receipt, setReceipt] = useState<SignedReceipt | null>(null);
  const [verification, setVerification] = useState<ReceiptVerificationResult | null>(null);
  const [loading, setLoading] = useState(true);
  const [errorType, setErrorType] = useState<
    "not_found" | "network_error" | "malformed" | "unsupported" | null
  >(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [copiedField, setCopiedField] = useState<string | null>(null);
  const [expectedKeyInput, setExpectedKeyInput] = useState<string>("");
  const [activeExpectedKey, setActiveExpectedKey] = useState<string | undefined>(undefined);

  useEffect(() => {
    let isCancelled = false;

    async function loadAndVerify() {
      if (!receiptId || typeof receiptId !== "string" || receiptId.trim().length === 0) {
        setErrorType("not_found");
        setErrorMessage("Receipt ID was not provided in the route.");
        setLoading(false);
        return;
      }

      setLoading(true);
      setErrorType(null);
      setErrorMessage(null);

      try {
        const data = await getReceipt(receiptId.trim());
        if (isCancelled) return;

        setReceipt(data);

        // Run client-side cryptographic verification
        const vResult = verifySignedReceipt(data, activeExpectedKey);
        setVerification(vResult);

        if (vResult.status === "unsupported") {
          setErrorType("unsupported");
          setErrorMessage(vResult.details);
        } else if (vResult.status === "malformed") {
          setErrorType("malformed");
          setErrorMessage(vResult.details);
        }
      } catch (err: unknown) {
        if (isCancelled) return;
        const msg = err instanceof Error ? err.message : String(err);
        if (msg.includes("404") || msg.toLowerCase().includes("not found")) {
          setErrorType("not_found");
          setErrorMessage(`Receipt '${receiptId}' was not found on this SatsGrid node.`);
        } else {
          setErrorType("network_error");
          setErrorMessage(
            msg || "Unable to reach the SatsGrid node or database to retrieve the receipt."
          );
        }
      } finally {
        if (!isCancelled) {
          setLoading(false);
        }
      }
    }

    void loadAndVerify();

    return () => {
      isCancelled = true;
    };
  }, [receiptId, activeExpectedKey]);

  const copyToClipboard = (text: string, fieldName: string) => {
    void navigator.clipboard?.writeText(text);
    setCopiedField(fieldName);
    setTimeout(() => {
      setCopiedField((curr) => (curr === fieldName ? null : curr));
    }, 2000);
  };

  const handleApplyExpectedKey = (e: React.FormEvent) => {
    e.preventDefault();
    const clean = expectedKeyInput.trim();
    setActiveExpectedKey(clean.length > 0 ? clean : undefined);
  };

  const cliCommand = receipt
    ? `satsgrid receipt verify --file receipt.json --provider-key ${receipt.public_key_hex || "<provider-key-hex>"}`
    : `satsgrid receipt get ${receiptId} | satsgrid receipt verify`;

  return (
    <div className="flex flex-col min-h-screen bg-background text-foreground">
      <Navbar />

      <main className="flex-1 max-w-4xl w-full mx-auto px-4 py-8 sm:px-6 sm:py-10">
        {/* Breadcrumb / Nav */}
        <div className="mb-6 flex items-center justify-between text-xs text-muted-foreground">
          <div className="flex items-center gap-2">
            <Link href="/playground" className="hover:text-foreground transition-colors">
              Playground
            </Link>
            <span>/</span>
            <span className="font-mono text-foreground">{receiptId}</span>
          </div>
          <Button
            variant="outline"
            size="sm"
            className="text-xs h-8"
            onClick={() => copyToClipboard(typeof window !== "undefined" ? window.location.href : "", "page_url")}
          >
            {copiedField === "page_url" ? (
              <Check className="w-3.5 h-3.5 text-green-500 mr-1.5" />
            ) : (
              <Copy className="w-3.5 h-3.5 mr-1.5" />
            )}
            Share Verification Link
          </Button>
        </div>

        {/* Loading State */}
        {loading && (
          <div className="py-20 flex flex-col items-center justify-center gap-3 text-muted-foreground rounded-2xl border border-border/50 bg-card p-8">
            <Loader2 className="w-8 h-8 animate-spin text-primary" />
            <h2 className="text-sm font-medium text-foreground">Retrieving Receipt & Verifying Signature</h2>
            <p className="text-xs text-muted-foreground text-center max-w-sm">
              Fetching signed receipt <span className="font-mono text-foreground">{receiptId}</span> from the node and executing independent Ed25519 verification in your browser...
            </p>
          </div>
        )}

        {/* Error States */}
        {!loading && errorType && (
          <div className="py-12 px-6 rounded-2xl border border-destructive/30 bg-destructive/5 text-center flex flex-col items-center justify-center gap-4">
            <div className="w-12 h-12 rounded-full bg-destructive/10 flex items-center justify-center text-destructive">
              <ShieldX className="w-6 h-6" />
            </div>
            <div>
              <h2 className="text-lg font-semibold text-foreground">
                {errorType === "not_found" && "Receipt Not Found"}
                {errorType === "network_error" && "Node Connection Error"}
                {errorType === "malformed" && "Malformed Receipt Structure"}
                {errorType === "unsupported" && "Unsupported Receipt Format"}
              </h2>
              <p className="text-xs text-muted-foreground max-w-md mt-1.5 break-words">
                {errorMessage}
              </p>
            </div>
            <div className="flex items-center gap-3 pt-2">
              <Link href="/playground">
                <Button variant="outline" size="sm">Back to Playground</Button>
              </Link>
              <Button
                size="sm"
                variant="default"
                onClick={() => {
                  setLoading(true);
                  setErrorType(null);
                  setActiveExpectedKey(undefined);
                }}
              >
                Retry
              </Button>
            </div>
          </div>
        )}

        {/* Successfully Loaded & Verified Receipt Content */}
        {!loading && !errorType && receipt && verification && (
          <div className="space-y-6">
            {/* Primary Verification Banner */}
            <div
              className={`p-5 rounded-2xl border flex flex-col sm:flex-row items-start sm:items-center justify-between gap-4 ${
                verification.status === "valid" && verification.identityTrust === "trusted"
                  ? "border-green-500/40 bg-green-500/10 text-green-700 dark:text-green-300"
                  : verification.status === "valid" && verification.identityTrust === "unverified"
                  ? "border-emerald-500/40 bg-emerald-500/10 text-emerald-800 dark:text-emerald-200"
                  : "border-destructive/40 bg-destructive/10 text-destructive dark:text-red-300"
              }`}
            >
              <div className="flex items-start gap-3.5">
                <div
                  className={`w-10 h-10 rounded-full flex items-center justify-center shrink-0 mt-0.5 ${
                    verification.isSignatureValid
                      ? "bg-green-500/20 text-green-600 dark:text-green-400"
                      : "bg-destructive/20 text-destructive"
                  }`}
                >
                  {verification.isSignatureValid ? (
                    <ShieldCheck className="w-6 h-6" />
                  ) : (
                    <ShieldAlert className="w-6 h-6" />
                  )}
                </div>
                <div>
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-semibold text-base text-foreground">
                      {verification.isSignatureValid
                        ? "Ed25519 Signature Verified"
                        : "Cryptographic Verification Failed"}
                    </span>
                    <span
                      className={`text-[11px] px-2 py-0.5 rounded-full font-medium ${
                        verification.isSignatureValid
                          ? "bg-green-500/20 text-green-700 dark:text-green-300 border border-green-500/30"
                          : "bg-destructive/20 text-destructive border border-destructive/30"
                      }`}
                    >
                      {verification.isSignatureValid ? "VALID" : "INVALID"}
                    </span>
                    <span
                      className={`text-[11px] px-2 py-0.5 rounded-full font-medium ${
                        verification.identityTrust === "trusted"
                          ? "bg-blue-500/20 text-blue-700 dark:text-blue-300 border border-blue-500/30"
                          : verification.identityTrust === "mismatch"
                          ? "bg-destructive/20 text-destructive border border-destructive/30"
                          : "bg-amber-500/20 text-amber-800 dark:text-amber-300 border border-amber-500/30"
                      }`}
                    >
                      {verification.identityTrust === "trusted"
                        ? "TRUSTED KEY MATCH"
                        : verification.identityTrust === "mismatch"
                        ? "PROVIDER KEY MISMATCH"
                        : "IDENTITY UNVERIFIED"}
                    </span>
                  </div>
                  <p className="text-xs text-muted-foreground mt-1.5 max-w-xl">
                    {verification.details}
                  </p>
                </div>
              </div>

              <div className="flex items-center gap-2 shrink-0 w-full sm:w-auto justify-end">
                <Button
                  variant="outline"
                  size="sm"
                  className="text-xs h-8"
                  onClick={() => copyToClipboard(JSON.stringify(receipt, null, 2), "raw_json")}
                >
                  {copiedField === "raw_json" ? (
                    <Check className="w-3.5 h-3.5 text-green-500 mr-1" />
                  ) : (
                    <Copy className="w-3.5 h-3.5 mr-1" />
                  )}
                  Export JSON
                </Button>
              </div>
            </div>

            {/* Provider Identity Verification Section */}
            <div className="p-4 rounded-xl border border-border bg-card">
              <div className="flex items-center gap-2 text-sm font-semibold mb-2">
                <Key className="w-4 h-4 text-primary" />
                <span>Provider Identity Verification</span>
              </div>
              <p className="text-xs text-muted-foreground mb-3">
                A valid cryptographic signature proves mathematical authenticity by the key holder, but does not independently prove the identity of the provider. To verify against a known trusted provider, enter their 32-byte Ed25519 public key hex below.
              </p>
              <form onSubmit={handleApplyExpectedKey} className="flex flex-col sm:flex-row gap-2">
                <input
                  type="text"
                  placeholder="Enter expected provider Ed25519 public key (64 hex characters)..."
                  value={expectedKeyInput}
                  onChange={(e) => setExpectedKeyInput(e.target.value)}
                  className="flex-1 bg-background border border-border rounded-lg px-3 py-1.5 text-xs font-mono focus:outline-none focus:ring-1 focus:ring-primary"
                />
                <div className="flex items-center gap-2">
                  <Button type="submit" size="sm" variant="secondary" className="text-xs h-8">
                    Verify Key
                  </Button>
                  {activeExpectedKey && (
                    <Button
                      type="button"
                      size="sm"
                      variant="ghost"
                      className="text-xs h-8"
                      onClick={() => {
                        setExpectedKeyInput("");
                        setActiveExpectedKey(undefined);
                      }}
                    >
                      Clear
                    </Button>
                  )}
                </div>
              </form>
            </div>

            {/* Core Receipt Metadata Grid */}
            <div className="rounded-xl border border-border bg-card overflow-hidden">
              <div className="px-4 py-3 border-b border-border bg-muted/30 flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <FileText className="w-4 h-4 text-muted-foreground" />
                  <span className="text-sm font-semibold">Inference Execution Facts</span>
                </div>
                <span className="text-[11px] font-mono text-muted-foreground">
                  Schema v{receipt.payload.schema_version}
                </span>
              </div>

              <div className="p-4 grid grid-cols-1 md:grid-cols-2 gap-4 text-xs">
                {/* Receipt ID */}
                <div>
                  <span className="text-muted-foreground block text-[11px] mb-1">Receipt ID</span>
                  <div className="flex items-center justify-between p-2 rounded-lg bg-muted/40 font-mono font-medium">
                    <span className="truncate" title={receipt.payload.receipt_id}>
                      {receipt.payload.receipt_id}
                    </span>
                    <button
                      onClick={() => copyToClipboard(receipt.payload.receipt_id, "receipt_id")}
                      className="text-muted-foreground hover:text-foreground ml-1"
                      title="Copy receipt ID"
                      aria-label="Copy receipt ID"
                    >
                      {copiedField === "receipt_id" ? <Check className="w-3.5 h-3.5 text-green-500" /> : <Copy className="w-3.5 h-3.5" />}
                    </button>
                  </div>
                </div>

                {/* Request ID */}
                <div>
                  <span className="text-muted-foreground block text-[11px] mb-1">Request ID</span>
                  <div className="flex items-center justify-between p-2 rounded-lg bg-muted/40 font-mono font-medium">
                    <span className="truncate" title={receipt.payload.request_id}>
                      {receipt.payload.request_id}
                    </span>
                    <button
                      onClick={() => copyToClipboard(receipt.payload.request_id, "request_id")}
                      className="text-muted-foreground hover:text-foreground ml-1"
                      title="Copy request ID"
                      aria-label="Copy request ID"
                    >
                      {copiedField === "request_id" ? <Check className="w-3.5 h-3.5 text-green-500" /> : <Copy className="w-3.5 h-3.5" />}
                    </button>
                  </div>
                </div>

                {/* Model */}
                <div>
                  <span className="text-muted-foreground block text-[11px] mb-1">Model</span>
                  <div className="p-2 rounded-lg bg-muted/40 font-mono font-medium flex items-center gap-1.5">
                    <Cpu className="w-3.5 h-3.5 text-primary" />
                    <span>{receipt.payload.model}</span>
                  </div>
                </div>

                {/* Execution & Payment State */}
                <div>
                  <span className="text-muted-foreground block text-[11px] mb-1">Inference & Payment Status</span>
                  <div className="p-2 rounded-lg bg-muted/40 font-medium flex items-center justify-between">
                    <span className="capitalize">{receipt.payload.inference_status}</span>
                    <span className="px-2 py-0.5 rounded bg-primary/10 text-primary text-[11px] font-mono capitalize">
                      {receipt.payload.payment_status}
                    </span>
                  </div>
                </div>

                {/* Charge & Pricing */}
                <div>
                  <span className="text-muted-foreground block text-[11px] mb-1">Actual Charge</span>
                  <div className="p-2 rounded-lg bg-muted/40 font-medium flex items-center justify-between">
                    <div className="flex items-center gap-1 font-mono text-primary font-bold">
                      <Coins className="w-3.5 h-3.5" />
                      <span>{receipt.payload.actual_charge_sats} {receipt.payload.currency}</span>
                    </div>
                    <span className="text-muted-foreground text-[11px]">
                      Max approved: {receipt.payload.approved_max_sats} {receipt.payload.currency}
                    </span>
                  </div>
                </div>

                {/* Token Usage */}
                <div>
                  <span className="text-muted-foreground block text-[11px] mb-1">Token Metrics</span>
                  <div className="p-2 rounded-lg bg-muted/40 font-mono flex items-center justify-between">
                    <span>Prompt: {receipt.payload.prompt_tokens ?? "N/A"}</span>
                    <span>Completion: {receipt.payload.completion_tokens ?? "N/A"}</span>
                  </div>
                </div>

                {/* Creation Timestamp */}
                <div>
                  <span className="text-muted-foreground block text-[11px] mb-1">Creation Time</span>
                  <div className="p-2 rounded-lg bg-muted/40 flex items-center gap-1.5 text-muted-foreground">
                    <Calendar className="w-3.5 h-3.5" />
                    <span>{new Date(receipt.payload.created_at * 1000).toUTCString()}</span>
                  </div>
                </div>

                {/* Provider ID */}
                <div>
                  <span className="text-muted-foreground block text-[11px] mb-1">Provider ID</span>
                  <div className="p-2 rounded-lg bg-muted/40 font-mono truncate" title={receipt.payload.provider_id}>
                    {receipt.payload.provider_id}
                  </div>
                </div>

                {/* Payment Hash (if available) */}
                {receipt.payload.payment_hash && (
                  <div className="md:col-span-2">
                    <span className="text-muted-foreground block text-[11px] mb-1">Payment Hash</span>
                    <div className="flex items-center justify-between p-2 rounded-lg bg-muted/40 font-mono text-[11px]">
                      <span className="truncate" title={receipt.payload.payment_hash}>
                        {receipt.payload.payment_hash}
                      </span>
                      <button
                        onClick={() => copyToClipboard(receipt.payload.payment_hash!, "payment_hash")}
                        className="text-muted-foreground hover:text-foreground ml-1"
                        title="Copy payment hash"
                        aria-label="Copy payment hash"
                      >
                        {copiedField === "payment_hash" ? <Check className="w-3.5 h-3.5 text-green-500" /> : <Copy className="w-3.5 h-3.5" />}
                      </button>
                    </div>
                  </div>
                )}
              </div>
            </div>

            {/* Cryptographic Signature Container */}
            <div className="rounded-xl border border-border bg-card overflow-hidden">
              <div className="px-4 py-3 border-b border-border bg-muted/30 flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <Key className="w-4 h-4 text-muted-foreground" />
                  <span className="text-sm font-semibold">Cryptographic Signature Proof</span>
                </div>
                <span className="text-[11px] font-mono text-primary font-semibold uppercase">
                  {receipt.algorithm}
                </span>
              </div>

              <div className="p-4 space-y-3 text-xs">
                {/* Public Key */}
                <div>
                  <span className="text-muted-foreground block text-[11px] mb-1">Signing Public Key (Ed25519 Hex)</span>
                  <div className="flex items-center justify-between p-2 rounded-lg bg-muted/40 font-mono text-[11px]">
                    <span className="truncate" title={receipt.public_key_hex}>
                      {receipt.public_key_hex}
                    </span>
                    <button
                      onClick={() => copyToClipboard(receipt.public_key_hex, "pubkey")}
                      className="text-muted-foreground hover:text-foreground ml-1"
                      title="Copy public key"
                      aria-label="Copy public key"
                    >
                      {copiedField === "pubkey" ? <Check className="w-3.5 h-3.5 text-green-500" /> : <Copy className="w-3.5 h-3.5" />}
                    </button>
                  </div>
                </div>

                {/* Signature */}
                <div>
                  <span className="text-muted-foreground block text-[11px] mb-1">Signature (64-byte Hex)</span>
                  <div className="flex items-center justify-between p-2 rounded-lg bg-muted/40 font-mono text-[11px]">
                    <span className="truncate" title={receipt.signature_hex}>
                      {receipt.signature_hex}
                    </span>
                    <button
                      onClick={() => copyToClipboard(receipt.signature_hex, "signature")}
                      className="text-muted-foreground hover:text-foreground ml-1"
                      title="Copy signature"
                      aria-label="Copy signature"
                    >
                      {copiedField === "signature" ? <Check className="w-3.5 h-3.5 text-green-500" /> : <Copy className="w-3.5 h-3.5" />}
                    </button>
                  </div>
                </div>
              </div>
            </div>

            {/* Offline CLI Verification Box */}
            <div className="rounded-xl border border-border/80 bg-muted/20 p-4 space-y-2">
              <div className="flex items-center gap-2 text-xs font-semibold">
                <Terminal className="w-4 h-4 text-muted-foreground" />
                <span>Independent CLI Verification</span>
              </div>
              <p className="text-xs text-muted-foreground">
                Run this command on your terminal with the SatsGrid CLI to verify this receipt offline using the same deterministic Ed25519 verification algorithm:
              </p>
              <div className="flex items-center justify-between bg-black/80 text-white font-mono text-xs p-2.5 rounded-lg gap-2">
                <div className="overflow-x-auto whitespace-nowrap scrollbar-thin py-0.5 flex-1">
                  <code>{cliCommand}</code>
                </div>
                <button
                  onClick={() => copyToClipboard(cliCommand, "cli_command")}
                  className="text-zinc-400 hover:text-white shrink-0 p-1"
                  title="Copy verification command"
                  aria-label="Copy verification command"
                >
                  {copiedField === "cli_command" ? <Check className="w-3.5 h-3.5 text-green-400" /> : <Copy className="w-3.5 h-3.5" />}
                </button>
              </div>
            </div>

            {/* Security Notice / Disclaimer */}
            <div className="p-4 rounded-xl border border-border/60 bg-muted/10 text-xs text-muted-foreground flex items-start gap-3">
              <Info className="w-4 h-4 text-muted-foreground shrink-0 mt-0.5" />
              <div>
                <span className="font-semibold text-foreground">Cryptographic Scope & Trust Boundaries</span>
                <p className="mt-1 leading-relaxed">
                  A verified Ed25519 signature confirms that the provider node possessing this private key issued and signed these exact token, charge, and payment parameters. A cryptographic signature does not independently prove model output honesty or Bitcoin Lightning settlement finality without checking payment preimage proof.
                </p>
              </div>
            </div>
          </div>
        )}
      </main>

      <Footer />
    </div>
  );
}
