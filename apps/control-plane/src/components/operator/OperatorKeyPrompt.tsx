"use client";

/**
 * Unlock form for the operator console (Round 2, defect D1).
 *
 * The operator plane is authenticated by an operator API key, not by the SaaS
 * session: `require_role` resolves `x-api-key` through the operator key
 * registry. Until that key is present in memory, every operator call fails
 * locally with `OperatorKeyRequiredError` — so this form is the only way in.
 *
 * Handling rules:
 * - the input is `type="password"` and is cleared from state immediately after
 *   submit, on success and on failure alike;
 * - the value is never stored (see `lib/operator-session.ts`), never rendered
 *   back, never returned in the `onUnlock` callback;
 * - the backend's 401 `reason` is shown verbatim: "invalid or missing
 *   x-api-key" is the operator's signal that the key was refused.
 */

import { useState } from "react";
import { clearOperatorKey } from "@/lib/operator-session";

export interface OperatorKeyPromptProps {
  /** Called with the typed key exactly once, then the field is cleared. */
  onUnlock: (key: string) => void;
  /** Backend refusal reason to surface (ApiError.reason), when there is one. */
  error?: string | null;
  /** True while an unlock attempt is in flight. */
  busy?: boolean;
  /** Minimum length the backend's key registry accepts. */
  minLength?: number;
}

export function OperatorKeyPrompt({
  onUnlock,
  error = null,
  busy = false,
  minLength = 24,
}: OperatorKeyPromptProps) {
  const [value, setValue] = useState("");

  const submit = (event: React.FormEvent) => {
    event.preventDefault();
    const key = value.trim();
    if (!key || key.length < minLength || busy) return;
    // Clear the field BEFORE handing the value on: from here the plaintext
    // lives only in the in-memory operator store.
    setValue("");
    onUnlock(key);
  };

  return (
    <div className="card stack" style={{ maxWidth: "560px", margin: "2rem auto" }}>
      <h2>Operator Console Locked</h2>
      <p className="muted">
        The operator control plane is authenticated with an operator API key presented as{" "}
        <code>x-api-key</code>. Your tenant session is a separate credential and cannot
        unlock it.
      </p>
      <p className="muted small">
        The key is held in memory for this tab only, is cleared after 15 minutes of
        idleness, and is dropped the moment you sign out. It is never written to
        storage, never echoed back and never sent to a tenant route.
      </p>

      {error && (
        <div className="card" style={{ borderLeft: "4px solid var(--bad)" }}>
          <p style={{ color: "var(--bad)", margin: 0 }}>{error}</p>
        </div>
      )}

      <form onSubmit={submit} className="stack">
        <div className="form-group">
          <label htmlFor="operator-key">Operator API key</label>
          <input
            id="operator-key"
            type="password"
            autoComplete="off"
            autoFocus
            value={value}
            onChange={(event) => setValue(event.target.value)}
            placeholder={`at least ${minLength} characters`}
            disabled={busy}
          />
        </div>
        <div className="row" style={{ gap: "0.5rem" }}>
          <button
            type="submit"
            className="primary"
            disabled={busy || value.trim().length < minLength}
          >
            {busy ? "Unlocking…" : "Unlock operator console"}
          </button>
          <button
            type="button"
            onClick={() => {
              setValue("");
              clearOperatorKey();
            }}
            disabled={busy}
          >
            Lock
          </button>
        </div>
      </form>
    </div>
  );
}

export default OperatorKeyPrompt;
