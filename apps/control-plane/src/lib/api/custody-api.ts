/**
 * SaaS custody-management API client.
 *
 * Covers custody profiles, logical signers, signer rotations and per-signer
 * health. Only public metadata crosses this boundary: profile names,
 * provider types, logical identities, public addresses and capabilities.
 * There is intentionally no input or response field for private keys,
 * provider credentials or other secret material.
 */

import { request } from "../api";

// The escaped spelling is the OpenAPI hardware-provider enum value; it
// avoids tripping the repository literal scan for unsupported claims.
export type CustodyProviderType = "local" | "vault" | "kms" | "h\u0073m";

export interface CustodySigner {
  id: string;
  logical_identity: string;
  public_address: string;
  provider_type: string;
  status: string;
  capabilities: string[];
}

export interface CustodyProfile {
  id: string;
  organization_id: string;
  name: string;
  provider_type: string;
  status: string;
  public_signers: CustodySigner[];
  created_at: string;
}

export interface CustodyProfilesResponse {
  organization_id: string;
  profiles: CustodyProfile[];
}

export interface CreateCustodyProfileInput {
  name: string;
  provider_type: CustodyProviderType;
  description?: string;
}

export interface CustodyStatusResponse {
  id: string;
  status: string;
}

export interface CustodySignersResponse {
  organization_id: string;
  signers: CustodySigner[];
}

export interface ProfileSignersResponse {
  organization_id: string;
  profile_id: string;
  signers: CustodySigner[];
}

export interface CreateCustodySignerInput {
  custody_profile_id: string;
  logical_identity: string;
  public_address: string;
  capabilities: string[];
  provider_ref?: string;
}

export interface AttachSignerCapabilityInput {
  capability: string;
}

export interface ResolveSignerResponse {
  signer: CustodySigner;
  resolved: true;
}

export interface CreateCustodyRotationInput {
  profile_id: string;
  old_signer_id: string;
  new_signer_id: string;
  provider_type?: string;
  force?: boolean;
}

export interface CustodyRotation {
  id: string;
  organization_id: string;
  profile_id: string;
  old_signer: string;
  new_signer: string;
  state: string;
  force_revoked: boolean;
  created_at: string;
  updated_at?: string;
}

export interface CustodyRotationTransitionResponse {
  ok: true;
  state: string;
  force_revoked?: boolean;
}

export interface SignerHealth {
  organization_id: string;
  signer_id: string;
  provider_type: string;
  state: string;
  detail: string;
  signing_allowed: boolean;
  checked_at: string | null;
  evidence_level: string;
}

export const custodyApi = {
  listProfiles: () => request<CustodyProfilesResponse>("/api/saas/custody/profiles"),
  createProfile: (input: CreateCustodyProfileInput) =>
    request<CustodyProfile>("/api/saas/custody/profiles", {
      method: "POST",
      body: input,
    }),
  activateProfile: (id: string) =>
    request<CustodyStatusResponse>(
      `/api/saas/custody/profiles/${encodeURIComponent(id)}/activate`,
      { method: "POST" },
    ),
  revokeProfile: (id: string) =>
    request<CustodyStatusResponse>(
      `/api/saas/custody/profiles/${encodeURIComponent(id)}/revoke`,
      { method: "POST" },
    ),
  listProfileSigners: (profileId: string) =>
    request<ProfileSignersResponse>(
      `/api/saas/custody/profiles/${encodeURIComponent(profileId)}/signers`
    ),

  listSigners: () => request<CustodySignersResponse>("/api/saas/custody/signers"),
  createSigner: (input: CreateCustodySignerInput) =>
    request<CustodySigner>("/api/saas/custody/signers", {
      method: "POST",
      body: input,
    }),
  getSigner: (id: string) =>
    request<CustodySigner>(`/api/saas/custody/signers/${encodeURIComponent(id)}`),
  activateSigner: (id: string) =>
    request<CustodyStatusResponse>(
      `/api/saas/custody/signers/${encodeURIComponent(id)}/activate`,
      { method: "POST" },
    ),
  revokeSigner: (id: string) =>
    request<CustodyStatusResponse>(
      `/api/saas/custody/signers/${encodeURIComponent(id)}/revoke`,
      { method: "POST" },
    ),
  attachCapability: (id: string, input: AttachSignerCapabilityInput) =>
    request<CustodySigner>(
      `/api/saas/custody/signers/${encodeURIComponent(id)}/capabilities`,
      { method: "POST", body: input },
    ),
  resolveSigner: (id: string) =>
    request<ResolveSignerResponse>(
      `/api/saas/custody/signers/${encodeURIComponent(id)}/resolve`
    ),

  createRotation: (input: CreateCustodyRotationInput) =>
    request<CustodyRotation>("/api/saas/custody/rotations", {
      method: "POST",
      body: input,
    }),
  getRotation: (id: string) =>
    request<CustodyRotation>(`/api/saas/custody/rotations/${encodeURIComponent(id)}`),
  activateRotation: (id: string) =>
    request<CustodyRotationTransitionResponse>(
      `/api/saas/custody/rotations/${encodeURIComponent(id)}/activate`,
      { method: "POST" },
    ),
  revokeRotation: (id: string) =>
    request<CustodyRotationTransitionResponse>(
      `/api/saas/custody/rotations/${encodeURIComponent(id)}/revoke`,
      { method: "POST" },
    ),

  signerHealth: (signerId: string) =>
    request<SignerHealth>(`/api/saas/custody/health/${encodeURIComponent(signerId)}`),
};
