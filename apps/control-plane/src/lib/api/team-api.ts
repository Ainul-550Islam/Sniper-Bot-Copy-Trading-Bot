/**
 * Team Administration API Client (SECOND.md §79).
 *
 * Fully typed client for tenant user management, role delegation,
 * and security policy enforcement.
 */

import { request } from "../api";

export type TeamRole = "owner" | "admin" | "operator" | "analyst" | "viewer";

export interface TeamMember {
  id: string;
  user_id: string;
  email: string;
  role: TeamRole;
  created_at: string;
  last_login_at?: string;
}

export interface InviteMemberInput {
  email: string;
  role: TeamRole;
}

/**
 * Dispatch an invitation to join the tenant organization.
 */
export async function inviteMember(
  input: InviteMemberInput,
): Promise<{ success: boolean; invitation_id: string; organization_id: string; email: string; role: string }> {
  return request<{ success: boolean; invitation_id: string; organization_id: string; email: string; role: string }>(
    "/api/saas/team/invites",
    {
      method: "POST",
      body: input,
    },
  );
}

/**
 * Revoke team membership for a user.
 */
export async function removeMember(memberId: string): Promise<{ success: boolean }> {
  return request<{ success: boolean }>(`/api/saas/team/members/${encodeURIComponent(memberId)}`, {
    method: "DELETE",
  });
}

/**
 * Update member RBAC role.
 */
export async function updateMemberRole(
  memberId: string,
  role: TeamRole,
): Promise<{ success: boolean; role: string }> {
  return request<{ success: boolean; role: string }>(`/api/saas/team/members/${encodeURIComponent(memberId)}`, {
    method: "PATCH",
    body: { role },
  });
}
