//! Typed Support & Incident Tickets SDK Client (THIRD.md §149).

use serde::{Deserialize, Serialize};

use crate::client::Client;
use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupportTicket {
    pub id: String,
    pub organization_id: String,
    pub subject: String,
    pub priority: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupportTicketsResponse {
    pub organization_id: String,
    pub items: Vec<SupportTicket>,
    pub count: usize,
}

/// Dedicated support ticket client.
#[derive(Debug, Clone)]
pub struct SupportClient {
    client: Client,
}

impl SupportClient {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Lists organization support tickets.
    pub async fn list_tickets(&self) -> Result<SupportTicketsResponse> {
        self.client.get("/api/saas/support/tickets").await
    }

    /// Creates a support ticket.
    pub async fn create_ticket(
        &self,
        subject: &str,
        priority: &str,
        description: &str,
    ) -> Result<SupportTicket> {
        self.client
            .post(
                "/api/saas/support/tickets",
                &serde_json::json!({
                    "subject": subject,
                    "priority": priority,
                    "description": description
                }),
            )
            .await
    }
}
