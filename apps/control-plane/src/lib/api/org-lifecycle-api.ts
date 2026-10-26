/**
 * Organization lifecycle API client (Part 04, L2).
 *
 * Covers `GET /api/saas/organizations/{id}/lifecycle`,
 * `POST /api/saas/organizations/{id}/suspend`,
 * `POST /api/saas/organizations/{id}/resume` and
 * `POST /api/saas/organizations/{id}/close` — the organization lifecycle
 * endpoints that had OpenAPI contracts but no frontend caller.
 *
 * Suspend/resume/close are destructive or state-changing: the page confirms
 * (close requires a typed organization-slug confirmation) before the request
 * is sent, and the displayed lifecycle state always comes from a server
 * refetch — never from an optimistic local rewrite.
 */

import { request } from "../api";

export interface OrganizationLifecycle {
  organization_id: string;
  lifecycle_status: string;
  state: string;
  reason: string | null;
  suspended_at: string | null;
  suspended_by: string | null;
  closed_at: string | null;
  updated_at: string;
}

export interface OrgLifecycleActionResponse {
  organization_id: string;
  lifecycle_status: string;
  state: string;
  updated_at: string;
}

export interface CloseOrganizationResponse {
  organization_id: string;
  status: "close_scheduled";
  reason: string;
  updated_at: string;
}

export const orgLifecycleApi = {
  /** Current lifecycle state of the organization (server truth). */
  get: (organizationId: string) =>
    request<OrganizationLifecycle>(
      `/api/saas/organizations/${encodeURIComponent(organizationId)}/lifecycle`,
    ),

  /** Suspend trading and API access for the organization. */
  suspend: (organizationId: string) =>
    request<OrgLifecycleActionResponse>(
      `/api/saas/organizations/${encodeURIComponent(organizationId)}/suspend`,
      { method: "POST" },
    ),

  /** Resume trading and API access for the organization. */
  resume: (organizationId: string) =>
    request<OrgLifecycleActionResponse>(
      `/api/saas/organizations/${encodeURIComponent(organizationId)}/resume`,
      { method: "POST" },
    ),

  /** Schedule irreversible closure. `reason` is stored server-side. */
  close: (organizationId: string, reason: string) =>
    request<CloseOrganizationResponse>(
      `/api/saas/organizations/${encodeURIComponent(organizationId)}/close`,
      { method: "POST", body: { reason } },
    ),
};
