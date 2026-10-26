/**
 * Organization events feed API client (Part 04, L5).
 *
 * `GET /api/saas/events` returns the tenant's audit/event stream. The page
 * renders it as an honest feed: an empty array is "no events", never a
 * synthesized row.
 */

import { request } from "../api";

export interface OrganizationEvent {
  id: string;
  actor: string;
  action: string;
  target: string | null;
  summary: string;
  timestamp: string;
}

export interface EventsResponse {
  organization_id: string;
  events: OrganizationEvent[];
  count: number;
}

export const eventsApi = {
  list: () => request<EventsResponse>("/api/saas/events"),
};
