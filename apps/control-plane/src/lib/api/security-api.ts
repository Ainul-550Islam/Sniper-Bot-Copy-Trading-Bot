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
  has_totp_configured: boolean;
  ip_allowlist: string[];
  ip_allowlist_count: number;
  session_duration_hours: number;
  require_signed_commits: boolean;
}

export async function getSecurityStatus(): Promise<SecurityPosture> {
  return request<SecurityPosture>("/api/saas/security/status");
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
export async function setupTotp(): Promise<{
  device_id: string;
  secret: string;
  otpauth_url: string;
  verified: boolean;
  backup_codes: string[] | null;
}> {
  return request<{
    device_id: string;
    secret: string;
    otpauth_url: string;
    verified: boolean;
    backup_codes: string[] | null;
  }>("/api/saas/security/totp/setup", { method: "POST" });
}

export async function verifyTotp(
  deviceId: string,
  code: string,
): Promise<{ success: boolean; device_id: string; verified: boolean; session_promoted: boolean }> {
  return request<{ success: boolean; device_id: string; verified: boolean; session_promoted: boolean }>("/api/saas/security/totp/verify", {
    method: "POST",
    body: { device_id: deviceId, code },
  });
}

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
