//! Buyer-facing typed SDK for team, security and webhook APIs (SECOND.md §99).

use serde::{Deserialize, Serialize};

use crate::client::SaasClient;
use crate::error::BotResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberDto {
    pub membership_id: String,
    pub user_id: String,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEndpointDto {
    pub id: String,
    pub url: String,
    pub event_types: Vec<String>,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWebhookRequest {
    pub url: String,
    pub event_types: Vec<String>,
}

pub struct TeamSecurityClient<'a> {
    pub(crate) client: &'a SaasClient,
}

impl<'a> TeamSecurityClient<'a> {
    pub fn new(client: &'a SaasClient) -> Self {
        Self { client }
    }

    /// List team members for an organization.
    pub async fn list_members(&self, org_id: &str) -> BotResult<Vec<MemberDto>> {
        let res: serde_json::Value = self
            .client
            .get(&format!("/api/saas/organizations/{}/members", org_id))
            .await?;
        let members = res
            .get("members")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        serde_json::from_value(members).map_err(|e| crate::error::BotError::invalid_input(e.to_string()))
    }

    /// List webhook endpoints.
    pub async fn list_webhooks(&self) -> BotResult<Vec<WebhookEndpointDto>> {
        let res: serde_json::Value = self.client.get("/api/saas/webhooks").await?;
        let items = res
            .get("items")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        serde_json::from_value(items).map_err(|e| crate::error::BotError::invalid_input(e.to_string()))
    }

    /// Create webhook endpoint.
    pub async fn create_webhook(&self, req: &CreateWebhookRequest) -> BotResult<WebhookEndpointDto> {
        self.client.post("/api/saas/webhooks", req).await
    }
}
