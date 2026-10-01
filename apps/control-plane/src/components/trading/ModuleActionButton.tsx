"use client";

/**
 * Reusable tenant-scoped module action button (PROMPT 6 §I).
 *
 * Wraps one `moduleControl` action (enable/disable) for one trading
 * module (sniper/copy/polymarket) with the full honest-state handling
 * the trading surfaces require:
 *
 *   - busy: the button is disabled while the request is in flight
 *   - denied: if `available` is false the control is not rendered as
 *     an active button at all — a muted explanation is shown instead,
 *     so the UI never offers an action the chain will refuse
 *   - disable prompts for a reason (kept in the audit trail); an
 *     empty confirm leaves the action un-fired
 *   - failures surface as an inline notice; the caller gets the
 *     resulting `ModuleStatusResponse` via `onDone` so it can refresh
 *
 * The action applies ONLY to the caller's organization — that scope is
 * enforced server-side by the controls endpoint; this component merely
 * invokes it.
 */

import { useState } from "react";
import {
  classifyTradingError,
  customerTrading,
  type ModuleStatusResponse,
  tradingStateMessage,
} from "@/lib/customer-trading-api";

export type ModuleActionModule = "sniper" | "copy" | "polymarket";
export type ModuleActionKind = "enable" | "disable";

interface ModuleActionButtonProps {
  module: ModuleActionModule;
  action: ModuleActionKind;
  label: string;
  /** Whether the controls endpoint advertises this control as available. */
  available: boolean;
  disabled?: boolean;
  /** Called after a successful action with the server's response. */
  onDone?: (response: ModuleStatusResponse) => void;
}

export default function ModuleActionButton({
  module,
  action,
  label,
  available,
  disabled = false,
  onDone,
}: ModuleActionButtonProps) {
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  async function fire() {
    const reason =
      action === "disable"
        ? window.prompt("Reason for disabling (kept in your audit trail):") ?? ""
        : undefined;
    if (action === "disable" && (reason === undefined || reason === null)) return;
    setBusy(true);
    try {
      const response = await customerTrading.moduleControl(module, action, reason);
      setNotice(response.control_result?.detail ?? `Control '${action}' applied.`);
      onDone?.(response);
    } catch (e) {
      setNotice(tradingStateMessage(classifyTradingError(e)));
    } finally {
      setBusy(false);
    }
  }

  if (!available) {
    return (
      <span className="muted" aria-label={`${label} unavailable`}>
        {label}: not available
      </span>
    );
  }

  return (
    <>
      <button type="button" disabled={busy || disabled} onClick={() => void fire()}>
        {busy ? `${label}…` : label}
      </button>
      {notice && (
        <p className="muted" role="status">
          {notice}
        </p>
      )}
    </>
  );
}
