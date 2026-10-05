"use client";

import React from "react";

interface ErrorStateProps {
  title?: string;
  error: string;
  onRetry?: () => void;
  correlationId?: string;
}

export function ErrorState({
  title = "Failed to load resource",
  error,
  onRetry,
  correlationId,
}: ErrorStateProps) {
  return (
    <div
      className="card"
      style={{
        borderColor: "rgba(239, 68, 68, 0.4)",
        background: "var(--bad-glow)",
        padding: "1.5rem",
        marginBottom: "1.5rem",
      }}
    >
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start" }}>
        <div>
          <h3 style={{ margin: "0 0 0.5rem", color: "var(--bad)" }}>{title}</h3>
          <p style={{ margin: 0, fontSize: "0.9rem", color: "var(--text)" }}>{error}</p>
          {correlationId && (
            <div style={{ marginTop: "0.5rem", fontSize: "0.75rem", color: "var(--muted)" }}>
              Correlation ID: <code>{correlationId}</code>
            </div>
          )}
        </div>
        {onRetry && (
          <button onClick={onRetry} className="btn btn-secondary" style={{ fontSize: "0.85rem" }}>
            Retry
          </button>
        )}
      </div>
    </div>
  );
}

export default ErrorState;
