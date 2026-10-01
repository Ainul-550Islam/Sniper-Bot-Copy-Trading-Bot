"use client";

/**
 * Telegram tenant binding/status page (PROMPT 5 §K, file 88).
 *
 * Shows the control-plane module status and manages THIS organization's
 * notification binding (`/api/tenant/telegram/binding`). The page states
 * plainly what the binding is and what the deployment currently does
 * with it — no delivery claim that is not true.
 */

import { useCallback, useEffect, useState } from "react";
import {
  classifyTradingError,
  customerTrading,
  isRetryable,
  TelegramStatusResponse,
  TradingSurfaceState,
  tradingStateMessage,
} from "@/lib/customer-trading-api";

export default function TelegramPage() {
  const [status, setStatus] = useState<TelegramStatusResponse | null>(null);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });
  const [chatId, setChatId] = useState("");
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    setState({ kind: "loading" });
    try {
      const response = await customerTrading.telegramStatus();
      setStatus(response);
      setNotice(null);
      if (response.runtime === null) {
        setState({ kind: "stale_runtime", reason: response.runtime_detail ?? "no runtime registered" });
      } else {
        setState({ kind: "ready" });
      }
    } catch (e) {
      setState(classifyTradingError(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function bind() {
    const parsed = Number(chatId.trim());
    if (!Number.isInteger(parsed)) {
      setNotice("Enter an integer Telegram chat id.");
      return;
    }
    setBusy(true);
    try {
      const response = await customerTrading.bindTelegram(parsed);
      setNotice(response.detail);
    } catch (e) {
      setNotice(tradingStateMessage(classifyTradingError(e)));
    } finally {
      setBusy(false);
      await load();
    }
  }

  async function unbind() {
    setBusy(true);
    try {
      const response = await customerTrading.unbindTelegram();
      setNotice(response.detail);
    } catch (e) {
      setNotice(tradingStateMessage(classifyTradingError(e)));
    } finally {
      setBusy(false);
      await load();
    }
  }

  return (
    <main id="main" className="stack">
      <h1>Telegram</h1>
      <section className="card" aria-label="Telegram status">
        <h2>Status</h2>
        {state.kind === "loading" && <p>Loading Telegram status…</p>}
        {state.kind !== "loading" && state.kind !== "ready" && (
          <p role="alert" className="error">
            {tradingStateMessage(state)}
            {isRetryable(state) && (
              <>
                <br />
                <button onClick={() => void load()} className="link">Retry</button>
              </>
            )}
          </p>
        )}
        {state.kind === "ready" && status && (
          <ul>
            <li>
              Runtime phase: <strong>{status.runtime?.phase}</strong> since{" "}
              {status.runtime ? new Date(status.runtime.since).toLocaleString() : "—"}
            </li>
            <li>
              Entitlement: control plane — every authenticated tenant has it; it never trades.
            </li>
          </ul>
        )}
      </section>

      <section className="card" aria-label="Telegram binding">
        <h2>Notification binding</h2>
        {status?.binding ? (
          <p>
            Bound to chat <code>{status.binding.chat_id}</code> since{" "}
            {new Date(status.binding.bound_at).toLocaleString()} (by {status.binding.bound_by}).
          </p>
        ) : (
          <p>No binding is configured for your organization yet.</p>
        )}
        <p className="muted">{status?.binding_detail?.purpose ?? "Records your organization's declared Telegram notification chat."}</p>
        <p className="muted">{status?.binding_detail?.delivery ?? "The deployment-level alert forwarder currently routes to the deployment alert chat; per-tenant routing is a deployment-side integration step."}</p>
        <div className="row">
          <label>
            Chat id{" "}
            <input
              inputMode="numeric"
              placeholder="e.g. -1001234567890"
              value={chatId}
              onChange={(e) => setChatId(e.target.value)}
            />
          </label>
          <button disabled={busy} onClick={() => void bind()}>
            Save binding
          </button>
          {status?.binding && (
            <button disabled={busy} onClick={() => void unbind()}>
              Remove binding
            </button>
          )}
        </div>
        {notice && <p role="status">{notice}</p>}
        <p className="muted">
          The chat id is a public Telegram identifier; no bot token material is ever stored or
          shown here — the token stays in the operator environment.
        </p>
      </section>
    </main>
  );
}
