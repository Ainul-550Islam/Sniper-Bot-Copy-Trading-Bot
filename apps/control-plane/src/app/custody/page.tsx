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

  async function handleRotation(profileId: string, oldId: string, newId: string) {
    setError(null);
    setRotation(null);
    try {
      const res = await commercial.createRotation(profileId, oldId, newId);
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
        <RotationForm profiles={profiles ?? []} signers={signers ?? []} onSubmit={handleRotation} />
      </section>
    </main>
  );
}

function RotationForm({
  profiles,
  signers,
  onSubmit,
}: {
  profiles: CustodyProfile[];
  signers: SignerView[];
  onSubmit: (profileId: string, oldId: string, newId: string) => Promise<void>;
}) {
  const [profileId, setProfileId] = useState("");
  const [oldId, setOldId] = useState("");
  const [newId, setNewId] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!profileId || !oldId || !newId) return;
    setBusy(true);
    try {
      await onSubmit(profileId, oldId, newId);
    } finally {
      setBusy(false);
    }
  }

  if (profiles.length === 0) {
    return <p className="muted">No custody profile exists yet — a rotation needs a real profile with at least two signers.</p>;
  }

  return (
    <form onSubmit={submit} className="stack">
      <label>
        Profile{" "}
        <select required value={profileId} onChange={(e) => setProfileId(e.target.value)}>
          <option value="">— select profile —</option>
          {profiles.map((p) => (
            <option key={p.id} value={p.id}>{p.name} ({p.provider_type}, {p.status})</option>
          ))}
        </select>
      </label>
      <label>
        Old signer{" "}
        <select required value={oldId} onChange={(e) => setOldId(e.target.value)}>
          <option value="">— select current signer —</option>
          {signers.map((s) => (
            <option key={s.id} value={s.id}>{s.public_address} ({s.status})</option>
          ))}
        </select>
      </label>
      <label>
        New signer{" "}
        <select required value={newId} onChange={(e) => setNewId(e.target.value)}>
          <option value="">— select replacement signer —</option>
          {signers.filter((s) => s.id !== oldId).map((s) => (
            <option key={s.id} value={s.id}>{s.public_address} ({s.status})</option>
          ))}
        </select>
      </label>
      <button className="primary" type="submit" disabled={busy || !profileId || !oldId || !newId || oldId === newId}>
        Create rotation
      </button>
      <p className="muted small">
        The backend re-verifies that the profile is active and BOTH signers exist, are active,
        and belong to the profile — a rotation is never created against synthetic ids.
      </p>
    </form>
  );
}
