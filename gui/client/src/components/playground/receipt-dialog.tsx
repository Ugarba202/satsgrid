"use client";

import * as React from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { getReceipt, type SignedReceipt } from "@/lib/api/receipts";
import { Check, Copy, ShieldCheck, Terminal, AlertCircle, Loader2, ExternalLink } from "lucide-react";
import Link from "next/link";

interface ReceiptDialogProps {
  receiptId: string | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function ReceiptDialog({ receiptId, open, onOpenChange }: ReceiptDialogProps) {
  const [receipt, setReceipt] = React.useState<SignedReceipt | null>(null);
  const [loading, setLoading] = React.useState<boolean>(false);
  const [error, setError] = React.useState<string | null>(null);
  const [copiedField, setCopiedField] = React.useState<string | null>(null);

  React.useEffect(() => {
    if (!open || !receiptId) {
      return;
    }

    let isCancelled = false;
    // Set loading asynchronously to avoid synchronous effect setState cascade
    const timer = setTimeout(() => {
      if (!isCancelled) {
        setLoading(true);
        setError(null);
      }
    }, 0);

    getReceipt(receiptId)
      .then((data) => {
        if (!isCancelled) {
          setReceipt(data);
          setLoading(false);
        }
      })
      .catch((err) => {
        if (!isCancelled) {
          setError(err instanceof Error ? err.message : "Failed to load receipt from node.");
          setLoading(false);
        }
      });

    return () => {
      isCancelled = true;
      clearTimeout(timer);
    };
  }, [open, receiptId]);

  const handleOpenChange = (newOpen: boolean) => {
    if (!newOpen) {
      setReceipt(null);
      setError(null);
      setLoading(false);
    }
    onOpenChange(newOpen);
  };

  const copyToClipboard = (text: string, fieldName: string) => {
    void navigator.clipboard?.writeText(text);
    setCopiedField(fieldName);
    setTimeout(() => {
      setCopiedField((current) => (current === fieldName ? null : current));
    }, 2000);
  };

  const cliCommand = receipt
    ? `satsgrid receipt verify --file receipt.json --provider-key ${receipt.public_key_hex || "<provider-key-hex>"}`
    : receiptId
    ? `satsgrid receipt get ${receiptId} | satsgrid receipt verify`
    : "";

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="w-[calc(100vw-1.5rem)] sm:w-full sm:max-w-xl max-h-[90vh] overflow-y-auto overflow-x-hidden p-3.5 sm:p-5">
        <DialogHeader>
          <div className="flex items-center gap-2">
            <div className="w-8 h-8 rounded-full bg-primary/10 flex items-center justify-center text-primary shrink-0">
              <ShieldCheck className="w-5 h-5" />
            </div>
            <div className="min-w-0 flex-1 pr-6">
              <DialogTitle className="text-base sm:text-lg font-semibold truncate">
                Signed Inference Receipt
              </DialogTitle>
              <DialogDescription className="text-xs text-muted-foreground line-clamp-2 sm:line-clamp-none">
                Cryptographic execution proof signed by the node&apos;s Ed25519 identity.
              </DialogDescription>
            </div>
          </div>
        </DialogHeader>

        {loading && (
          <div className="py-12 flex flex-col items-center justify-center gap-3 text-muted-foreground">
            <Loader2 className="w-6 h-6 animate-spin text-primary" />
            <p className="text-xs">Fetching receipt from node...</p>
          </div>
        )}

        {error && (
          <div className="py-6 flex flex-col items-center justify-center gap-2 text-center px-2">
            <AlertCircle className="w-8 h-8 text-destructive shrink-0" />
            <p className="text-sm font-medium text-destructive">Failed to Load Receipt</p>
            <p className="text-xs text-muted-foreground max-w-sm break-words">{error}</p>
            <Button
              variant="outline"
              size="sm"
              className="mt-3 text-xs"
              onClick={() => {
                if (receiptId) {
                  setLoading(true);
                  setError(null);
                  getReceipt(receiptId)
                    .then((data) => {
                      setReceipt(data);
                      setLoading(false);
                    })
                    .catch((err) => {
                      setError(err instanceof Error ? err.message : "Failed to load receipt.");
                      setLoading(false);
                    });
                }
              }}
            >
              Retry
            </Button>
          </div>
        )}

        {!loading && !error && receipt && (
          <div className="space-y-3.5 text-xs w-full min-w-0">
            {/* Top metadata grid */}
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-2.5 bg-muted/40 p-2.5 sm:p-3 rounded-lg border border-border/50 min-w-0">
              <div className="min-w-0">
                <span className="text-muted-foreground block text-[11px]">Receipt ID</span>
                <div className="flex items-center justify-between gap-1 font-mono font-medium min-w-0">
                  <span className="truncate text-[11px] sm:text-xs" title={receipt.payload.receipt_id}>
                    {receipt.payload.receipt_id}
                  </span>
                  <button
                    onClick={() => copyToClipboard(receipt.payload.receipt_id, "receipt_id")}
                    className="text-muted-foreground hover:text-foreground shrink-0 p-1"
                    title="Copy receipt ID"
                    aria-label="Copy receipt ID"
                  >
                    {copiedField === "receipt_id" ? <Check className="w-3.5 h-3.5 text-green-500" /> : <Copy className="w-3.5 h-3.5" />}
                  </button>
                </div>
              </div>

              <div className="min-w-0">
                <span className="text-muted-foreground block text-[11px]">Request ID</span>
                <div className="flex items-center justify-between gap-1 font-mono font-medium min-w-0">
                  <span className="truncate text-[11px] sm:text-xs" title={receipt.payload.request_id}>
                    {receipt.payload.request_id}
                  </span>
                  <button
                    onClick={() => copyToClipboard(receipt.payload.request_id, "request_id")}
                    className="text-muted-foreground hover:text-foreground shrink-0 p-1"
                    title="Copy request ID"
                    aria-label="Copy request ID"
                  >
                    {copiedField === "request_id" ? <Check className="w-3.5 h-3.5 text-green-500" /> : <Copy className="w-3.5 h-3.5" />}
                  </button>
                </div>
              </div>

              <div className="min-w-0">
                <span className="text-muted-foreground block text-[11px]">Model</span>
                <span className="font-medium truncate block" title={receipt.payload.model}>
                  {receipt.payload.model}
                </span>
              </div>

              <div className="min-w-0">
                <span className="text-muted-foreground block text-[11px]">Execution Status</span>
                <span className="inline-flex items-center gap-1.5 font-medium capitalize">
                  <span className={`w-1.5 h-1.5 rounded-full shrink-0 ${receipt.payload.inference_status === "completed" ? "bg-green-500" : "bg-amber-500"}`} />
                  {receipt.payload.inference_status}
                </span>
              </div>

              <div className="min-w-0">
                <span className="text-muted-foreground block text-[11px]">Actual Charge</span>
                <span className="font-semibold text-primary">{receipt.payload.actual_charge_sats} sats</span>
                {receipt.payload.approved_max_sats !== undefined && (
                  <span className="text-muted-foreground ml-1 text-[11px]">(max: {receipt.payload.approved_max_sats})</span>
                )}
              </div>

              <div className="min-w-0">
                <span className="text-muted-foreground block text-[11px]">Payment Status</span>
                <span className="font-medium capitalize">{receipt.payload.payment_status}</span>
              </div>

              <div className="min-w-0">
                <span className="text-muted-foreground block text-[11px]">Tokens</span>
                <span className="truncate block">
                  Prompt: {receipt.payload.prompt_tokens ?? "—"} | Completion: {receipt.payload.completion_tokens ?? "—"}
                </span>
              </div>

              <div className="min-w-0">
                <span className="text-muted-foreground block text-[11px]">Created At</span>
                <span className="truncate block">{new Date(receipt.payload.created_at * 1000).toLocaleString()}</span>
              </div>
            </div>

            {/* Cryptographic Signing Info */}
            <div className="space-y-2 border border-border/60 rounded-lg p-2.5 sm:p-3 bg-card min-w-0">
              <div className="flex items-center justify-between gap-2">
                <span className="font-medium flex items-center gap-1.5 truncate">
                  <ShieldCheck className="w-3.5 h-3.5 text-primary shrink-0" />
                  <span className="truncate">Cryptographic Signature</span>
                </span>
                <span className="text-[11px] px-2 py-0.5 rounded-full bg-primary/10 text-primary font-medium shrink-0">
                  {receipt.algorithm}
                </span>
              </div>

              <div className="space-y-2 font-mono text-[11px] min-w-0">
                <div className="min-w-0">
                  <span className="text-muted-foreground block text-[11px] mb-0.5">Key ID (Node Identity):</span>
                  <div className="flex items-start justify-between bg-muted/50 px-2 py-1.5 rounded gap-2 min-w-0">
                    <span className="break-all font-mono leading-relaxed select-all min-w-0">{receipt.key_id}</span>
                    <button
                      onClick={() => copyToClipboard(receipt.key_id, "key_id")}
                      className="text-muted-foreground hover:text-foreground shrink-0 p-0.5"
                      title="Copy Key ID"
                      aria-label="Copy Key ID"
                    >
                      {copiedField === "key_id" ? <Check className="w-3.5 h-3.5 text-green-500" /> : <Copy className="w-3.5 h-3.5" />}
                    </button>
                  </div>
                </div>

                <div className="min-w-0">
                  <span className="text-muted-foreground block text-[11px] mb-0.5">Public Key (Hex):</span>
                  <div className="flex items-start justify-between bg-muted/50 px-2 py-1.5 rounded gap-2 min-w-0">
                    <span className="break-all font-mono leading-relaxed select-all min-w-0">{receipt.public_key_hex}</span>
                    <button
                      onClick={() => copyToClipboard(receipt.public_key_hex, "public_key_hex")}
                      className="text-muted-foreground hover:text-foreground shrink-0 p-0.5"
                      title="Copy Public Key"
                      aria-label="Copy Public Key"
                    >
                      {copiedField === "public_key_hex" ? <Check className="w-3.5 h-3.5 text-green-500" /> : <Copy className="w-3.5 h-3.5" />}
                    </button>
                  </div>
                </div>

                <div className="min-w-0">
                  <span className="text-muted-foreground block text-[11px] mb-0.5">Signature (Hex):</span>
                  <div className="flex items-start justify-between bg-muted/50 px-2 py-1.5 rounded gap-2 min-w-0">
                    <span className="break-all font-mono leading-relaxed select-all min-w-0">{receipt.signature_hex}</span>
                    <button
                      onClick={() => copyToClipboard(receipt.signature_hex, "signature_hex")}
                      className="text-muted-foreground hover:text-foreground shrink-0 p-0.5"
                      title="Copy Signature"
                      aria-label="Copy Signature"
                    >
                      {copiedField === "signature_hex" ? <Check className="w-3.5 h-3.5 text-green-500" /> : <Copy className="w-3.5 h-3.5" />}
                    </button>
                  </div>
                </div>
              </div>

              <div className="mt-2 p-2 rounded bg-amber-500/10 border border-amber-500/20 text-[11px] text-amber-600 dark:text-amber-400 leading-normal break-words">
                <strong>Verification note:</strong> This receipt was retrieved from the node and cryptographically signed with the node&apos;s Ed25519 key. Signature verification guarantees that the receipt contents were signed by the holder of the key, but does not independently prove model output honesty or Lightning settlement.
              </div>
            </div>

            {/* Independent CLI Verification */}
            <div className="space-y-1.5 border border-border/60 rounded-lg p-2.5 sm:p-3 bg-muted/20 min-w-0">
              <span className="font-medium flex items-center gap-1.5">
                <Terminal className="w-3.5 h-3.5 text-muted-foreground shrink-0" />
                <span>Independent CLI Verification</span>
              </span>
              <p className="text-[11px] text-muted-foreground">
                Verify this receipt offline or independently using the SatsGrid CLI:
              </p>
              <div className="flex items-center justify-between bg-black/80 text-white font-mono text-[11px] p-2 rounded gap-2 min-w-0">
                <div className="overflow-x-auto whitespace-nowrap scrollbar-thin py-0.5 min-w-0 flex-1">
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

            {/* Action buttons */}
            <div className="flex flex-col-reverse sm:flex-row items-stretch sm:items-center justify-between gap-2 pt-2 min-w-0">
              <div className="flex flex-col sm:flex-row items-stretch sm:items-center gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  className="text-xs flex items-center justify-center gap-1.5 w-full sm:w-auto"
                  onClick={() => copyToClipboard(JSON.stringify(receipt, null, 2), "raw_json")}
                >
                  {copiedField === "raw_json" ? <Check className="w-3.5 h-3.5 text-green-500" /> : <Copy className="w-3.5 h-3.5" />}
                  Copy Raw Receipt JSON
                </Button>
                {receiptId && (
                  <Link href={`/verify/${encodeURIComponent(receiptId)}`} target="_blank" rel="noopener noreferrer">
                    <Button
                      variant="secondary"
                      size="sm"
                      className="text-xs flex items-center justify-center gap-1.5 w-full sm:w-auto"
                    >
                      <ExternalLink className="w-3.5 h-3.5 text-primary" />
                      View Public Verification
                    </Button>
                  </Link>
                )}
              </div>
              <Button
                variant="ghost"
                size="sm"
                className="text-xs w-full sm:w-auto"
                onClick={() => handleOpenChange(false)}
              >
                Close
              </Button>
            </div>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
