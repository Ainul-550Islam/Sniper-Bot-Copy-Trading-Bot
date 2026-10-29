"use client";

import { useState } from "react";
import { TypedConfirm } from "@/components/lifecycle/TypedConfirm";
import {
  killSystem,
  resumeSystem,
  setRiskKillSwitch,
  type RiskKillSwitchResponse,
} from "@/lib/api/operator-api";

function getErrorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}

export interface OperatorKillSwitchPanelProps {
  killSwitchActive: boolean;
  onRefresh?: () => Promise<void> | void;
}

export function KillSwitchPanel({
  killSwitchActive,
  onRefresh,
}: OperatorKillSwitchPanelProps) {
  const [busy, setBusy] = useState(false);
  const [showKillConfirm, setShowKillConfirm] = useState(false);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Scoped switch form state
  const [scope, setScope] = useState("");
  const [engaged, setEngaged] = useState(true);
  const [reason, setReason] = useState("");
  const [scopeResult, setScopeResult] = useState<RiskKillSwitchResponse | null>(null);

  const handleKill = async () => {
    setBusy(true);
    setError(null);
    try {
      const res = await killSystem();
      setStatusMessage(`System emergency stop engaged (kill_switch: ${String(res.kill_switch)})`);
      setShowKillConfirm(false);
      await onRefresh?.();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleResume = async () => {
    setBusy(true);
    setError(null);
    try {
      const res = await resumeSystem();
      setStatusMessage(`System halt cleared and trading resumed (kill_switch: ${String(res.kill_switch)})`);
      await onRefresh?.();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleScopedKillSwitch = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!scope.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const res = await setRiskKillSwitch({
        scope: scope.trim(),
        engaged,
        reason: reason.trim() || undefined,
      });
      setScopeResult(res);
      setStatusMessage(`Scope ${res.scope} updated: outcome=${res.outcome}, active=${String(res.active)}`);
      await onRefresh?.();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="stack">
      {error && (
        <div className="card" style={{ borderLeft: "4px solid var(--bad)" }}>
          <p style={{ color: "var(--bad)", margin: 0 }}>{error}</p>
        </div>
      )}

      {statusMessage && (
        <div className="card" style={{ borderLeft: "4px solid var(--good)" }}>
          <p style={{ margin: 0 }}>{statusMessage}</p>
        </div>
      )}

      {/* Global Kill Switch */}
      <div className="card stack" style={{ border: killSwitchActive ? "2px solid var(--bad)" : "1px solid var(--border)" }}>
        <div className="row-between">
          <div>
            <h3>Global Emergency Stop (/api/kill &amp; /api/resume)</h3>
            <p className="muted small">
              Engaging the global kill switch halts all order submission and triggers emergency cancellation.
            </p>
          </div>
          <div>
            <span className={`tag ${killSwitchActive ? "tag--danger" : "tag--active"}`} style={{ fontSize: "1rem", padding: "0.4rem 0.8rem" }}>
              {killSwitchActive ? "HALTED — TRADING BLOCKED" : "SYSTEM OPERATIONAL"}
            </span>
          </div>
        </div>

        {!killSwitchActive ? (
          <div>
            {!showKillConfirm ? (
              <button
                type="button"
                className="danger"
                style={{ padding: "0.8rem 1.6rem", fontSize: "1.1rem", fontWeight: 700 }}
                onClick={() => setShowKillConfirm(true)}
                disabled={busy}
              >
                ENGAGE EMERGENCY STOP
              </button>
            ) : (
              <div className="card" style={{ marginTop: "1rem" }}>
                <TypedConfirm
                  title="CRITICAL: Engage Global Kill Switch"
                  description="This will immediately stop all automated and manual trading across every module and venue. Type EMERGENCY STOP to proceed."
                  expected="EMERGENCY STOP"
                  confirmLabel="KILL SYSTEM NOW"
                  busy={busy}
                  onConfirm={() => void handleKill()}
                />
                <button
                  type="button"
                  style={{ marginTop: "0.5rem" }}
                  disabled={busy}
                  onClick={() => setShowKillConfirm(false)}
                >
                  Cancel
                </button>
              </div>
            )}
          </div>
        ) : (
          <div>
            <p className="muted small">Emergency stop is active. Resuming clears the halt flag and re-allows order submissions.</p>
            <button
              type="button"
              className="primary"
              disabled={busy}
              onClick={() => void handleResume()}
            >
              Resume Trading System (/api/resume)
            </button>
          </div>
        )}
      </div>

      {/* Scoped Venue / Strategy Kill Switch */}
      <div className="card stack">
        <div>
          <h3>Scoped Risk Kill Switch (/api/risk/kill-switch)</h3>
          <p className="muted small">
            Engage or release specific venue (<code>venue:polymarket</code>) or strategy (<code>strategy:label</code>) kill switches.
          </p>
        </div>

        <form onSubmit={(e) => void handleScopedKillSwitch(e)} className="stack">
          <div className="grid-3">
            <div className="form-group">
              <label htmlFor="scope-input">Scope (venue:&lt;venue&gt; or strategy:&lt;label&gt;)</label>
              <input
                id="scope-input"
                type="text"
                value={scope}
                onChange={(e) => setScope(e.target.value)}
                placeholder="venue:polymarket"
                required
                disabled={busy}
              />
            </div>
            <div className="form-group">
              <label htmlFor="engaged-select">Action</label>
              <select
                id="engaged-select"
                value={engaged ? "true" : "false"}
                onChange={(e) => setEngaged(e.target.value === "true")}
                disabled={busy}
              >
                <option value="true">Engage (Block)</option>
                <option value="false">Release (Unblock)</option>
              </select>
            </div>
            <div className="form-group">
              <label htmlFor="reason-input">Reason</label>
              <input
                id="reason-input"
                type="text"
                value={reason}
                onChange={(e) => setReason(e.target.value)}
                placeholder="Market volatility / circuit breaker"
                disabled={busy}
              />
            </div>
          </div>

          <div>
            <button type="submit" className="primary" disabled={busy || !scope.trim()}>
              Apply Scoped Switch
            </button>
          </div>
        </form>

        {scopeResult && (
          <div className="card" style={{ marginTop: "0.5rem" }}>
            <p className="small" style={{ margin: 0 }}>
              Scope: <code>{scopeResult.scope}</code> | Outcome: <strong>{scopeResult.outcome}</strong> | Active: <strong>{String(scopeResult.active)}</strong>
            </p>
          </div>
        )}
      </div>
    </div>
  );
}

export default KillSwitchPanel;
