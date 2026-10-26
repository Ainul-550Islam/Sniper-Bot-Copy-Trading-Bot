"use client";

import { useCallback, useEffect, useState } from "react";
import { AppShell } from "@/components/AppShell";
import { commercial, toDisplayError, type CustodyHealth } from "@/lib/commercial";
import { request } from "@/lib/api";
import { ProfilePanel } from "@/components/custody/ProfilePanel";
import { RotationPanel } from "@/components/custody/RotationPanel";
import { SignerPanel } from "@/components/custody/SignerPanel";
import { sessionStore } from "@/lib/auth";
import {
  custodyApi,
  type CreateCustodyProfileInput,
  type CreateCustodyRotationInput,
  type CreateCustodySignerInput,
  type CustodyProfile as ManagedCustodyProfile,
  type CustodyRotation,
  type CustodySigner as ManagedCustodySigner,
  type SignerHealth,
} from "@/lib/api/custody-api";
import { getCapabilities, type MembershipRole } from "@/lib/permissions";

function currentRole(): MembershipRole {
  const state = sessionStore.getSnapshot();
  const organization =
    state.organizations.find((item) => item.organization_id === state.selectedOrganizationId) ??
    state.organizations[0];
  const role = organization?.role.toLowerCase() ?? "";
  if (role.includes("owner")) return "owner";
  if (role.includes("admin")) return "admin";
  if (role.includes("operator") || role.includes("trader")) return "operator";
  if (role.includes("analyst") || role.includes("auditor")) return "analyst";
  return "viewer";
}

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
  const [, setSessionTick] = useState(0);
  const [managedProfiles, setManagedProfiles] = useState<ManagedCustodyProfile[]>([]);
  const [managedSigners, setManagedSigners] = useState<ManagedCustodySigner[]>([]);
  const [managementError, setManagementError] = useState<string | null>(null);
  const [managementBusy, setManagementBusy] = useState(false);
  const [healthBySigner, setHealthBySigner] = useState<Record<string, SignerHealth>>({});
  const [healthLoadingId, setHealthLoadingId] = useState<string | null>(null);
  const [resolvedSigner, setResolvedSigner] = useState<ManagedCustodySigner | null>(null);
  const [detailedSigner, setDetailedSigner] = useState<ManagedCustodySigner | null>(null);
  const [profileSigners, setProfileSigners] = useState<ManagedCustodySigner[] | null>(null);
  const [profileSignersProfile, setProfileSignersProfile] =
    useState<ManagedCustodyProfile | null>(null);
  const [rotations, setRotations] = useState<CustodyRotation[]>([]);

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
            if (!cancelled) {
              const legacySigners = Array.isArray(s) ? s : s?.signers ?? [];
              // The management loader owns the complete signer inventory;
              // do not let this legacy first-profile read overwrite it if
              // the complete response arrived first.
              setSigners((current) => current ?? legacySigners);
            }
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

  useEffect(() => sessionStore.subscribe(() => setSessionTick((value) => value + 1)), []);
  const canManageCustody = getCapabilities({ role: currentRole() }).canManageTeam;

  const loadManagement = useCallback(async () => {
    try {
      const [profileResponse, signerResponse] = await Promise.all([
        custodyApi.listProfiles(),
        custodyApi.listSigners(),
      ]);
      setManagedProfiles(profileResponse.profiles);
      setManagedSigners(signerResponse.signers);
      setProfiles(profileResponse.profiles);
      setSigners(signerResponse.signers);
    } catch (e) {
      setManagementError(toDisplayError(e));
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(loadManagement);
  }, [loadManagement]);

  async function runManagement<T>(action: () => Promise<T>): Promise<T | null> {
    setManagementBusy(true);
    setManagementError(null);
    // Detail views are server snapshots; a mutation can make them stale, so
    // they must be loaded again explicitly after the management refetch.
    setHealthBySigner({});
    setResolvedSigner(null);
    setDetailedSigner(null);
    setProfileSigners(null);
    setProfileSignersProfile(null);
    try {
      const result = await action();
      await loadManagement();
      return result;
    } catch (e) {
      setManagementError(toDisplayError(e));
      return null;
    } finally {
      setManagementBusy(false);
    }
  }

  async function createManagedProfile(input: CreateCustodyProfileInput) {
    await runManagement(() => custodyApi.createProfile(input));
  }

  async function createManagedSigner(input: CreateCustodySignerInput) {
    await runManagement(() => custodyApi.createSigner(input));
  }

  async function loadSignerHealth(signer: ManagedCustodySigner) {
    setHealthLoadingId(signer.id);
    setManagementError(null);
    try {
      const signerHealth = await custodyApi.signerHealth(signer.id);
      setHealthBySigner((current) => ({ ...current, [signer.id]: signerHealth }));
    } catch (e) {
      setManagementError(toDisplayError(e));
    } finally {
      setHealthLoadingId(null);
    }
  }

  async function resolveSigner(signer: ManagedCustodySigner) {
    setManagementError(null);
    try {
      const response = await custodyApi.resolveSigner(signer.id);
      setResolvedSigner(response.signer);
    } catch (e) {
      setManagementError(toDisplayError(e));
    }
  }

  async function getSigner(signer: ManagedCustodySigner) {
    setManagementError(null);
    try {
      setDetailedSigner(await custodyApi.getSigner(signer.id));
    } catch (e) {
      setManagementError(toDisplayError(e));
    }
  }

  async function loadProfileSigners(profile: ManagedCustodyProfile) {
    setManagementError(null);
    try {
      const response = await custodyApi.listProfileSigners(profile.id);
      setProfileSigners(response.signers);
      setProfileSignersProfile(profile);
    } catch (e) {
      setManagementError(toDisplayError(e));
    }
  }

  function upsertRotation(next: CustodyRotation) {
    setRotations((current) => {
      const existing = current.findIndex((item) => item.id === next.id);
      if (existing === -1) return [next, ...current];
      return current.map((item) => (item.id === next.id ? next : item));
    });
  }

  async function createManagedRotation(input: CreateCustodyRotationInput) {
    const created = await runManagement(() => custodyApi.createRotation(input));
    if (created) upsertRotation(created);
  }

  async function loadManagedRotation(id: string) {
    setManagementBusy(true);
    setManagementError(null);
    try {
      upsertRotation(await custodyApi.getRotation(id));
    } catch (e) {
      setManagementError(toDisplayError(e));
    } finally {
      setManagementBusy(false);
    }
  }

  async function transitionRotation(
    rotationToTransition: CustodyRotation,
    action: "activate" | "revoke",
  ) {
    const transition = await runManagement(() =>
      action === "activate"
        ? custodyApi.activateRotation(rotationToTransition.id)
        : custodyApi.revokeRotation(rotationToTransition.id),
    );
    if (transition) await loadManagedRotation(rotationToTransition.id);
  }

  async function handleRotation(profileId: string, oldId: string, newId: string) {
    setError(null);
    setRotation(null);
    try {
      const res = await commercial.createRotation(profileId, oldId, newId);
      setRotation(`Rotation created: ${res.id} — new signer will be activated after verification. Old signer remains valid until replacement is active.`);
      await loadManagedRotation(res.id);
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

            {managementError && <div className="notice danger" role="alert">{managementError}</div>}

            <ProfilePanel
              profiles={managedProfiles}
              canManage={canManageCustody}
              busy={managementBusy}
              onCreate={createManagedProfile}
              onActivate={async (profile) => {
                await runManagement(() => custodyApi.activateProfile(profile.id));
              }}
              onRevoke={async (profile) => {
                await runManagement(() => custodyApi.revokeProfile(profile.id));
              }}
            />

            <SignerPanel
              profiles={managedProfiles}
              signers={managedSigners}
              healthBySigner={healthBySigner}
              healthLoadingId={healthLoadingId}
              resolvedSigner={resolvedSigner}
              detailedSigner={detailedSigner}
              profileSigners={profileSigners}
              profileSignersProfile={profileSignersProfile}
              canManage={canManageCustody}
              busy={managementBusy}
              onCreate={createManagedSigner}
              onActivate={async (signer) => {
                await runManagement(() => custodyApi.activateSigner(signer.id));
              }}
              onRevoke={async (signer) => {
                await runManagement(() => custodyApi.revokeSigner(signer.id));
              }}
              onAttachCapability={async (signer, capability) => {
                await runManagement(() =>
                  custodyApi.attachCapability(signer.id, { capability }),
                );
              }}
              onResolve={resolveSigner}
              onHealth={loadSignerHealth}
              onGet={getSigner}
              onLoadProfileSigners={loadProfileSigners}
            />

            <RotationPanel
              profiles={managedProfiles}
              signers={managedSigners}
              rotations={rotations}
              canManage={canManageCustody}
              busy={managementBusy}
              onCreate={createManagedRotation}
              onLoad={loadManagedRotation}
              onActivate={async (managedRotation) => {
                await transitionRotation(managedRotation, "activate");
              }}
              onRevoke={async (managedRotation) => {
                await transitionRotation(managedRotation, "revoke");
              }}
            />
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
