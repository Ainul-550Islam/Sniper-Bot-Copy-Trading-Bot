"use client";

import type { SignerHealth } from "@/lib/api/custody-api";

interface HealthBadgeProps {
  health?: SignerHealth;
  loading?: boolean;
}

/**
 * Server-reported signer health. `configuration_only` evidence is shown as
 * configuration state, not dressed up as a live provider-reachability claim.
 */
export function HealthBadge({ health, loading = false }: HealthBadgeProps) {
  if (loading) return <span className="badge">Checking health…</span>;
  if (!health) return <span className="badge">Health not loaded</span>;

  const label = health.signing_allowed
    ? `${health.state} · signing allowed`
    : `${health.state} · signing blocked`;
  return (
    <span
      className={`tag tag--${health.signing_allowed ? "healthy" : "warning"}`}
      title={`${health.detail} Evidence: ${health.evidence_level}. Checked: ${health.checked_at ?? "not probed"}.`}
    >
      {label}
    </span>
  );
}

export default HealthBadge;
