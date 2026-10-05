/**
 * Security & Hardening API Client (SECOND.md §80).
 *
 * Fully typed client for credential rotation, multi-factor enforcement,
 * and tenant IP allowlisting.
 */

import { request } from "../api";

export interface SecurityPosture {
  organization_id: string;
  mfa_enforced: boolean;
  ip_allowlist: string[];
  active_sessions_count: number;
}

/**
 * Rotate all tenant session tokens and API keys.
 */
export async function rotateTokens(): Promise<{ success: boolean; organization_id: string; message: string }> {
  return request<{ success: boolean; organization_id: string; message: string }>(
    "/api/saas/security/rotate-tokens",
    {
      method: "POST",
    },
  );
}

/**
 * Enforce multi-factor authentication org-wide.
 */
export async function enforceMfa(
  enabled: boolean,
): Promise<{ success: boolean; organization_id: string; mfa_enforced: boolean }> {
  return request<{ success: boolean; organization_id: string; mfa_enforced: boolean }>(
    "/api/saas/security/mfa-enforce",
    {
      method: "POST",
      body: { enabled },
    },
  );
}

/**
 * Update tenant-specific IP allowlist CIDR blocks.
 */
export async function updateIpAllowlist(
  cidrs: string[],
): Promise<{ success: boolean; organization_id: string; cidrs: string[] }> {
  return request<{ success: boolean; organization_id: string; cidrs: string[] }>(
    "/api/saas/security/ip-allowlist",
    {
      method: "POST",
      body: { cidrs },
    },
  );
}
