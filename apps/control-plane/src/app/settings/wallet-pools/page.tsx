"use client";

import { useCallback, useEffect, useState } from "react";
import { AppShell } from "@/components/AppShell";
import { MemberManager } from "@/components/wallet-pools/MemberManager";
import { PoolDetailPanel } from "@/components/wallet-pools/PoolDetail";
import { PoolList } from "@/components/wallet-pools/PoolList";
import { toDisplayError } from "@/lib/api";
import { sessionStore } from "@/lib/auth";
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

  useEffect(() => sessionStore.subscribe(() => setSessionTick((value) => value + 1)), []);
  const canManage = getCapabilities({ role: currentRole() }).canManageTeam;

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
      const [poolResponse, signerResponse] = await Promise.all([
        walletPoolsApi.list(),
        custodyApi.listSigners(),
      ]);
      setPools(poolResponse.pools);
      setSigners(signerResponse.signers);
      const nextSelectedId = selectedId ?? poolResponse.pools[0]?.id ?? null;
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
      </div>
    </AppShell>
  );
}
