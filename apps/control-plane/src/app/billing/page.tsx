"use client";

import { useEffect, useState } from "react";
import { commercial, toDisplayError, BillingStatus, UsageLimits, CommercialState } from "@/lib/commercial";

type State = { billing: BillingStatus | null; usage: UsageLimits | null; commercial: CommercialState | null; loading: boolean; error: string | null };

export default function BillingPage() {
  const [state, setState] = useState<State>({ billing: null, usage: null, commercial: null, loading: true, error: null });

  useEffect(() => {
    let cancelled = false;
    async function load() {
      try {
        const [billing, usage, comm] = await Promise.all([
          commercial.billingStatus().catch(() => null),
          commercial.usageLimits().catch(() => null),
          commercial.commercialState().catch(() => null),
        ]);
        if (!cancelled) setState({ billing, usage, commercial: comm, loading: false, error: null });
      } catch (e) {
        if (!cancelled) setState((s) => ({ ...s, loading: false, error: toDisplayError(e) }));
      }
    }
    load();
    return () => { cancelled = true; };
  }, []);

  if (state.loading) return <main className="card"><p>Loading billing status…</p></main>;
  if (state.error) return <main className="card"><p role="alert" className="error">{state.error}</p></main>;
  if (!state.billing && !state.commercial) return <main className="card"><p className="muted">No billing data available.</p></main>;

  const b = state.billing;
  const u = state.usage;
  const c = state.commercial;

  return (
    <main id="main" className="stack">
      <h1>Billing &amp; Commercial State</h1>
      {b && (
        <section className="card">
          <h2>Plan &amp; Subscription</h2>
          <dl>
            <dt>Plan</dt><dd>{b.plan_code} (v{b.plan_version})</dd>
            <dt>Subscription</dt><dd>{b.subscription_status}</dd>
            <dt>Provider</dt><dd>{b.billing_provider}</dd>
            <dt>Entitlements</dt><dd>{b.entitlements_active ? "active" : "suspended"}</dd>
            <dt>Dunning</dt><dd>{b.dunning_state}{b.grace_until ? ` — grace until ${b.grace_until}` : ""}</dd>
            {b.suspension_reason && <><dt>Suspension</dt><dd>{b.suspension_reason}</dd></>}
            <dt>As of</dt><dd>{b.as_of}</dd>
          </dl>
        </section>
      )}
      {u && (
        <section className="card">
          <h2>Usage &amp; Limits ({u.period})</h2>
          {u.limits.length === 0 ? <p className="muted">No limits configured.</p> : (
            <table>
              <thead><tr><th>Feature</th><th>State</th><th>Current / Limit</th><th>Allows</th></tr></thead>
              <tbody>
                {u.limits.map((l) => (
                  <tr key={l.feature}>
                    <td>{l.feature}</td>
                    <td>{l.state}</td>
                    <td>{l.current} / {l.limit ?? "∞"} {l.remaining != null ? `(${l.remaining} left)` : ""}</td>
                    <td>{l.allows ? "yes" : "no"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <p className="muted small">Plan: {u.plan_code} — as of {u.as_of}</p>
        </section>
      )}
      {c && (
        <section className="card">
          <h2>Commercial State</h2>
          <dl>
            <dt>Lifecycle</dt><dd>{c.lifecycle_status}</dd>
            <dt>Consistent</dt><dd>{c.commercial_consistent ? "yes" : "no — review required"}</dd>
            <dt>Billing</dt><dd>{c.subscription_status} / {c.dunning_state}</dd>
          </dl>
        </section>
      )}
      <p className="muted small">All values are server-authoritative. Provider secrets are never shown.</p>
    </main>
  );
}
