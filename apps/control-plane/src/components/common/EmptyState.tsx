"use client";

import React from "react";
import Link from "next/link";

interface EmptyStateProps {
  title: string;
  description: string;
  actionText?: string;
  actionHref?: string;
  onAction?: () => void;
  icon?: string;
}

export function EmptyState({
  title,
  description,
  actionText,
  actionHref,
  onAction,
  icon = "📂",
}: EmptyStateProps) {
  return (
    <div
      className="card"
      style={{
        textAlign: "center",
        padding: "3.5rem 1.5rem",
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
      }}
    >
      <div style={{ fontSize: "2.5rem", marginBottom: "0.75rem" }}>{icon}</div>
      <h3 style={{ margin: "0 0 0.5rem", fontSize: "1.2rem" }}>{title}</h3>
      <p style={{ color: "var(--muted)", maxWidth: "480px", margin: "0 0 1.5rem", fontSize: "0.9rem" }}>
        {description}
      </p>
      {actionText && actionHref && (
        <Link href={actionHref} className="btn btn-primary">
          {actionText}
        </Link>
      )}
      {actionText && onAction && !actionHref && (
        <button onClick={onAction} className="btn btn-primary">
          {actionText}
        </button>
      )}
    </div>
  );
}

export default EmptyState;
