//! Durable tenant module configuration storage.
//!
//! Module configuration is stored as a namespaced document in `tenant_configs`.
//! This helper never supplies defaults: an absent module document is returned as
//! `null`, and updates are serialized under a row lock with a version bump.

use bot_core::db::Database;
use bot_core::tenant::OrganizationId;
use serde_json::{Map, Value};
use sqlx::Row;

fn next_version(previous: Option<i64>) -> Result<i64, sqlx::Error> {
    previous.map_or(Ok(1), |version| {
        version.checked_add(1).ok_or_else(|| {
            sqlx::Error::Protocol("tenant configuration version overflow".to_string())
        })
    })
}

pub async fn read_module(
    db: &Database,
    organization_id: OrganizationId,
    module: &str,
) -> Result<Option<Value>, sqlx::Error> {
    let row = sqlx::query("SELECT config FROM tenant_configs WHERE organization_id = $1")
        .bind(organization_id.as_uuid())
        .fetch_optional(db.pool())
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let config: Value = row.try_get("config")?;
    if !config.is_object() {
        return Err(sqlx::Error::Protocol(
            "tenant configuration document is not a JSON object".to_string(),
        ));
    }
    Ok(config
        .get("modules")
        .and_then(|modules| modules.get(module))
        .cloned())
}

pub async fn write_module(
    db: &Database,
    organization_id: OrganizationId,
    module: &str,
    patch: &Map<String, Value>,
    updated_by: &str,
) -> Result<Value, sqlx::Error> {
    let mut transaction = db.pool().begin().await?;
    let existing = sqlx::query(
        "SELECT version, config
           FROM tenant_configs
          WHERE organization_id = $1
          FOR UPDATE",
    )
    .bind(organization_id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await?;
    let from_version = match existing.as_ref() {
        Some(row) => Some(row.try_get::<i64, _>("version")?),
        None => None,
    };
    let to_version = next_version(from_version)?;

    let mut document = match existing.as_ref() {
        Some(row) => row.try_get::<Value, _>("config")?,
        None => Value::Object(Map::new()),
    };
    if !document.is_object() {
        return Err(sqlx::Error::Protocol(
            "tenant configuration document is not a JSON object".to_string(),
        ));
    }
    let root = document
        .as_object_mut()
        .expect("configuration document is an object");
    let modules = root
        .entry("modules")
        .or_insert_with(|| Value::Object(Map::new()));
    if !modules.is_object() {
        return Err(sqlx::Error::Protocol(
            "tenant configuration modules namespace is not a JSON object".to_string(),
        ));
    }
    let module_document = modules
        .as_object_mut()
        .expect("module configuration namespace was validated")
        .entry(module.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !module_document.is_object() {
        return Err(sqlx::Error::Protocol(
            "tenant module configuration is not a JSON object".to_string(),
        ));
    }
    let module_object = module_document
        .as_object_mut()
        .expect("module configuration is an object");
    for (key, value) in patch {
        module_object.insert(key.clone(), value.clone());
    }
    module_object.insert(
        "organization_id".to_string(),
        Value::String(organization_id.to_string()),
    );
    module_object.insert(
        "updated_at".to_string(),
        Value::String(chrono::Utc::now().to_rfc3339()),
    );
    root.insert(
        "updated_by".to_string(),
        Value::String(updated_by.to_string()),
    );
    root.insert(
        "updated_at".to_string(),
        Value::String(chrono::Utc::now().to_rfc3339()),
    );

    if existing.is_some() {
        sqlx::query(
            "UPDATE tenant_configs
                SET config = $2, version = $3, updated_by = $4, updated_at = now()
              WHERE organization_id = $1",
        )
        .bind(organization_id.as_uuid())
        .bind(&document)
        .bind(to_version)
        .bind(updated_by)
        .execute(&mut *transaction)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO tenant_configs (organization_id, version, config, updated_by, updated_at)
             VALUES ($1, $2, $3, $4, now())",
        )
        .bind(organization_id.as_uuid())
        .bind(to_version)
        .bind(&document)
        .bind(updated_by)
        .execute(&mut *transaction)
        .await?;
    }

    let changes = Value::Array(
        patch
            .iter()
            .map(|(key, value)| serde_json::json!({"op": "set", "path": key, "value": value}))
            .collect(),
    );
    sqlx::query(
        "INSERT INTO tenant_config_audit
             (organization_id, from_version, to_version, changes, updated_by)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(organization_id.as_uuid())
    .bind(from_version)
    .bind(to_version)
    .bind(changes)
    .bind(updated_by)
    .execute(&mut *transaction)
    .await?;

    transaction.commit().await?;
    Ok(document
        .get("modules")
        .and_then(|modules| modules.get(module))
        .cloned()
        .unwrap_or(Value::Null))
}

pub async fn clear_module(
    db: &Database,
    organization_id: OrganizationId,
    module: &str,
    updated_by: &str,
) -> Result<bool, sqlx::Error> {
    let mut transaction = db.pool().begin().await?;
    let existing = sqlx::query(
        "SELECT version, config
           FROM tenant_configs
          WHERE organization_id = $1
          FOR UPDATE",
    )
    .bind(organization_id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(row) = existing else {
        return Ok(false);
    };
    let from_version = Some(row.try_get::<i64, _>("version")?);
    let to_version = next_version(from_version)?;
    let mut document = row.try_get::<Value, _>("config")?;
    let Some(root) = document.as_object_mut() else {
        return Err(sqlx::Error::Protocol(
            "tenant configuration document is not a JSON object".to_string(),
        ));
    };
    let Some(modules_value) = root.get_mut("modules") else {
        return Ok(false);
    };
    let Some(modules) = modules_value.as_object_mut() else {
        return Err(sqlx::Error::Protocol(
            "tenant configuration modules namespace is not a JSON object".to_string(),
        ));
    };
    if modules.remove(module).is_none() {
        return Ok(false);
    }
    root.insert(
        "updated_by".to_string(),
        Value::String(updated_by.to_string()),
    );
    root.insert(
        "updated_at".to_string(),
        Value::String(chrono::Utc::now().to_rfc3339()),
    );
    sqlx::query(
        "UPDATE tenant_configs
            SET config = $2, version = $3, updated_by = $4, updated_at = now()
          WHERE organization_id = $1",
    )
    .bind(organization_id.as_uuid())
    .bind(&document)
    .bind(to_version)
    .bind(updated_by)
    .execute(&mut *transaction)
    .await?;
    let changes = Value::Array(vec![serde_json::json!({
        "op": "remove",
        "path": format!("modules/{module}"),
    })]);
    sqlx::query(
        "INSERT INTO tenant_config_audit
             (organization_id, from_version, to_version, changes, updated_by)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(organization_id.as_uuid())
    .bind(from_version)
    .bind(to_version)
    .bind(changes)
    .bind(updated_by)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(true)
}
