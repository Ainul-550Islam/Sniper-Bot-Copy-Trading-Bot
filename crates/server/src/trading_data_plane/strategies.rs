//! Durable tenant-scoped strategy CRUD and versioning service.
//!
//! Strategy records are stored in migration 0037's `strategies` table. Every
//! read and write carries the authenticated organization id, and updates lock
//! the row before applying a versioned change.

use std::str::FromStr;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use bot_core::membership::Permission;
use bot_core::models::{BotModule, ExecutionMode};
use bot_core::strategy::{validate_strategy_params, StrategyId, StrategyRecord, StrategyStatus};
use bot_core::tenant::OrganizationId;

use super::authorization_chain::{guard, guard_manage, TradingModuleFamily};
use crate::api::ApiState;

const MAX_NAME_LENGTH: usize = 128;
const MAX_DESCRIPTION_LENGTH: usize = 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyQuery {
    pub module: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateStrategyBody {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub module: String,
    #[serde(default)]
    pub mode: Option<String>,
    pub config: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateStrategyBody {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub config: Option<serde_json::Value>,
}

fn error_response(status: StatusCode, error: &'static str, reason: impl Into<String>) -> Response {
    (
        status,
        Json(json!({
            "error": error,
            "reason": reason.into(),
        })),
    )
        .into_response()
}

fn database(state: &ApiState) -> Result<&bot_core::db::Database, Response> {
    state.db.as_deref().ok_or_else(|| {
        error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "trading_data_plane_unavailable",
            "strategy storage requires an attached PostgreSQL database",
        )
    })
}

fn parse_module(value: &str) -> Result<BotModule, Response> {
    match BotModule::from_str(value.trim()) {
        Ok(BotModule::Sniper) => Ok(BotModule::Sniper),
        Ok(BotModule::Copy) => Ok(BotModule::Copy),
        Ok(BotModule::Polymarket) => Ok(BotModule::Polymarket),
        Ok(_) => Err(error_response(
            StatusCode::BAD_REQUEST,
            "unsupported_module",
            "only sniper, copy, and polymarket strategies are supported",
        )),
        Err(_) => Err(error_response(
            StatusCode::BAD_REQUEST,
            "unsupported_module",
            "module must be sniper, copy, or polymarket",
        )),
    }
}

fn parse_mode(value: Option<&str>) -> Result<ExecutionMode, Response> {
    let selected = value.unwrap_or("paper");
    ExecutionMode::from_str(selected).map_err(|_| {
        error_response(
            StatusCode::BAD_REQUEST,
            "invalid_execution_mode",
            "mode must be paper, simulate, or live",
        )
    })
}

fn validate_text(name: &str, description: &str) -> Result<(), Response> {
    if name.trim().is_empty() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "name_required",
            "strategy name is required",
        ));
    }
    if name.trim().len() > MAX_NAME_LENGTH {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "name_too_long",
            format!("strategy name must be no longer than {MAX_NAME_LENGTH} characters"),
        ));
    }
    if description.trim().len() > MAX_DESCRIPTION_LENGTH {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "description_too_long",
            format!(
                "strategy description must be no longer than {MAX_DESCRIPTION_LENGTH} characters"
            ),
        ));
    }
    Ok(())
}

fn record_from_row(row: &sqlx::postgres::PgRow) -> Result<StrategyRecord, Response> {
    let module = BotModule::from_str(row.get::<String, _>("module").as_str()).map_err(|_| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "invalid_strategy_record",
            "stored strategy module is invalid",
        )
    })?;
    let mode = ExecutionMode::from_str(row.get::<String, _>("mode").as_str()).map_err(|_| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "invalid_strategy_record",
            "stored strategy mode is invalid",
        )
    })?;
    let status =
        StrategyStatus::parse(row.get::<String, _>("status").as_str()).ok_or_else(|| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "invalid_strategy_record",
                "stored strategy status is invalid",
            )
        })?;
    Ok(StrategyRecord {
        id: StrategyId::from_uuid(row.get::<Uuid, _>("id")),
        organization_id: OrganizationId::from(row.get::<Uuid, _>("organization_id")),
        name: row.get("name"),
        description: row.get("description"),
        module,
        mode,
        status,
        version: row.get::<i32, _>("version").max(1) as u32,
        config_json: row.get("config_json"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

async fn fetch_one(
    db: &bot_core::db::Database,
    organization_id: OrganizationId,
    strategy_id: StrategyId,
) -> Result<Option<StrategyRecord>, Response> {
    let row = sqlx::query(
        "SELECT id, organization_id, name, description, module, mode, status,
                version, config_json, created_at, updated_at
           FROM strategies
          WHERE id = $1 AND organization_id = $2",
    )
    .bind(strategy_id.as_uuid())
    .bind(organization_id.as_uuid())
    .fetch_optional(db.pool())
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "failed to load tenant strategy");
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "strategy_storage_error",
            "strategy could not be loaded",
        )
    })?;

    row.map(|value| record_from_row(&value)).transpose()
}

/// `GET /api/tenant/strategies?module=`.
pub async fn list(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<StrategyQuery>,
) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };
    let db = match database(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let org = auth.organization_id();

    let rows = if let Some(module) = query.module.as_deref() {
        let parsed = match parse_module(module) {
            Ok(value) => value,
            Err(response) => return response,
        };
        match sqlx::query(
            "SELECT id, organization_id, name, description, module, mode, status,
                    version, config_json, created_at, updated_at
               FROM strategies
              WHERE organization_id = $1 AND module = $2
              ORDER BY created_at DESC, id DESC",
        )
        .bind(org.as_uuid())
        .bind(parsed.as_str())
        .fetch_all(db.pool())
        .await
        {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(error = %error, "failed to list filtered tenant strategies");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "strategy_storage_error",
                    "strategies could not be loaded",
                );
            }
        }
    } else {
        match sqlx::query(
            "SELECT id, organization_id, name, description, module, mode, status,
                    version, config_json, created_at, updated_at
               FROM strategies
              WHERE organization_id = $1
              ORDER BY created_at DESC, id DESC",
        )
        .bind(org.as_uuid())
        .fetch_all(db.pool())
        .await
        {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(error = %error, "failed to list tenant strategies");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "strategy_storage_error",
                    "strategies could not be loaded",
                );
            }
        }
    };

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        match record_from_row(&row) {
            Ok(record) => items.push(record),
            Err(response) => return response,
        }
    }

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": org.to_string(),
            "items": items,
            "count": items.len(),
        })),
    )
        .into_response()
}

/// `GET /api/tenant/strategies/:id`.
pub async fn get_one(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };
    let db = match database(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let strategy_id = match StrategyId::parse(&id) {
        Some(value) => value,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_strategy_id",
                "strategy id must be a UUID",
            )
        }
    };

    match fetch_one(db, auth.organization_id(), strategy_id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(record)).into_response(),
        Ok(None) => error_response(
            StatusCode::NOT_FOUND,
            "strategy_not_found",
            "strategy was not found",
        ),
        Err(response) => response,
    }
}

/// `POST /api/tenant/strategies`.
pub async fn create(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CreateStrategyBody>,
) -> Response {
    let auth = match guard_manage(
        &state,
        &headers,
        Permission::BotStart,
        TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };
    let db = match database(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = validate_text(&body.name, &body.description) {
        return response;
    }
    let module = match parse_module(&body.module) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let mode = match parse_mode(body.mode.as_deref()) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(error) = validate_strategy_params(module, &body.config) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_parameters",
            error.to_string(),
        );
    }

    let record = StrategyRecord::new(
        auth.organization_id(),
        body.name.trim().to_string(),
        body.description.trim().to_string(),
        module,
        mode,
        body.config,
        Utc::now(),
    );
    let insert = sqlx::query(
        "INSERT INTO strategies
             (id, organization_id, name, description, module, mode, status,
              version, config_json, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)",
    )
    .bind(record.id.as_uuid())
    .bind(record.organization_id.as_uuid())
    .bind(&record.name)
    .bind(&record.description)
    .bind(record.module.as_str())
    .bind(record.mode.as_str().to_ascii_lowercase())
    .bind(record.status.as_str())
    .bind(record.version as i32)
    .bind(&record.config_json)
    .bind(record.created_at)
    .execute(db.pool())
    .await;

    if let Err(error) = insert {
        tracing::error!(error = %error, "failed to create tenant strategy");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "strategy_storage_error",
            "strategy could not be created",
        );
    }

    (StatusCode::CREATED, Json(record)).into_response()
}

/// `PUT /api/tenant/strategies/:id`.
pub async fn update(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<UpdateStrategyBody>,
) -> Response {
    let auth = match guard_manage(
        &state,
        &headers,
        Permission::BotStart,
        TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };
    let db = match database(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let strategy_id = match StrategyId::parse(&id) {
        Some(value) => value,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_strategy_id",
                "strategy id must be a UUID",
            )
        }
    };
    let org = auth.organization_id();
    let current = match fetch_one(db, org, strategy_id).await {
        Ok(Some(value)) => value,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "strategy_not_found",
                "strategy was not found",
            )
        }
        Err(response) => return response,
    };

    let name = body.name.unwrap_or_else(|| current.name.clone());
    let description = body
        .description
        .unwrap_or_else(|| current.description.clone());
    if let Err(response) = validate_text(&name, &description) {
        return response;
    }
    let config = body.config.unwrap_or_else(|| current.config_json.clone());
    if let Err(error) = validate_strategy_params(current.module, &config) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_parameters",
            error.to_string(),
        );
    }
    let status = match body.status {
        Some(value) => match StrategyStatus::parse(&value) {
            Some(parsed) => parsed,
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_strategy_status",
                    "status must be active, paused, or archived",
                )
            }
        },
        None => current.status,
    };
    let now = Utc::now();
    let result = sqlx::query(
        "UPDATE strategies
            SET name = $3, description = $4, status = $5, config_json = $6,
                version = version + 1, updated_at = $7
          WHERE id = $1 AND organization_id = $2 AND version = $8",
    )
    .bind(strategy_id.as_uuid())
    .bind(org.as_uuid())
    .bind(name.trim())
    .bind(description.trim())
    .bind(status.as_str())
    .bind(&config)
    .bind(now)
    .bind(current.version as i32)
    .execute(db.pool())
    .await;

    match result {
        Ok(done) if done.rows_affected() == 1 => match fetch_one(db, org, strategy_id).await {
            Ok(Some(record)) => (StatusCode::OK, Json(record)).into_response(),
            Ok(None) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "strategy_storage_error",
                "updated strategy could not be reloaded",
            ),
            Err(response) => response,
        },
        Ok(_) => error_response(
            StatusCode::CONFLICT,
            "strategy_version_conflict",
            "strategy changed; reload before updating",
        ),
        Err(error) => {
            tracing::error!(error = %error, "failed to update tenant strategy");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "strategy_storage_error",
                "strategy could not be updated",
            )
        }
    }
}

/// `DELETE /api/tenant/strategies/:id` — archive rather than destroy.
pub async fn archive(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let auth = match guard_manage(
        &state,
        &headers,
        Permission::BotStop,
        TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };
    let db = match database(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let strategy_id = match StrategyId::parse(&id) {
        Some(value) => value,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_strategy_id",
                "strategy id must be a UUID",
            )
        }
    };
    let result = sqlx::query(
        "UPDATE strategies
            SET status = 'archived', version = version + 1, updated_at = now()
          WHERE id = $1 AND organization_id = $2 AND status <> 'archived'",
    )
    .bind(strategy_id.as_uuid())
    .bind(auth.organization_id().as_uuid())
    .execute(db.pool())
    .await;

    match result {
        Ok(done) if done.rows_affected() == 1 => (
            StatusCode::OK,
            Json(json!({ "archived": true, "id": strategy_id.to_string(), "status": "archived" })),
        )
            .into_response(),
        Ok(_) => error_response(
            StatusCode::NOT_FOUND,
            "strategy_not_found",
            "strategy was not found or is already archived",
        ),
        Err(error) => {
            tracing::error!(error = %error, "failed to archive tenant strategy");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "strategy_storage_error",
                "strategy could not be archived",
            )
        }
    }
}
