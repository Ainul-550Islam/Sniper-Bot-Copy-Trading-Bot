/**
 * Webhook Outbound Events API Client (SECOND.md §81).
 *
 * Fully typed client for webhook subscription, signature verification,
 * and ping payload delivery testing.
 */

import { request } from "../api";

export interface WebhookEndpoint {
  id: string;
  organization_id: string;
  url: string;
  description: string;
  events: string[];
  secret?: string;
  is_active: boolean;
  created_at: string;
}

export interface WebhooksResponse {
  organization_id: string;
  items: WebhookEndpoint[];
  count: number;
}

export interface CreateWebhookInput {
  url: string;
  description?: string;
  events: string[];
}

/**
 * List all configured webhook endpoints for the tenant.
 */
export async function listWebhooks(): Promise<WebhookEndpoint[]> {
  const res = await request<WebhooksResponse>("/api/saas/webhooks");
  return res.items;
}

/**
 * Register a new outbound webhook endpoint.
 */
export async function createWebhook(input: CreateWebhookInput): Promise<WebhookEndpoint> {
  return request<WebhookEndpoint>("/api/saas/webhooks", {
    method: "POST",
    body: input,
  });
}

/**
 * Remove a webhook endpoint.
 */
export async function deleteWebhook(id: string): Promise<{ success: boolean }> {
  return request<{ success: boolean }>(`/api/saas/webhooks/${encodeURIComponent(id)}`, {
    method: "DELETE",
  });
}

/**
 * Replace the endpoint signing secret. The new secret is returned once.
 */
export async function rotateWebhookSecret(id: string): Promise<{ id: string; secret: string; warning: string }> {
  return request<{ id: string; secret: string; warning: string }>(
    `/api/saas/webhooks/${encodeURIComponent(id)}/rotate`,
    {
      method: "POST",
    },
  );
}

/**
 * Dispatch a signed test event to the webhook URL and return the remote result.
 */
export async function testWebhook(
  id: string,
): Promise<{ success: boolean; delivery_id: string; status_code: number; latency_ms: number }> {
  return request<{ success: boolean; delivery_id: string; status_code: number; latency_ms: number }>(
    `/api/saas/webhooks/${encodeURIComponent(id)}/test`,
    {
      method: "POST",
    },
  );
}
