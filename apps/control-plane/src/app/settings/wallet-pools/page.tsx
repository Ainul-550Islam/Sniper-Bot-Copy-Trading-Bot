"use client";

import { useCallback, useEffect, useState } from "react";
import { AppShell } from "@/components/AppShell";
import { MemberManager } from "@/components/wallet-pools/MemberManager";
import { PoolDetailPanel } from "@/components/wallet-pools/PoolDetail";
import { PoolList } from "@/components/wallet-pools/PoolList";
import { toDisplayError, type WalletBindingView, wallets } from "@/lib/api";
import { sessionStore } from "@/lib/auth";
import { walletAccessApi } from "@/lib/api/wallet-access-api";
import { custodyApi, type CustodySigner } from "@/lib/api/custody-api";
import {
  walletPoolsApi,
  type AddWalletPoolMemberInput,
  type CreateWalletPoolInput,
  type UpdateWalletPoolInput,
  type WalletPoolDetail,
  type WalletPoolMember,
  type WalletPoolSummary,
} from "@/lib/api/wallet-pools-api";
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

export default function WalletPoolsPage() {
  const [, setSessionTick] = useState(0);
  const [pools, setPools] = useState<WalletPoolSummary[] | null>(null);
  const [selectedPool, setSelectedPool] = useState<WalletPoolDetail | null>(null);
  const [signers, setSigners] = useState<CustodySigner[]>([]);
  const [loading, setLoading] = useState(true);
  const [detailLoading, setDetailLoading] = useState(false);
  const [creating, setCreating] = useState(false);
  const [mutating, setMutating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [bindings, setBindings] = useState<WalletBindingView[]>([]);
  const [bindingsLoading, setBindingsLoading] = useState(false);
  const [busyAuthorize, setBusyAuthorize] = useState<string | null>(null);
  const [authorizeModule, setAuthorizeModule] = useState<"module.sniper" | "module.copy" | "module.polymarket">("module.sniper");

  useEffect(() => sessionStore.subscribe(() => setSessionTick((value) => value + 1)), []);
  const canManage = getCapabilities({ role: currentRole() }).canManageTeam;

  async function authorize(id: string, module: "module.sniper" | "module.copy" | "module.polymarket") {
    if (!window.confirm(`Authorize wallet binding "${id.slice(0, 8)}…" for ${module}?`)) return;
    setBusyAuthorize(id);
    setError(null);
    try {
      await walletAccessApi.authorize(id, { module });
      await load(selectedPool?.id ?? null);
    } catch (err: unknown) {
      setError(toDisplayError(err));
    } finally {
      setBusyAuthorize(null);
    }
  }

  const loadDetail = useCallback(async (id: string) => {
    setDetailLoading(true);
    try {
      setSelectedPool(await walletPoolsApi.get(id));
    } catch (loadError) {
      setError(toDisplayError(loadError));
    } finally {
      setDetailLoading(false);
    }
  }, []);

  const load = useCallback(async (selectedId?: string | null) => {
    setLoading(true);
    setError(null);
    try {
      const [poolResponse, signerResponse, bindingsResponse] = await Promise.all([
        walletPoolsApi.list(),
        custodyApi.listSigners(),
        wallets.list(),
      ]);
      // Array-shape guards: a non-conforming body (e.g. an unmocked proxy
      // answering `{}`) must render empty server truth, never crash the page.
      const pools = Array.isArray(poolResponse.pools) ? poolResponse.pools : [];
      setPools(pools);
      setSigners(Array.isArray(signerResponse.signers) ? signerResponse.signers : []);
      setBindings(Array.isArray(bindingsResponse.wallets) ? bindingsResponse.wallets : []);
      const nextSelectedId = selectedId ?? pools[0]?.id ?? null;
      if (nextSelectedId) {
        await loadDetail(nextSelectedId);
      } else {
        setSelectedPool(null);
      }
    } catch (loadError) {
      setError(toDisplayError(loadError));
    } finally {
      setLoading(false);
    }
  }, [loadDetail]);

  useEffect(() => {
    void Promise.resolve().then(() => load());
  }, [load]);

  async function create(input: CreateWalletPoolInput) {
    setCreating(true);
    setError(null);
    try {
      const created = await walletPoolsApi.create(input);
      await load(created.id);
    } catch (createError) {
      setError(toDisplayError(createError));
    } finally {
      setCreating(false);
    }
  }

  async function update(input: UpdateWalletPoolInput) {
    if (!selectedPool) return;
    setMutating(true);
    setError(null);
    try {
      await walletPoolsApi.update(selectedPool.id, input);
      await load(selectedPool.id);
    } catch (updateError) {
      setError(toDisplayError(updateError));
    } finally {
      setMutating(false);
    }
  }

  async function addMember(input: AddWalletPoolMemberInput) {
    if (!selectedPool) return;
    setMutating(true);
    setError(null);
    try {
      await walletPoolsApi.addMember(selectedPool.id, input);
      await load(selectedPool.id);
    } catch (addError) {
      setError(toDisplayError(addError));
    } finally {
      setMutating(false);
    }
  }

  async function removeMember(member: WalletPoolMember) {
    if (!selectedPool) return;
    setMutating(true);
    setError(null);
    try {
      await walletPoolsApi.removeMember(selectedPool.id, member.id);
      await load(selectedPool.id);
    } catch (removeError) {
      setError(toDisplayError(removeError));
    } finally {
      setMutating(false);
    }
  }

  return (
    <AppShell>
      <div className="stack">
        <div className="row-between">
          <div>
            <h1>Wallet Pools</h1>
            <p className="muted">
              Named pools of this organization&apos;s custody signers. Membership changes apply
              to new orders; open positions remain with the wallet that opened them.
            </p>
          </div>
          <button type="button" onClick={() => void load(selectedPool?.id)}>Refresh</button>
        </div>

        {error && <div className="notice danger" role="alert">{error}</div>}
        {loading ? (
          <p className="muted">Loading wallet pools…</p>
        ) : (
          <>
            <PoolList
              pools={pools ?? []}
              selectedPoolId={selectedPool?.id ?? null}
              canManage={canManage}
              creating={creating}
              onCreate={create}
              onSelect={(pool) => void loadDetail(pool.id)}
            />

            {detailLoading && <p className="muted">Loading pool details…</p>}
                  {selectedPool && !detailLoading && (
        <>
          <PoolDetailPanel
            key={`${selectedPool.id}:${selectedPool.name}:${selectedPool.allocation}:${selectedPool.status}`}
            pool={selectedPool}
            canManage={canManage}
            busy={mutating}
            onUpdate={update}
          />
          <MemberManager
            pool={selectedPool}
            signers={signers}
            canManage={canManage}
            busy={mutating}
            onAdd={addMember}
            onRemove={removeMember}
          />
        </>
      )}
    </>
  )}

  {/* Wallet access authorization: authorize a wallet binding for a trading module */}
  <section className="card" style={{ marginTop: "1.5rem" }} aria-label="Wallet access authorization">
    <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
      <h3 style={{ margin: 0 }}>Wallet Access Authorization</h3>
      <p className="muted small">
        Authorize a wallet binding (public address only — no key
        material) for a trading module. Authorization is
        destructive-equivalent and requires confirmation.
      </p>
    </div>
    {bindings.length === 0 ? (
      <p className="muted" style={{ padding: "1rem" }}>
        No wallet bindings were returned for this organization.
      </p>
    ) : (
      <div style={{ padding: "1rem" }}>
        <div className="form-group" style={{ display: "flex", gap: "0.75rem", alignItems: "flex-end", flexWrap: "wrap", marginBottom: "0.75rem" }}>
          <label htmlFor="wallet-access-module" style={{ margin: 0, fontSize: "0.85rem", color: "var(--muted)" }}>
            Module
          </label>
          <select
            id="wallet-access-module"
            value={authorizeModule}
            onChange={(event) => setAuthorizeModule(event.target.value as "module.sniper" | "module.copy" | "module.polymarket")}
            disabled={!canManage}
            className="select"
          >
            <option value="module.sniper">Sniper</option>
            <option value="module.copy">Copy Trading</option>
            <option value="module.polymarket">Polymarket</option>
          </select>
        </div>
        <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
          <thead>
            <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
              <th style={{ padding: "0.75rem 1rem" }}>Label</th>
              <th style={{ padding: "0.75rem 1rem" }}>Public Address</th>
              <th style={{ padding: "0.75rem 1rem" }}>Modules</th>
              <th style={{ padding: "0.75rem 1rem" }}>Authorize</th>
            </tr>
          </thead>
          <tbody>
            {bindings.map((b) => (
              <tr key={b.id} style={{ borderTop: "1px solid var(--line)" }}>
                <td style={{ padding: "0.75rem 1rem" }}>{b.label}</td>
                <td style={{ padding: "0.75rem 1rem" }}><code>{b.public_address}</code></td>
                <td style={{ padding: "0.75rem 1rem" }}>{b.modules.join(", ")}</td>
                <td style={{ padding: "0.75rem 1rem" }}>
                  <button
                    type="button"
                    disabled={!canManage || busyAuthorize === b.id}
                    onClick={() => void authorize(b.id, authorizeModule)}
                    className="btn btn-secondary"
                    style={{ fontSize: "0.75rem", padding: "0.2rem 0.5rem" }}
                  >
                    {busyAuthorize === b.id ? "Authorizing…" : "Authorize"}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    )}
  </section>
</div>
    </AppShell>
  );
}
