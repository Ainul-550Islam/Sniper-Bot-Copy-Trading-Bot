"use client";

import { useState } from "react";
import { TypedConfirm } from "@/components/lifecycle/TypedConfirm";
import {
  enableModule,
  disableModule,
  type ModuleStatus,
} from "@/lib/api/operator-api";

function getErrorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}

export interface ModuleToggleTableProps {
  modules: ModuleStatus[];
  onRefresh: () => Promise<void> | void;
}

export function ModuleToggleTable({ modules, onRefresh }: ModuleToggleTableProps) {
  const [busyModule, setBusyModule] = useState<string | null>(null);
  const [confirmTarget, setConfirmTarget] = useState<{ name: string; action: "enable" | "disable" } | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const handleAction = async (name: string, action: "enable" | "disable") => {
    setBusyModule(name);
    setActionError(null);
    try {
      if (action === "enable") {
        await enableModule(name);
      } else {
        await disableModule(name);
      }
      setConfirmTarget(null);
      await onRefresh();
    } catch (err) {
      setActionError(getErrorMessage(err));
    } finally {
      setBusyModule(null);
    }
  };

  return (
    <div className="stack">
      {actionError && (
        <div className="card" style={{ borderLeft: "4px solid var(--bad)" }}>
          <p style={{ color: "var(--bad)", margin: 0 }}>{actionError}</p>
        </div>
      )}

      {confirmTarget && (
        <div className="card" style={{ marginBottom: "1rem" }}>
          <TypedConfirm
            title={`Confirm Module ${confirmTarget.action.toUpperCase()}`}
            description={`You are about to ${confirmTarget.action} bot runtime '${confirmTarget.name}'.`}
            expected={`${confirmTarget.action.toUpperCase()} ${confirmTarget.name.toUpperCase()}`}
            confirmLabel={`Confirm ${confirmTarget.action}`}
            busy={busyModule === confirmTarget.name}
            onConfirm={() => void handleAction(confirmTarget.name, confirmTarget.action)}
          />
          <button
            type="button"
            style={{ marginTop: "0.5rem" }}
            onClick={() => setConfirmTarget(null)}
            disabled={busyModule !== null}
          >
            Cancel
          </button>
        </div>
      )}

      <div className="card" style={{ padding: 0, overflowX: "auto" }}>
        <table className="table" style={{ width: "100%", margin: 0 }}>
          <thead>
            <tr>
              <th>Module</th>
              <th>Status</th>
              <th>Health</th>
              <th>Orders (Filled / Total)</th>
              <th>Realized PnL</th>
              <th>Errors</th>
              <th style={{ textAlign: "right" }}>Actions</th>
            </tr>
          </thead>
          <tbody>
            {modules.length === 0 ? (
              <tr>
                <td colSpan={7} className="muted" style={{ textAlign: "center", padding: "2rem" }}>
                  No modules registered in this deployment.
                </td>
              </tr>
            ) : (
              modules.map((m) => (
                <tr key={m.module}>
                  <td>
                    <strong>{m.module}</strong>
                    {m.detail && <p className="muted small" style={{ margin: 0 }}>{m.detail}</p>}
                  </td>
                  <td>
                    <span className={`tag ${m.enabled ? "tag--active" : "tag--suspended"}`}>
                      {m.enabled ? "Enabled" : "Disabled"}
                    </span>
                    {m.running && <span className="tag" style={{ marginLeft: "0.3rem" }}>Running</span>}
                  </td>
                  <td>
                    <span className={`tag ${m.healthy ? "tag--active" : "tag--danger"}`}>
                      {m.healthy ? "Healthy" : "Degraded"}
                    </span>
                  </td>
                  <td>
                    {m.orders_filled} / {m.orders_sent}
                  </td>
                  <td style={{ color: m.realized_pnl >= 0 ? "var(--good)" : "var(--bad)" }}>
                    {m.realized_pnl >= 0 ? "+" : ""}{m.realized_pnl.toFixed(4)}
                  </td>
                  <td>
                    {m.consecutive_errors > 0 ? (
                      <span style={{ color: "var(--bad)", fontWeight: 600 }}>{m.consecutive_errors}</span>
                    ) : (
                      "0"
                    )}
                  </td>
                  <td style={{ textAlign: "right" }}>
                    {m.enabled ? (
                      <button
                        type="button"
                        className="danger"
                        disabled={busyModule === m.module}
                        onClick={() => setConfirmTarget({ name: m.module, action: "disable" })}
                      >
                        Disable
                      </button>
                    ) : (
                      <button
                        type="button"
                        className="primary"
                        disabled={busyModule === m.module}
                        onClick={() => setConfirmTarget({ name: m.module, action: "enable" })}
                      >
                        Enable
                      </button>
                    )}
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

export default ModuleToggleTable;
