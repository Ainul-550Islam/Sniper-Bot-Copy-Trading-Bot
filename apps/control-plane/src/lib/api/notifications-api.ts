/**
 * Notification preferences API client (Part 04, L5).
 *
 * `GET /api/saas/notifications/preferences` is the only contract the server
 * exposes for this resource, so the client is read-only: the page renders
 * server truth and offers no write path.
 */

import { request } from "../api";

export interface NotificationPreference {
  channel: string;
  enabled: boolean;
  detail: string | null;
}

export interface NotificationsPreferences {
  organization_id: string;
  items: NotificationPreference[];
  updated_at: string;
}

export const notificationsApi = {
  preferences: () =>
    request<NotificationsPreferences>("/api/saas/notifications/preferences"),
};
