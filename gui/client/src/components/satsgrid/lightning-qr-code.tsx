"use client";

import React, { useMemo } from "react";
import qrcode from "qrcode-generator";

interface LightningQrCodeProps {
  invoice: string;
  size?: number;
  className?: string;
}

/**
 * Clean SVG-based Lightning QR Code component.
 * Renders the exact BOLT11 invoice without external network requests or canvas quirks.
 */
export function LightningQrCode({
  invoice,
  size = 200,
  className = "",
}: LightningQrCodeProps) {
  const qrSvg = useMemo(() => {
    if (!invoice || typeof invoice !== "string") return null;
    const cleanInvoice = invoice.trim();
    if (!cleanInvoice) return null;

    try {
      // Type number 0 enables auto-detection of QR version based on length
      // Error correction level 'L' or 'M' is optimal for BOLT11 density
      const qr = qrcode(0, "M");
      qr.addData(cleanInvoice);
      qr.make();
      // createSvgTag returns raw SVG with viewBox and black/white elements
      return qr.createSvgTag({
        scalable: true,
        margin: 2,
      });
    } catch (err) {
      console.error("Failed to generate QR code for invoice:", err);
      return null;
    }
  }, [invoice]);

  if (!qrSvg) {
    return (
      <div
        className={`flex items-center justify-center bg-white/5 border border-dashed border-border rounded-lg text-xs text-muted-foreground ${className}`}
        style={{ width: size, height: size }}
      >
        <span>Invalid invoice</span>
      </div>
    );
  }

  return (
    <div
      className={`p-3 bg-white rounded-xl shadow-md border border-border/80 flex items-center justify-center ${className}`}
      style={{ width: size, height: size }}
      // eslint-disable-next-line react/no-danger
      dangerouslySetInnerHTML={{ __html: qrSvg }}
    />
  );
}
