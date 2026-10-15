/**
 * Tenant SSO configuration client (File 3 §7). Mirrors `saas/sso.rs`.
 * The client secret is write-only: the server returns only `client_secret_set`.
 */

import { ApiError, request } from "../api";

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

/** Path the browser opens to run a real sign-in against the IdP. */
export function ssoAuthorizeUrl(organizationId: string): string {
  return `/api/saas/sso/authorize?organization=${encodeURIComponent(organizationId)}`;
}
