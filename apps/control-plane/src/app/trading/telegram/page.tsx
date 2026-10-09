"use client";

/**
 * Telegram Tenant Alert Binding & Control Page (PROMPT 5 §K, file 88 & Commercial Readiness).
 *
 * Shows the control-plane module status, manages THIS organization's
 * notification binding (`/api/tenant/telegram/binding`), and provides
 * safe status confirmation.
 */

import { useCallback, useEffect, useState } from "react";
import { AppShell } from "@/components/AppShell";
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
      if (response.binding?.chat_id) {
        setChatId(String(response.binding.chat_id));
      }
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
    void Promise.resolve().then(() => load());
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
      setNotice(response.detail || "Telegram chat binding updated.");
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
      setNotice(response.detail || "Telegram binding removed.");
      setChatId("");
    } catch (e) {
      setNotice(tradingStateMessage(classifyTradingError(e)));
    } finally {
      setBusy(false);
      await load();
    }
  }

  return (
    <AppShell>
      <div className="stack">
        <div className="row-between">
          <div>
            <h1>Telegram Notification &amp; Alert Dispatch</h1>
            <p className="muted">
              Receive real-time execution alerts, risk trigger warnings, and daily PnL summaries in your private Telegram channel.
            </p>
          </div>
        </div>

        {/* Status Alerts */}
        {state.kind !== "loading" && state.kind !== "ready" && (
          <div className="notice warn" role="alert">
            <p>{tradingStateMessage(state)}</p>
            {isRetryable(state) && (
              <button onClick={() => void load()} className="link" style={{ marginTop: "0.4rem" }}>
                Retry connection
              </button>
            )}
          </div>
        )}

        {notice && <div className="notice success">{notice}</div>}

        <div className="grid-2">
          {/* Status Card */}
          <section className="card">
            <h2>Bot Service Status</h2>
            {status ? (
              <div className="stack" style={{ marginTop: "1rem" }}>
                <p>
                  <strong>Runtime Phase:</strong> <span className={`tag tag--${status.runtime?.phase}`}>{status.runtime?.phase ?? "idle"}</span>
                </p>
                <p>
                  <strong>Runtime Since:</strong> {status.runtime ? new Date(status.runtime.since).toLocaleString() : "—"}
                </p>
                <p>
                  <strong>Entitlement:</strong> Control Plane Notification Service (Included in all tiers).
                </p>
                <p className="muted small">
                  The Telegram dispatcher operates fail-safe: failures to deliver Telegram messages never block live on-chain execution.
                </p>
              </div>
            ) : (
              <p className="muted">Loading status…</p>
            )}
          </section>

          {/* Binding Card */}
          <section className="card">
            <h2>Tenant Chat ID Binding</h2>
            {status?.binding ? (
              <div style={{ marginTop: "0.5rem" }}>
                <p>
                  Active binding: <code>{status.binding.chat_id}</code>
                </p>
                <p className="muted small">
                  Bound at {new Date(status.binding.bound_at).toLocaleString()} by {status.binding.bound_by}
                </p>
              </div>
            ) : (
              <p className="muted" style={{ marginTop: "0.5rem" }}>
                No private Telegram chat configured yet for this organization.
              </p>
            )}

            <div className="form" style={{ marginTop: "1rem" }}>
              <div className="form-group">
                <label htmlFor="chatId">Telegram Chat ID (Numeric)</label>
                <input
                  id="chatId"
                  inputMode="numeric"
                  placeholder="e.g. -1001234567890 or 12345678"
                  value={chatId}
                  onChange={(e) => setChatId(e.target.value)}
                />
                <small className="muted">
                  Message <code>@userinfobot</code> or your team group ID in Telegram to retrieve your chat ID.
                </small>
              </div>

              <div className="row">
                <button className="primary" disabled={busy || !chatId.trim()} onClick={() => void bind()}>
                  {busy ? "Saving…" : "Save Chat Binding"}
                </button>
                {status?.binding && (
                  <button className="danger" disabled={busy} onClick={() => void unbind()}>
                    Unbind Chat
                  </button>
                )}
              </div>
            </div>
          </section>
        </div>
      </div>
    </AppShell>
  );
}
