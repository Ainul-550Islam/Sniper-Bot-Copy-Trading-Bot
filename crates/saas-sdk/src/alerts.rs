//! Typed Alerts & Incident SDK Client (THIRD.md §148).

use serde::{Deserialize, Serialize};

use crate::client::Client;
use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertItem {
    pub id: String,
    pub organization_id: String,
    pub severity: String,
    pub category: String,
    pub title: String,
    pub message: String,
    pub is_acknowledged: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertsResponse {
    pub organization_id: String,
    pub items: Vec<AlertItem>,
    pub unacknowledged_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcknowledgeAlertResponse {
    pub success: bool,
    pub id: String,
    pub acknowledged_at: String,
}

/// Alerts and notifications client.
#[derive(Debug, Clone)]
pub struct AlertsClient {
    client: Client,
}

impl AlertsClient {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Lists alerts with optional unread filter.
    pub async fn list_alerts(&self, unread_only: bool) -> Result<AlertsResponse> {
        let path = if unread_only {
            "/api/saas/alerts?unread=true"
        } else {
            "/api/saas/alerts"
        };
        self.client.get(path).await
    }

    /// Acknowledges an alert.
    pub async fn acknowledge(&self, id: &str) -> Result<AcknowledgeAlertResponse> {
        self.client
            .post(
                &format!("/api/saas/alerts/{}/ack", id),
                &serde_json::json!({}),
            )
            .await
    }
}
