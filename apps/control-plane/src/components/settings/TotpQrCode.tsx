"use client";

/**
 * Inline-SVG QR code for the TOTP `otpauth://` enrollment URL.
 *
 * Rendered entirely client-side by `lib/qr` — no image fetch, no CDN, no
 * third-party runtime (the matrix is validated bit-for-bit against the
 * reference `qrcode` package and round-trips through an independent decoder).
 * The SVG gets a quiet zone and crisp squares so phone cameras lock on it
 * from a laptop screen.
 */
import { useMemo } from "react";
import { encodeQr, qrSvgInner } from "@/lib/qr";

interface TotpQrCodeProps {
  /** The `otpauth://totp/…` provisioning URL to encode. */
  value: string;
  /** Rendered pixel size of the SVG square. */
  size?: number;
  /** Accessible label. */
  label?: string;
}

export default function TotpQrCode({ value, size = 180, label }: TotpQrCodeProps) {
  const qr = useMemo(() => {
    try {
      return encodeQr(value);
    } catch {
      return null;
    }
  }, [value]);

  if (!qr) {
    return (
      <div style={{ fontSize: "0.8rem", color: "var(--bad)" }}>
        The provisioning URL is too long to render as a QR code; enter the
        secret manually instead.
      </div>
    );
  }

  const { size: modules, path } = qrSvgInner(qr);
  const quiet = 4; // modules of quiet zone
  const view = modules + quiet * 2;

  return (
    <svg
      width={size}
      height={size}
      viewBox={`0 0 ${view} ${view}`}
      role="img"
      aria-label={label ?? "Authenticator enrollment QR code"}
      style={{ background: "#ffffff", borderRadius: "6px", display: "block" }}
    >
      <g transform={`translate(${quiet} ${quiet})`} fill="#000000">
        <path d={path} />
      </g>
    </svg>
  );
}
