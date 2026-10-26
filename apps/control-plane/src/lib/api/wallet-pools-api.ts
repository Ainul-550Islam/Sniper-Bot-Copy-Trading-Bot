/**
 * SaaS wallet-pool API client.
 *
 * Types mirror the JSON returned by `crates/server/src/saas/wallet_pools.rs`.
 * A pool member is identified only by the custody signer's public metadata;
 * this client has no field for key material or provider credentials.
 */

import { request } from "../api";

export type WalletPoolAllocation = "round_robin" | "weighted_split";
export type WalletPoolStatus = "active" | "disabled";

export interface WalletPoolSummary {
  id: string;
  name: string;
  allocation: WalletPoolAllocation;
  status: WalletPoolStatus;
  rotation_cursor: number;
  active_members: number;
  created_at: string;
}

export interface WalletPoolsResponse {
  organization_id: string;
  pools: WalletPoolSummary[];
}

export interface WalletPoolMember {
  id: string;
  weight: number;
  status: string;
  logical_identity: string;
  public_address: string;
  added_at: string;
}

export interface WalletPoolDetail {
  id: string;
  organization_id: string;
  name: string;
  allocation: WalletPoolAllocation;
  status: WalletPoolStatus;
  rotation_cursor: number;
  created_at: string;
  members: WalletPoolMember[];
}

export interface CreateWalletPoolInput {
  name: string;
  allocation?: WalletPoolAllocation;
}

export interface CreateWalletPoolResponse {
  id: string;
  organization_id: string;
  name: string;
  allocation: WalletPoolAllocation;
  status: WalletPoolStatus;
  rotation_cursor: number;
  created_at: string;
}

export interface UpdateWalletPoolInput {
  name?: string;
  allocation?: WalletPoolAllocation;
  status?: WalletPoolStatus;
}

export interface UpdateWalletPoolResponse {
  id: string;
  name: string;
  allocation: WalletPoolAllocation;
  status: WalletPoolStatus;
  rotation_cursor: number;
}

export interface AddWalletPoolMemberInput {
  custody_signer_id: string;
  weight?: number;
}

export interface AddWalletPoolMemberResponse {
  id: string;
  pool_id: string;
  custody_signer_id: string;
  weight: number;
  status: string;
  added_at: string;
}

export interface RemoveWalletPoolMemberResponse {
  id: string;
  status: "removed";
}

export const walletPoolsApi = {
  list: () => request<WalletPoolsResponse>("/api/saas/wallet-pools"),
  create: (input: CreateWalletPoolInput) =>
    request<CreateWalletPoolResponse>("/api/saas/wallet-pools", {
      method: "POST",
      body: input,
    }),
  get: (id: string) =>
    request<WalletPoolDetail>(`/api/saas/wallet-pools/${encodeURIComponent(id)}`),
  update: (id: string, input: UpdateWalletPoolInput) =>
    request<UpdateWalletPoolResponse>(
      `/api/saas/wallet-pools/${encodeURIComponent(id)}`,
      { method: "PATCH", body: input },
    ),
  addMember: (id: string, input: AddWalletPoolMemberInput) =>
    request<AddWalletPoolMemberResponse>(
      `/api/saas/wallet-pools/${encodeURIComponent(id)}/members`,
      { method: "POST", body: input },
    ),
  removeMember: (id: string, memberId: string) =>
    request<RemoveWalletPoolMemberResponse>(
      `/api/saas/wallet-pools/${encodeURIComponent(id)}/members/${encodeURIComponent(memberId)}`,
      { method: "DELETE" },
    ),
};
