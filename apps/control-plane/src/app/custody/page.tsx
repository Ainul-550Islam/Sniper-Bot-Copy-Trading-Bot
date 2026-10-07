"use client";

import { useEffect, useState } from "react";
import { AppShell } from "@/components/AppShell";
import { commercial, toDisplayError, type CustodyHealth } from "@/lib/commercial";
import { request } from "@/lib/api";

interface CustodyProfile { id: string; name: string; provider_type: string; status: string; }
interface SignerView { id: string; public_address: string; provider_type: string; status: string; capabilities: string[]; }

export default function CustodyPage() {
  const [profiles, setProfiles] = useState<CustodyProfile[] | null>(null);
  const [signers, setSigners] = useState<SignerView[] | null>(null);
  const [health, setHealth] = useState<CustodyHealth | null>(null);
  const [rotation, setRotation] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [signerError, setSignerError] = useState<string | null>(null);

  useEffect(() => {
    const abort = new AbortController();
    let cancelled = false;
    async function load() {
      try {
        const p = await request<
          { profiles?: CustodyProfile[]; data?: CustodyProfile[] } | CustodyProfile[]
        >("/api/saas/custody/profiles", { signal: abort.signal });
        if (cancelled) return;
        const list: CustodyProfile[] = Array.isArray(p)
          ? p
          : p?.profiles ?? p?.data ?? [];
        setProfiles(list);

        const h = await commercial.custodyHealth();
        if (cancelled) return;
        setHealth(h);

        const first = list[0];
        if (first) {
          try {
            const s = await request<{ signers?: SignerView[] } | SignerView[]>(
              `/api/saas/custody/profiles/${first.id}/signers`,
              { signal: abort.signal },
            );
            if (!cancelled) setSigners(Array.isArray(s) ? s : s?.signers ?? []);
          } catch (e) {
            if (!cancelled) setSignerError(toDisplayError(e));
          }
        }
        if (!cancelled) setLoading(false);
      } catch (e) {
        if (!cancelled) {
          setError(toDisplayError(e));
          setLoading(false);
        }
      }
    }
    void load();
    return () => {
      cancelled = true;
      abort.abort();
    };
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

  return (
    <AppShell>
      <div className="stack">
        <div className="row-between">
          <div>
            <h1>Custody &amp; Key Management (KMS)</h1>
            <p className="muted">
              Hardware-secured transaction signing (AWS KMS / HashiCorp Vault), signer profiles, and zero-downtime rotation.
            </p>
          </div>
        </div>

        {error && <div className="notice danger">{error}</div>}

        {loading ? (
          <p className="muted">Loading custody profiles and signers…</p>
        ) : (
          <>
            <section className="card">
              <h2>Provider Health &amp; Availability</h2>
              {health ? (
                <ul style={{ marginTop: "0.5rem" }}>
                  {(health.providers ?? []).map((p) => (
                    <li key={p.provider_type} style={{ marginBottom: "0.4rem" }}>
                      <strong>{p.provider_type}</strong>: <span className={`tag tag--${p.signing_allowed ? "healthy" : "warning"}`}>{p.state}</span> — {p.detail}{" "}
                      {p.signing_allowed ? "(signing allowed)" : "(blocked)"}
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="muted" style={{ marginTop: "0.5rem" }}>No health data.</p>
              )}
              <p className="muted small" style={{ marginTop: "0.5rem" }}>
                No private keys or provider credentials are ever displayed. Signing remains blocked until an independent provider reachability probe supplies live evidence.
              </p>
            </section>

            <section className="card">
              <h2>Custody Profiles</h2>
              {!profiles || profiles.length === 0 ? (
                <p className="muted" style={{ marginTop: "0.5rem" }}>No custody profiles registered.</p>
              ) : (
                <ul style={{ marginTop: "0.5rem" }}>
                  {profiles.map((p) => (
                    <li key={p.id} style={{ marginBottom: "0.4rem" }}>
                      <strong>{p.name}</strong> — {p.provider_type} / <span className="tag tag--active">{p.status}</span> — <code className="muted">{p.id}</code>
                    </li>
                  ))}
                </ul>
              )}
            </section>

            <section className="card">
              <h2>Active Signer Inventory</h2>
              {signerError ? <p role="alert" className="error">{signerError}</p> : null}
              {!signers || signers.length === 0 ? (
                <p className="muted" style={{ marginTop: "0.5rem" }}>
                  {signerError ? "Signers could not be read." : "No signers loaded."}
                </p>
              ) : (
                <div style={{ marginTop: "1rem", overflowX: "auto" }}>
                  <table>
                    <thead>
                      <tr>
                        <th>Signer ID</th>
                        <th>Public Address</th>
                        <th>Provider</th>
                        <th>Status</th>
                        <th>Capabilities</th>
                      </tr>
                    </thead>
                    <tbody>
                      {signers.map((s) => (
                        <tr key={s.id}>
                          <td className="muted"><code>{s.id.slice(0, 8)}…</code></td>
                          <td><code>{s.public_address}</code></td>
                          <td>{s.provider_type}</td>
                          <td><span className={`tag tag--${s.status === "active" ? "healthy" : "warning"}`}>{s.status}</span></td>
                          <td>{s.capabilities?.join(", ") || "—"}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
              <p className="muted small" style={{ marginTop: "0.8rem" }}>
                Public address only — private key material never leaves the hardware security module.
              </p>
            </section>

            <section className="card">
              <h2>Zero-Downtime Signer Rotation</h2>
              <p className="muted small">
                Old signer remains valid until the replacement is safely active, ensuring un-interrupted market execution.
              </p>
              {rotation && <div className="notice success" style={{ margin: "0.8rem 0" }}>{rotation}</div>}
              <RotationForm profiles={profiles ?? []} signers={signers ?? []} onSubmit={handleRotation} />
            </section>
          </>
        )}
      </div>
    </AppShell>
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
    return <p className="muted" style={{ marginTop: "0.5rem" }}>No custody profile exists yet — a rotation needs a real profile with at least two signers.</p>;
  }

  return (
    <form onSubmit={submit} className="stack" style={{ marginTop: "1rem" }}>
      <div className="form-row">
        <div className="form-group">
          <label htmlFor="profileId">Custody Profile</label>
          <select id="profileId" required value={profileId} onChange={(e) => setProfileId(e.target.value)}>
            <option value="">— select profile —</option>
            {profiles.map((p) => (
              <option key={p.id} value={p.id}>{p.name} ({p.provider_type}, {p.status})</option>
            ))}
          </select>
        </div>
        <div className="form-group">
          <label htmlFor="oldId">Current Active Signer</label>
          <select id="oldId" required value={oldId} onChange={(e) => setOldId(e.target.value)}>
            <option value="">— select current signer —</option>
            {signers.map((s) => (
              <option key={s.id} value={s.id}>{s.public_address} ({s.status})</option>
            ))}
          </select>
        </div>
        <div className="form-group">
          <label htmlFor="newId">Replacement Signer</label>
          <select id="newId" required value={newId} onChange={(e) => setNewId(e.target.value)}>
            <option value="">— select replacement signer —</option>
            {signers.filter((s) => s.id !== oldId).map((s) => (
              <option key={s.id} value={s.id}>{s.public_address} ({s.status})</option>
            ))}
          </select>
        </div>
      </div>
      <button className="primary" type="submit" disabled={busy || !profileId || !oldId || !newId || oldId === newId}>
        {busy ? "Scheduling Rotation…" : "Execute Signer Rotation"}
      </button>
      <p className="muted small">
        The backend re-verifies that the profile is active and BOTH signers exist, are active,
        and belong to the profile.
      </p>
    </form>
  );
}
