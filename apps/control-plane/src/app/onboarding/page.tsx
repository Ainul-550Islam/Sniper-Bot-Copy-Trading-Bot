"use client";

/**
 * Tenant onboarding projection.
 *
 * Progress is loaded from the authenticated backend. The page does not mark
 * setup steps complete locally because the backend derives them from durable
 * custody, wallet, module, strategy, and trading records.
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import Link from "next/link";
import { customerTrading, type OnboardingState, toDisplayError } from "@/lib/customer-trading-api";

const stepDescriptions = [
  {
    title: "Organization Provisioning",
    description: "The authenticated organization exists and is available to the tenant control plane.",
  },
  {
    title: "Custody Configuration",
    description: "An active custody profile and active signer are required before signing-dependent operations can proceed.",
  },
  {
    title: "Module Enablement",
    description: "At least one tenant module control must explicitly be enabled in the durable control store.",
  },
  {
    title: "Strategy Configuration",
    description: "At least one non-archived strategy must be saved for this organization.",
  },
  {
    title: "Paper Trade Evidence",
    description: "A durable paper-mode trade record must exist for this organization.",
  },
  {
    title: "Live Prerequisites",
    description: "Signer, funded wallet evidence, enabled module, saved strategy, and paper-trade evidence must all be present.",
  },
];

function stepComplete(state: OnboardingState, step: number): boolean {
  switch (step) {
    case 1:
      return state.steps.org_created;
    case 2:
      return state.steps.custody_configured;
    case 3:
      return state.steps.module_enabled;
    case 4:
      return state.steps.strategy_configured;
    case 5:
      return state.steps.paper_trade_executed;
    case 6:
      return state.steps.live_prerequisites_met;
    default:
      return false;
  }
}

export default function OnboardingPage() {
  const [state, setState] = useState<OnboardingState | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadState = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setState(await customerTrading.onboardingState());
    } catch (err: unknown) {
      setState(null);
      setError(toDisplayError(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadState();
  }, [loadState]);

  return (
    <AppShell title="Tenant Onboarding">
      <div style={{ maxWidth: "860px", margin: "0 auto" }}>
        <div style={{ marginBottom: "2rem", textAlign: "center" }}>
          <h1 style={{ margin: 0 }}>Tenant onboarding</h1>
          <p style={{ margin: "0.5rem 0 0", color: "var(--muted)", fontSize: "0.95rem" }}>
            Progress is evidence-based and refreshes from the tenant data plane.
          </p>
        </div>

        {error && (
          <div className="card" style={{ color: "var(--bad)", background: "var(--bad-glow)", marginBottom: "1.5rem" }}>
            <div>{error}</div>
            <button type="button" onClick={() => void loadState()} className="btn btn-secondary" style={{ marginTop: "0.75rem" }}>
              Retry
            </button>
          </div>
        )}

        {loading ? (
          <div className="card">Loading onboarding evidence...</div>
        ) : state ? (
          <>
            <div className="card" style={{ marginBottom: "1.25rem", display: "flex", justifyContent: "space-between", gap: "1rem", alignItems: "center" }}>
              <div>
                <strong>{state.completed ? "Onboarding evidence complete" : `Current evidence step: ${state.current_step} of ${state.steps_total}`}</strong>
                <p style={{ margin: "0.35rem 0 0", color: "var(--muted)", fontSize: "0.85rem" }}>
                  Organization: {state.organization_id}
                </p>
              </div>
              <button type="button" onClick={() => void loadState()} className="btn btn-secondary">
                Refresh evidence
              </button>
            </div>

            <div style={{ display: "flex", flexDirection: "column", gap: "1rem", marginBottom: "1.5rem" }}>
              {stepDescriptions.map((description, index) => {
                const step = index + 1;
                const completed = stepComplete(state, step);
                const active = !state.completed && step === state.current_step;
                return (
                  <div
                    key={step}
                    className="card"
                    style={{
                      borderColor: active ? "var(--accent)" : "var(--line)",
                      background: active ? "var(--panel-2)" : "var(--panel)",
                    }}
                  >
                    <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: "1rem" }}>
                      <div style={{ display: "flex", alignItems: "center", gap: "1rem" }}>
                        <div
                          style={{
                            width: "32px",
                            height: "32px",
                            borderRadius: "50%",
                            background: completed ? "var(--ok)" : active ? "var(--accent)" : "rgba(255,255,255,0.1)",
                            display: "flex",
                            alignItems: "center",
                            justifyContent: "center",
                            fontWeight: 700,
                            fontSize: "0.85rem",
                            flexShrink: 0,
                          }}
                        >
                          {completed ? "✓" : step}
                        </div>
                        <div>
                          <h3 style={{ margin: 0, fontSize: "1rem" }}>{description.title}</h3>
                          <p style={{ margin: "0.2rem 0 0", color: "var(--muted)", fontSize: "0.85rem" }}>{description.description}</p>
                        </div>
                      </div>
                      <span className={`badge ${completed ? "badge-ok" : active ? "badge-warn" : ""}`}>
                        {completed ? "EVIDENCED" : active ? "ACTION REQUIRED" : "PENDING"}
                      </span>
                    </div>
                  </div>
                );
              })}
            </div>

            <div className="card" style={{ marginBottom: "1.5rem" }}>
              <h3 style={{ marginTop: 0 }}>Readiness details</h3>
              <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(210px, 1fr))", gap: "0.75rem", fontSize: "0.85rem" }}>
                <span>Active signer: {state.details.has_active_signer ? "yes" : "not evidenced"}</span>
                <span>Funded wallet: {state.details.has_funded_wallet ? "yes" : "not evidenced"}</span>
                <span>Enabled module: {state.details.has_enabled_module ? "yes" : "not evidenced"}</span>
                <span>Saved strategy: {state.details.has_saved_strategy ? "yes" : "not evidenced"}</span>
              </div>
            </div>
          </>
        ) : null}

        <div className="card" style={{ textAlign: "center", padding: "1.5rem" }}>
          <h3 style={{ margin: 0 }}>Continue configuration</h3>
          <p style={{ color: "var(--muted)", fontSize: "0.85rem", margin: "0.5rem 0 1rem" }}>
            Use the durable strategy and trading surfaces to create the records required by the checklist.
          </p>
          <div style={{ display: "flex", justifyContent: "center", gap: "1rem", flexWrap: "wrap" }}>
            <Link href="/strategies" className="btn btn-secondary">View Strategies</Link>
            <Link href="/trading" className="btn btn-primary">Open Trading Console</Link>
          </div>
        </div>
      </div>
    </AppShell>
  );
}
