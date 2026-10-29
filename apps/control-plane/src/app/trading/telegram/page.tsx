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
  TelegramConsentResponse,
  TelegramStatusResponse,
  TradingSurfaceState,
  tradingStateMessage,
} from "@/lib/customer-trading-api";

const LAMPORTS_PER_SOL = 1_000_000_000;

function formatLamports(lamports: number): string {
  return `${lamports.toLocaleString()} lamports (${(lamports / LAMPORTS_PER_SOL).toLocaleString()} SOL)`;
}

export default function TelegramPage() {
  const [status, setStatus] = useState<TelegramStatusResponse | null>(null);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });
  const [chatId, setChatId] = useState("");
  const [userId, setUserId] = useState("");
  const [tradingEnabled, setTradingEnabled] = useState(false);
  const [consent, setConsent] = useState<TelegramConsentResponse | null>(null);
  const [consentError, setConsentError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [consentBusy, setConsentBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    setState({ kind: "loading" });
    try {
      const response = await customerTrading.telegramStatus();
      setStatus(response);
      // Deliberately NOT clearing `notice` here: bind/unbind/revoke set the
      // notice and then reload, and wiping it in load() made every success
      // message vanish before it could be read.
      if (response.binding?.chat_id) {
        setChatId(String(response.binding.chat_id));
        setUserId(
          response.binding.telegram_user_id
            ? String(response.binding.telegram_user_id)
            : "",
        );
        setTradingEnabled(response.binding.trading_enabled);
        // Consent is per-chat: only meaningful once a chat is bound. A
        // failure here degrades the consent card, never the whole page.
        try {
          setConsent(await customerTrading.telegramConsent());
          setConsentError(null);
        } catch (e) {
          setConsent(null);
          setConsentError(tradingStateMessage(classifyTradingError(e)));
        }
      } else {
        setConsent(null);
        setConsentError(null);
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
    const trimmedUser = userId.trim();
    const parsedUser = trimmedUser === "" ? undefined : Number(trimmedUser);
    if (
      parsedUser !== undefined &&
      (!Number.isInteger(parsedUser) || parsedUser <= 0)
    ) {
      setNotice("Telegram user id must be a positive integer (or empty).");
      return;
    }
    if (tradingEnabled && parsedUser === undefined) {
      setNotice(
        "Trading requires a Telegram user id: a chat id alone is not an identity.",
      );
      return;
    }
    setBusy(true);
    try {
      const response = await customerTrading.bindTelegram(parsed, {
        telegramUserId: parsedUser,
        tradingEnabled,
      });
      setNotice(response.detail || "Telegram chat binding updated.");
    } catch (e) {
      setNotice(tradingStateMessage(classifyTradingError(e)));
    } finally {
      setBusy(false);
      await load();
    }
  }

  async function revokeConsent() {
    setConsentBusy(true);
    try {
      const response = await customerTrading.revokeTelegramConsent();
      setNotice(
        response.revoked
          ? `Telegram trade consent revoked; ${response.cancelled_pending.length} pending command(s) cancelled.`
          : "No live Telegram trade consent to revoke.",
      );
    } catch (e) {
      setNotice(tradingStateMessage(classifyTradingError(e)));
    } finally {
      setConsentBusy(false);
      await load();
    }
  }

  async function unbind() {
    setBusy(true);
    try {
      const response = await customerTrading.unbindTelegram();
      setNotice(response.detail || "Telegram binding removed.");
      setChatId("");
      setUserId("");
      setTradingEnabled(false);
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
                <p className="muted small">
                  {status.binding.telegram_user_id
                    ? `Linked user ${status.binding.telegram_user_id} — trade console link ${status.binding.trading_enabled ? "allowed" : "off (alerts only)"}.`
                    : "No linked Telegram user — notification-only binding."}
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

              <div className="form-group">
                <label htmlFor="telegramUserId">Telegram User ID (optional, required for trading)</label>
                <input
                  id="telegramUserId"
                  inputMode="numeric"
                  placeholder="e.g. 123456789"
                  value={userId}
                  onChange={(e) => setUserId(e.target.value)}
                />
                <small className="muted">
                  The one Telegram user allowed to act in this chat. A chat id alone is not an identity:
                  without a user id the binding stays notification-only.
                </small>
              </div>

              <div className="form-group">
                <label htmlFor="tradingEnabled">
                  <input
                    id="tradingEnabled"
                    type="checkbox"
                    checked={tradingEnabled}
                    onChange={(e) => setTradingEnabled(e.target.checked)}
                  />{" "}
                  Allow the trade console to link this chat (deployment organization only)
                </label>
                <small className="muted">
                  Linking never trades by itself: every order still needs an in-chat consent,
                  caps and a confirmation. Saving a binding without these fields clears the link.
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

        {/* Trade Consent Card */}
        <section className="card">
          <h2>Telegram Trade Consent</h2>
          <p className="muted small">
            Chat trading is consent-gated: the operator grants consent inside the bound Telegram
            chat by echoing the confirmation phrase. This page can only view and revoke — it can
            never grant. Revoking also cancels every pending command of the chat.
          </p>
          {!status?.binding ? (
            <p className="muted" style={{ marginTop: "0.5rem" }}>
              Bind a Telegram chat first; consent state is tracked per chat.
            </p>
          ) : consentError ? (
            <div className="notice warn" role="alert" style={{ marginTop: "0.5rem" }}>
              <p>Consent state unavailable: {consentError}</p>
            </div>
          ) : consent === null ? (
            <p className="muted" style={{ marginTop: "0.5rem" }}>Loading consent state…</p>
          ) : consent.consent === null ? (
            <p style={{ marginTop: "0.5rem" }}>
              No live consent for chat <code>{consent.chat_id}</code>. Grant it in Telegram with{" "}
              <code>/consent</code> in the bound chat.
            </p>
          ) : (
            <div className="stack" style={{ marginTop: "0.5rem" }}>
              <p>
                <strong>Status:</strong>{" "}
                <span className={`tag tag--${consent.consent.status}`}>{consent.consent.status}</span>
              </p>
              <p>
                <strong>Scope:</strong> {consent.consent.scope.join(", ") || "—"} ·{" "}
                <strong>User:</strong> {consent.consent.user_id}
              </p>
              <p>
                <strong>Per-order cap:</strong> {formatLamports(consent.consent.max_order_lamports)}
              </p>
              <p>
                <strong>Daily cap:</strong> {formatLamports(consent.consent.daily_cap_lamports)}
              </p>
              <p className="muted small">
                Granted {new Date(consent.consent.granted_at).toLocaleString()}, expires{" "}
                {new Date(consent.consent.expires_at).toLocaleString()}
                {consent.consent.revoked_at
                  ? `, revoked ${new Date(consent.consent.revoked_at).toLocaleString()}`
                  : ""}
                .
              </p>
              {consent.consent.status === "active" && (
                <div className="row">
                  <button
                    className="danger"
                    disabled={consentBusy}
                    onClick={() => void revokeConsent()}
                  >
                    {consentBusy ? "Revoking…" : "Revoke consent"}
                  </button>
                </div>
              )}
            </div>
          )}
        </section>
      </div>
    </AppShell>
  );
}
