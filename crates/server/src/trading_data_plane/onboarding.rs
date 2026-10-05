//! Tenant guided onboarding flow and setup progression tracker.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use bot_core::membership::Permission;
use bot_core::tenant::OrganizationId;

use super::authorization_chain::{guard, guard_manage, TradingModuleFamily};
use crate::api::ApiState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnboardingStepPayload {
    pub step: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantOnboardingState {
    pub organization_id: String,
    pub current_step: u32,
    pub steps_total: u32,
    pub completed: bool,
    pub steps: OnboardingSteps,
    pub details: OnboardingDetails,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnboardingSteps {
    pub org_created: bool,
    pub custody_configured: bool,
    pub module_enabled: bool,
    pub strategy_configured: bool,
    pub paper_trade_executed: bool,
    pub live_prerequisites_met: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnboardingDetails {
    pub has_active_signer: bool,
    pub has_funded_wallet: bool,
    pub has_enabled_module: bool,
    pub has_saved_strategy: bool,
}

static ONBOARDING_STORE: LazyLock<Arc<Mutex<HashMap<OrganizationId, TenantOnboardingState>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(HashMap::new())));

/// `GET /api/tenant/onboarding`
pub async fn get_state(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::Sniper,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };

    let org = auth.organization_id();
    let mut lock = ONBOARDING_STORE.lock().unwrap();
    let current = lock.entry(org).or_insert_with(|| TenantOnboardingState {
        organization_id: org.to_string(),
        current_step: 2,
        steps_total: 6,
        completed: false,
        steps: OnboardingSteps {
            org_created: true,
            custody_configured: true,
            module_enabled: true,
            strategy_configured: false,
            paper_trade_executed: false,
            live_prerequisites_met: false,
        },
        details: OnboardingDetails {
            has_active_signer: true,
            has_funded_wallet: true,
            has_enabled_module: true,
            has_saved_strategy: false,
        },
    });

    (StatusCode::OK, Json(current.clone())).into_response()
}

/// `POST /api/tenant/onboarding/complete`
pub async fn complete_step(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<OnboardingStepPayload>,
) -> Response {
    let auth = match guard_manage(
        &state,
        &headers,
        Permission::BotConfigure,
        TradingModuleFamily::Sniper,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };

    let org = auth.organization_id();
    let mut lock = ONBOARDING_STORE.lock().unwrap();
    let current = lock.entry(org).or_insert_with(|| TenantOnboardingState {
        organization_id: org.to_string(),
        current_step: 1,
        steps_total: 6,
        completed: false,
        steps: OnboardingSteps {
            org_created: true,
            custody_configured: false,
            module_enabled: false,
            strategy_configured: false,
            paper_trade_executed: false,
            live_prerequisites_met: false,
        },
        details: OnboardingDetails {
            has_active_signer: false,
            has_funded_wallet: false,
            has_enabled_module: false,
            has_saved_strategy: false,
        },
    });

    match body.step {
        1 => current.steps.org_created = true,
        2 => {
            current.steps.custody_configured = true;
            current.details.has_active_signer = true;
            current.details.has_funded_wallet = true;
        }
        3 => {
            current.steps.module_enabled = true;
            current.details.has_enabled_module = true;
        }
        4 => {
            current.steps.strategy_configured = true;
            current.details.has_saved_strategy = true;
        }
        5 => current.steps.paper_trade_executed = true,
        6 => current.steps.live_prerequisites_met = true,
        _ => {}
    }

    current.current_step = (body.step + 1).min(6);
    if current.current_step == 6 && current.steps.live_prerequisites_met {
        current.completed = true;
    }

    (StatusCode::OK, Json(current.clone())).into_response()
}
