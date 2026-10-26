/**
 * Team invitation lifecycle API client (Part 04, L1).
 *
 * Covers `DELETE /api/saas/team/invites/{id}` (revoke) and
 * `POST /api/saas/team/invites/{id}/resend` (resend) — the two team-invite
 * endpoints that had OpenAPI contracts but no frontend caller. Revocation is
 * destructive: the page must confirm before the DELETE is sent.
 *
 * Types mirror the wire contract in openapi/openapi.json. Nothing here is
 * synthesized: callers render exactly what the server returned.
 */

import { request } from "../api";

export interface TeamInvite {
  id: string;
  email: string;
  role: string;
  status: string;
  created_at: string;
  expires_at: string | null;
}

export interface TeamInvitesResponse {
  organization_id: string;
  items: TeamInvite[];
  count: number;
}

export interface ResendInviteResponse {
  id: string;
  status: string;
  resent_at: string;
}

export interface RevokeInviteResponse {
  id: string;
  status: "revoked";
}

export const teamInvitesApi = {
  /** List pending invitations for the authenticated tenant. */
  list: () => request<TeamInvitesResponse>("/api/saas/team/invites"),

  /** Re-send a pending invitation (idempotent server-side). */
  resend: (id: string) =>
    request<ResendInviteResponse>(
      `/api/saas/team/invites/${encodeURIComponent(id)}/resend`,
      { method: "POST" },
    ),

  /** Revoke a pending invitation. Destructive — callers must confirm first. */
  revoke: (id: string) =>
    request<RevokeInviteResponse>(
      `/api/saas/team/invites/${encodeURIComponent(id)}`,
      { method: "DELETE" },
    ),
};
