"use client";

import { useState } from "react";
import { TypedConfirm } from "@/components/lifecycle/TypedConfirm";
import { setMode, type ModeResponse } from "@/lib/api/operator-api";

function getErrorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}

export interface ModeSwitcherProps {
  currentMode: string;
  liveAllowed?: boolean;
  onModeChanged?: (response: ModeResponse) => void;
}

export function ModeSwitcher({ currentMode, liveAllowed, onModeChanged }: ModeSwitcherProps) {
  const [selectedMode, setSelectedMode] = useState<string>(currentMode || "paper");
  const [pendingLive, setPendingLive] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ModeResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const applyMode = async (mode: string) => {
    setBusy(true);
    setError(null);
    try {
      const res = await setMode(mode);
      setResult(res);
      setSelectedMode(res.mode);
      setPendingLive(false);
      onModeChanged?.(res);
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleSelect = (mode: string) => {
    if (mode === "live") {
      setPendingLive(true);
    } else {
      setPendingLive(false);
      void applyMode(mode);
    }
  };

  return (
    <div className="card stack">
      <div>
        <h3>Execution Mode Switcher</h3>
        <p className="muted small">
          Active execution mode controls transaction dispatch across all trading modules.
          Paper simulates orders locally; Simulate runs on-chain RPC simulation; Live dispatches signed mainnet transactions.
        </p>
      </div>

      {error && (
        <div className="card" style={{ borderLeft: "4px solid var(--bad)" }}>
          <p style={{ color: "var(--bad)", margin: 0 }}>{error}</p>
        </div>
      )}

      {result && (
        <div className="card" style={{ borderLeft: "4px solid var(--good)" }}>
          <p style={{ margin: 0 }}>
            Mode updated to <strong>{result.mode}</strong>. Live allowed: {String(result.live_allowed)}.
          </p>
          {result.note && <p className="muted small" style={{ margin: "0.4rem 0 0" }}>{result.note}</p>}
        </div>
      )}

      <div className="grid-3">
        <button
          type="button"
          className={selectedMode === "paper" ? "primary" : ""}
          disabled={busy}
          onClick={() => handleSelect("paper")}
        >
          Paper Mode
        </button>
        <button
          type="button"
          className={selectedMode === "simulate" || selectedMode === "sim" ? "primary" : ""}
          disabled={busy}
          onClick={() => handleSelect("simulate")}
        >
          Simulate Mode
        </button>
        <button
          type="button"
          className={selectedMode === "live" ? "danger" : ""}
          disabled={busy}
          onClick={() => handleSelect("live")}
        >
          Live Trading
        </button>
      </div>

      <div style={{ marginTop: "0.5rem" }}>
        <p className="muted small">
          Current server mode: <strong>{currentMode || "unknown"}</strong>
          {liveAllowed !== undefined && (
            <span style={{ marginLeft: "1rem" }}>
              Live trading allowed by config: <strong>{String(liveAllowed)}</strong>
            </span>
          )}
        </p>
      </div>

      {pendingLive && (
        <div className="card" style={{ marginTop: "1rem" }}>
          <TypedConfirm
            title="CRITICAL: Switch to Live Real-Money Trading"
            description="Live mode permits modules to submit real transactions spending real assets. You must type GO LIVE to confirm."
            expected="GO LIVE"
            confirmLabel="Confirm Live Mode"
            busy={busy}
            onConfirm={() => void applyMode("live")}
          />
          <button
            type="button"
            style={{ marginTop: "0.5rem" }}
            disabled={busy}
            onClick={() => setPendingLive(false)}
          >
            Cancel
          </button>
        </div>
      )}
    </div>
  );
}

export default ModeSwitcher;
