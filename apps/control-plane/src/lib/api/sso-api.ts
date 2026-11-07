/**
 * Tenant SSO configuration client (File 3 §7). Mirrors `saas/sso.rs`.
 * The client secret is write-only: the server returns only `client_secret_set`.
 */

import { ApiError, request, type LoginResponse } from "../api";

export type SsoProviderType = "oidc" | "google" | "okta" | "azure_ad";

export interface SsoConfig {
  id: string;
  organization_id: string;
  provider_type: SsoProviderType | string;
  issuer_url: string;
  client_id: string;
  client_secret_set: boolean;
  redirect_uri: string;
  allowed_domains: string[];
  role_mapping: Record<string, unknown>;
  enforce_sso: boolean;
  status: string;
}

export interface SsoConfigInput {
  provider_type: SsoProviderType;
  issuer_url: string;
  client_id: string;
  /** Omit to keep the stored secret; send a value to replace it. */
  client_secret?: string;
  redirect_uri: string;
  allowed_domains: string[];
  role_mapping: Record<string, unknown>;
  enforce_sso: boolean;
}

/** Returns null when the organization has no SSO configuration yet. */
export async function getSsoConfig(): Promise<SsoConfig | null> {
  try {
    return await request<SsoConfig>("/api/saas/sso/config");
  } catch (err: unknown) {
    if (err instanceof ApiError && err.status === 404) return null;
    throw err;
  }
}

export async function putSsoConfig(input: SsoConfigInput): Promise<SsoConfig> {
  return request<SsoConfig>("/api/saas/sso/config", { method: "PUT", body: input });
}

export async function deleteSsoConfig(): Promise<void> {
  await request<unknown>("/api/saas/sso/config", { method: "DELETE" });
}

/** State stored only for this browser tab to block unsolicited callback login swaps. */
export const PENDING_SSO_STATE_KEY = "sniper.sso.pending-state";

interface SsoAuthorizeResponse {
  authorize_url: string;
  state: string;
}

/** Begin a same-tab sign-in, retaining only the non-secret CSRF state. */
export async function beginSsoLogin(organizationId: string): Promise<void> {
  const result = await request<SsoAuthorizeResponse>(
    `/api/saas/sso/authorize?organization=${encodeURIComponent(organizationId)}`,
    { anonymous: true },
  );
  if (!result.authorize_url || !result.state) {
    throw new Error("The SSO authorization response was incomplete.");
  }
  window.sessionStorage.setItem(PENDING_SSO_STATE_KEY, result.state);
  window.location.assign(result.authorize_url);
}

/** Exchange an IdP authorization code after the callback page matches state. */
export function completeSsoLogin(state: string, code: string): Promise<LoginResponse> {
  return request<LoginResponse>("/api/saas/sso/callback", {
    method: "POST",
    body: { state, code },
    anonymous: true,
  });
}
