"use client";

import { useId, useState } from "react";

interface TypedConfirmProps {
  title: string;
  description: string;
  expected: string;
  confirmLabel: string;
  busy: boolean;
  disabled?: boolean;
  onConfirm: () => void;
}

/**
 * Typed destructive-action gate. The confirm button stays disabled until
 * the operator types the exact action word; rendering or typing alone sends
 * no request.
 */
export function TypedConfirm({
  title,
  description,
  expected,
  confirmLabel,
  busy,
  disabled = false,
  onConfirm,
}: TypedConfirmProps) {
  const inputId = useId();
  const [value, setValue] = useState("");
  const confirmed = value === expected;

  return (
    <div style={{ borderLeft: "4px solid var(--bad)", paddingLeft: "1rem" }}>
      <h3>{title}</h3>
      <p className="muted small">{description}</p>
      <div className="form-group">
        <label htmlFor={inputId}>
          Type <code>{expected}</code> to confirm
        </label>
        <input
          id={inputId}
          value={value}
          onChange={(event) => setValue(event.target.value)}
          disabled={disabled || busy}
          autoComplete="off"
        />
      </div>
      <button
        type="button"
        className="danger"
        disabled={disabled || busy || !confirmed}
        onClick={() => {
          setValue("");
          onConfirm();
        }}
      >
        {busy ? "Processing…" : confirmLabel}
      </button>
    </div>
  );
}

export default TypedConfirm;
