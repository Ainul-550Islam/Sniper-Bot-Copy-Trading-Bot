"use client";

import { useEffect, useState } from "react";
import { commercial, toDisplayError } from "@/lib/commercial";
import { request } from "@/lib/api";

interface CustodyProfile { id: string; name: string; provider_type: string; status: string; }
interface SignerView { id: string; public_address: string; provider_type: string; status: string; capabilities: string[]; }

export default function CustodyPage() {
  const [profiles, setProfiles] = useState<CustodyProfile[] | null>(null);
  const [signers, setSigners] = useState<SignerView[] | null>(null);
  const [health, setHealth] = useState<any>(null);
  const [rotation, setRotation] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    async function load() {
      try {
        const [p, h] = await Promise.all([
          request<{ profiles?: CustodyProfile[]; data?: CustodyProfile[] } | CustodyProfile[]>("/api/saas/custody/profiles").catch(() => []),
          commercial.custodyHealth().catch(() => null),
        ]);
        const list = Array.isArray(p) ? p : (p as any)?.profiles ?? (p as any)?.data ?? [];
        if (!cancelled) {
          setProfiles(list);
          setHealth(h);
          // Try to load signers for first profile if any
          if (list.length > 0) {
            const first = list[0] as CustodyProfile;
            try {
              const s = await request<{ signers?: SignerView[] } | SignerView[]>(`/api/saas/custody/profiles/${first.id}/signers`).catch(() => null);
              if (s && !cancelled) setSigners(Array.isArray(s) ? s : (s as any)?.signers ?? []);
            } catch { /* ignore */ }
          }
          setLoading(false);
        }
      } catch (e) {
        if (!cancelled) { setError(toDisplayError(e)); setLoading(false); }
      }
    }
    load();
    return () => { cancelled = true; };
  }, []);

  async function handleRotation(oldId: string, newId: string) {
    setError(null);
    setRotation(null);
    try {
      const res = await commercial.createRotation(oldId, newId);
      setRotation(`Rotation created: ${res.id} — new signer will be activated after verification. Old signer remains valid until replacement is active.`);
    } catch (e) {
      setError(toDisplayError(e));
    }
  }

  if (loading) return <main className="card"><p>Loading custody…</p></main>;
  if (error) return <main className="card"><p role="alert" className="error">{error}</p></main>;

  return (
    <main id="main" className="stack">
      <h1>Custody</h1>
      <section className="card">
        <h2>Provider &amp; Health</h2>
        {health ? (
          <ul>
            {(health.providers ?? []).map((p: any) => (
              <li key={p.provider_type}>{p.provider_type}: {p.state} — {p.detail} {p.signing_allowed ? "(signing allowed)" : "(blocked)"}</li>
            ))}
          </ul>
        ) : <p className="muted">No health data.</p>}
        <p className="muted small">No private keys or provider credentials are ever displayed. Unavailable providers fail closed.</p>
      </section>

      <section className="card">
        <h2>Profiles</h2>
        {!profiles || profiles.length === 0 ? <p className="muted">No custody profiles.</p> : (
          <ul>{profiles.map((p) => <li key={p.id}>{p.name} — {p.provider_type} / {p.status} — <span className="muted">{p.id}</span></li>)}</ul>
        )}
      </section>

      <section className="card">
        <h2>Signers</h2>
        {!signers || signers.length === 0 ? <p className="muted">No signers loaded.</p> : (
          <table>
            <thead><tr><th>ID</th><th>Address</th><th>Provider</th><th>Status</th><th>Capabilities</th></tr></thead>
            <tbody>
              {signers.map((s) => (
                <tr key={s.id}>
                  <td className="muted">{s.id.slice(0, 8)}…</td>
                  <td>{s.public_address}</td>
                  <td>{s.provider_type}</td>
                  <td>{s.status}</td>
                  <td>{s.capabilities?.join(", ")}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <p className="muted small">Public address only — private keys are never shown.</p>
      </section>

      <section className="card">
        <h2>Rotation</h2>
        <p className="muted">Old signer remains valid until the replacement is safely active, unless you explicitly use emergency revoke.</p>
        {rotation && <p className="success">{rotation}</p>}
        {/* Simple demo controls — in production these would be modal-confirmed */}
        <button className="primary" onClick={() => {
          const oldId = signers?.[0]?.id ?? "";
          const newId = typeof crypto !== "undefined" && "randomUUID" in crypto ? crypto.randomUUID() : "00000000-0000-0000-0000-000000000000";
          if (!oldId) { setError("No existing signer to rotate"); return; }
          handleRotation(oldId, newId);
        }}>Create rotation (demo)</button>
      </section>
    </main>
  );
}
