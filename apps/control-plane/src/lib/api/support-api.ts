/**
 * Typed Support Ticket Client (THIRD.md §129).
 *
 * Interfaces with `/api/saas/support/tickets`.
 */

import { request } from "../api";

export type TicketPriority = "low" | "normal" | "high" | "urgent";
export type TicketStatus = "open" | "in_progress" | "resolved" | "closed";

export interface SupportTicket {
  id: string;
  organization_id: string;
  subject: string;
  priority: TicketPriority;
  status: TicketStatus;
  description: string;
  created_at: string;
  updated_at: string;
}

export interface SupportTicketsResponse {
  organization_id: string;
  items: SupportTicket[];
  count: number;
}

export interface CreateTicketInput {
  subject: string;
  priority: TicketPriority;
  description: string;
}

/** Lists active and historical tenant support tickets. */
export async function listSupportTickets(): Promise<SupportTicketsResponse> {
  return request<SupportTicketsResponse>("/api/saas/support/tickets");
}

/** Submits a new support or incident ticket. */
export async function createSupportTicket(input: CreateTicketInput): Promise<SupportTicket> {
  return request<SupportTicket>("/api/saas/support/tickets", {
    method: "POST",
    body: input,
  });
}
