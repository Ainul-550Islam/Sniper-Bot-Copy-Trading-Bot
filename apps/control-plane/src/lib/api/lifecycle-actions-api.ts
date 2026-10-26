/**
 * SaaS data-lifecycle action client.
 *
 * The existing status read remains owned by `commercial.lifecycleStatus` in
 * `lib/commercial.ts`. This module owns only the four action endpoints that
 * previously had no frontend caller. An action's response is not treated as
 * the new lifecycle state by the page; the page refetches `/status` after a
 * successful POST.
 */

import { request } from "../api";

export interface LifecycleActionInput {
  reason?: string;
  confirm?: boolean;
}

export interface DisconnectLifecycleResponse {
  ok: true;
}

export interface RevokeCredentialsResponse {
  ok: true;
  credentials_revoked: true;
}

export interface RevokeSessionsResponse {
  ok: true;
  sessions_invalidated: true;
}

export interface ScheduleRetentionResponse {
  ok: true;
  retention_scheduled: true;
  purge_eligible_at: string;
}

function actionPath(organizationId: string, action: string): string {
  return `/api/saas/data-lifecycle/${encodeURIComponent(organizationId)}/${action}`;
}

export const lifecycleActionsApi = {
  disconnect: (organizationId: string) =>
    request<DisconnectLifecycleResponse>(actionPath(organizationId, "disconnect"), {
      method: "POST",
    }),
  revokeCredentials: (organizationId: string) =>
    request<RevokeCredentialsResponse>(
      actionPath(organizationId, "revoke-credentials"),
      { method: "POST" },
    ),
  revokeSessions: (organizationId: string) =>
    request<RevokeSessionsResponse>(actionPath(organizationId, "revoke-sessions"), {
      method: "POST",
    }),
  scheduleRetention: (organizationId: string, input: LifecycleActionInput = {}) =>
    request<ScheduleRetentionResponse>(
      actionPath(organizationId, "schedule-retention"),
      { method: "POST", body: input },
    ),
};
