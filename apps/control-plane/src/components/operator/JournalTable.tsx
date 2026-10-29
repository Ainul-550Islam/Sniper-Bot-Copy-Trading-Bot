"use client";

import { useState } from "react";
import {
  rotateJournal,
  type JournalStatus,
  type JournalRotateResponse,
} from "@/lib/api/operator-api";

function getErrorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}

function formatBytes(bytes?: number): string {
  if (bytes === undefined || bytes === null) return "0 B";
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / Math.pow(k, i)).toFixed(2)} ${sizes[i]}`;
}

export interface JournalTableProps {
  status: JournalStatus | null;
  onRefresh?: () => Promise<void> | void;
}

export function JournalTable({ status, onRefresh }: JournalTableProps) {
  const [rotating, setRotating] = useState(false);
  const [rotateResult, setRotateResult] = useState<JournalRotateResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const handleRotate = async () => {
    if (!confirm("Rotate and archive all intent journals (trades, positions, events)?")) return;
    setRotating(true);
    setError(null);
    try {
      const res = await rotateJournal();
      setRotateResult(res);
      await onRefresh?.();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setRotating(false);
    }
  };

  return (
    <div className="stack">
      <div className="row-between">
        <div>
          <h3>JSONL Intent Journal</h3>
          <p className="muted small">
            Low-overhead local intent persistence for trades, positions, and events before settlement.
          </p>
        </div>
        <div>
          <button
            type="button"
            className="primary"
            disabled={rotating || !status?.available}
            onClick={() => void handleRotate()}
          >
            {rotating ? "Rotating…" : "Rotate Journals (POST /api/journal)"}
          </button>
        </div>
      </div>

      {error && (
        <div className="card" style={{ borderLeft: "4px solid var(--bad)" }}>
          <p style={{ color: "var(--bad)", margin: 0 }}>{error}</p>
        </div>
      )}

      {rotateResult && (
        <div
          className="card"
          style={{
            borderLeft: `4px solid ${
              rotateResult.ok ? "var(--good)" : "var(--bad)"
            }`,
          }}
        >
          <p style={{ margin: 0 }}>
            Rotation status: <strong>{rotateResult.ok ? "SUCCESS" : "PARTIAL / FAILED"}</strong>
          </p>
          {rotateResult.rotated.length > 0 && (
            <p className="small muted" style={{ margin: "0.4rem 0 0" }}>
              Archived: {rotateResult.rotated.join(", ")}
            </p>
          )}
          {rotateResult.errors.length > 0 && (
            <p className="small" style={{ color: "var(--bad)", margin: "0.4rem 0 0" }}>
              Errors: {rotateResult.errors.join("; ")}
            </p>
          )}
        </div>
      )}

      <div className="grid-3">
        <div className="card">
          <h4>Trades Journal</h4>
          <p style={{ fontSize: "1.4rem", fontWeight: 700, margin: "0.5rem 0" }}>
            {formatBytes(status?.trades_bytes)}
          </p>
          <span className="tag">Append-only JSONL</span>
        </div>
        <div className="card">
          <h4>Positions Journal</h4>
          <p style={{ fontSize: "1.4rem", fontWeight: 700, margin: "0.5rem 0" }}>
            {formatBytes(status?.positions_bytes)}
          </p>
          <span className="tag">State snapshots</span>
        </div>
        <div className="card">
          <h4>Events Journal</h4>
          <p style={{ fontSize: "1.4rem", fontWeight: 700, margin: "0.5rem 0" }}>
            {formatBytes(status?.events_bytes)}
          </p>
          <span className="tag">WebSocket feed mirror</span>
        </div>
      </div>

      {status?.dir && (
        <div className="card">
          <p className="small muted" style={{ margin: 0 }}>
            Journal Storage Directory: <code>{status.dir}</code>
          </p>
        </div>
      )}
    </div>
  );
}

export default JournalTable;
