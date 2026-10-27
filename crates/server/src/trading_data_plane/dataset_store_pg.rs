//! Postgres METADATA store for recorded launch datasets (BATCH 02).
//!
//! This is the durable, queryable system of record for the GLOBAL market-data
//! tables created by migrations 0063–0065. The chunk BYTES still live in the
//! [`FsDatasetStore`]; this store owns only the metadata rows:
//! `recorded_launch_datasets`, `recorded_launch_dataset_chunks`, and the
//! append-only `recorded_launch_dataset_verifications` audit log.
//!
//! ## Authoritative honesty
//! This store NEVER decides authority. `record_verification` takes a
//! [`VerificationReport`] produced by `module_sniper::dataset::verifier` and
//! persists `report.authoritative`. The 0063 CHECK constraint independently
//! forbids `authoritative = true` unless `status = 'verified' AND source =
//! 'recorded'`, so even a buggy writer cannot forge an authoritative row.

use sqlx::{PgPool, Row};
use uuid::Uuid;

use bot_core::error::{BotError, BotResult};

use module_sniper::dataset::format::{DatasetManifest, DatasetStatus};
use module_sniper::dataset::policy::VerifierPolicy;
use module_sniper::dataset::store::FsDatasetStore;
use module_sniper::dataset::verifier::{verify, VerificationOutcome, VerificationReport};

/// One `recorded_launch_datasets` row (the columns the lifecycle cares about).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DatasetRow {
    pub dataset_id: Uuid,
    pub source: String,
    pub status: String,
    pub authoritative: bool,
    pub record_count: i64,
    pub sha256_root: String,
    pub schema_version: i32,
}

fn parse_id(id: &str) -> BotResult<Uuid> {
    Uuid::parse_str(id)
        .map_err(|e| BotError::InvalidArgument(format!("invalid dataset_id {id:?}: {e}")))
}

fn db_err(e: sqlx::Error) -> BotError {
    BotError::db(e.to_string())
}

/// Async metadata store over a `sqlx` `PgPool`.
#[derive(Clone)]
pub struct PgDatasetStore {
    pool: PgPool,
}

impl PgDatasetStore {
    pub fn new(pool: PgPool) -> Self {
        PgDatasetStore { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Register a dataset in status `recording`. `authoritative` is forced
    /// false — a self-declared flag from the manifest is never trusted.
    pub async fn create(&self, manifest: &DatasetManifest) -> BotResult<()> {
        let id = parse_id(&manifest.dataset_id)?;
        sqlx::query(
            "INSERT INTO recorded_launch_datasets \
             (dataset_id, name, source, cluster, schema_version, status, authoritative) \
             VALUES ($1, $2, $3, $4, $5, 'recording', false) \
             ON CONFLICT (dataset_id) DO NOTHING",
        )
        .bind(id)
        .bind(&manifest.name)
        .bind(manifest.source.as_str())
        .bind(&manifest.cluster)
        .bind(manifest.schema_version as i32)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// Seal a dataset: persist its summary + chunk hash-chain metadata.
    /// Idempotent (chunk inserts use ON CONFLICT DO NOTHING).
    pub async fn seal(&self, manifest: &DatasetManifest) -> BotResult<()> {
        let id = parse_id(&manifest.dataset_id)?;
        sqlx::query(
            "UPDATE recorded_launch_datasets \
             SET status = 'sealed', slot_start = $2, slot_end = $3, \
                 record_count = $4, sha256_root = $5, sealed_at = now(), \
                 authoritative = false \
             WHERE dataset_id = $1",
        )
        .bind(id)
        .bind(manifest.slot_start as i64)
        .bind(manifest.slot_end as i64)
        .bind(manifest.record_count as i64)
        .bind(&manifest.sha256_root)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;

        for c in &manifest.chunks {
            sqlx::query(
                "INSERT INTO recorded_launch_dataset_chunks \
                 (dataset_id, chunk_index, first_slot, last_slot, record_count, \
                  byte_len, sha256, prev_sha256) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
                 ON CONFLICT (dataset_id, chunk_index) DO NOTHING",
            )
            .bind(id)
            .bind(c.chunk_index as i64)
            .bind(c.first_slot as i64)
            .bind(c.last_slot as i64)
            .bind(c.record_count as i64)
            .bind(c.byte_len as i64)
            .bind(&c.sha256)
            .bind(&c.prev_sha256)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        }
        Ok(())
    }

    /// Persist a verifier result and update the dataset status. This is the
    /// ONLY method that may flip `authoritative`, and it does so exclusively
    /// from `report.authoritative` (the verifier's decision).
    pub async fn record_verification(&self, report: &VerificationReport) -> BotResult<()> {
        let id = parse_id(&report.dataset_id)?;
        sqlx::query(
            "INSERT INTO recorded_launch_dataset_verifications \
             (dataset_id, verifier_version, hash_chain_ok, gaps_found, \
              max_slot_gap, coverage_ok, outcome, detail) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(id)
        .bind(&report.verifier_version)
        .bind(report.hash_chain_ok)
        .bind(report.gaps_found)
        .bind(report.max_slot_gap)
        .bind(report.coverage_ok)
        .bind(report.outcome.as_str())
        .bind(sqlx::types::Json(report.detail.clone()))
        .execute(&self.pool)
        .await
        .map_err(db_err)?;

        let new_status = if report.outcome == VerificationOutcome::Pass {
            DatasetStatus::Verified.as_str()
        } else {
            DatasetStatus::Failed.as_str()
        };
        sqlx::query(
            "UPDATE recorded_launch_datasets SET status = $2, authoritative = $3 \
             WHERE dataset_id = $1",
        )
        .bind(id)
        .bind(new_status)
        .bind(report.authoritative)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// Run the (sole) verifier over `files` and persist its result. Returns
    /// the report so callers can act on the verdict.
    pub async fn verify_and_record(
        &self,
        files: &FsDatasetStore,
        dataset_id: &str,
        policy: &VerifierPolicy,
    ) -> BotResult<VerificationReport> {
        let report = verify(files, dataset_id, policy)?;
        self.record_verification(&report).await?;
        Ok(report)
    }

    pub async fn get(&self, dataset_id: &str) -> BotResult<DatasetRow> {
        let id = parse_id(dataset_id)?;
        let row = sqlx::query_as::<_, DatasetRow>(
            "SELECT dataset_id, source, status, authoritative, record_count, \
                    sha256_root, schema_version \
             FROM recorded_launch_datasets WHERE dataset_id = $1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row)
    }

    pub async fn chunk_count(&self, dataset_id: &str) -> BotResult<i64> {
        let id = parse_id(dataset_id)?;
        let row = sqlx::query(
            "SELECT COUNT(*) AS n FROM recorded_launch_dataset_chunks WHERE dataset_id = $1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.get::<i64, _>("n"))
    }

    pub async fn verification_count(&self, dataset_id: &str) -> BotResult<i64> {
        let id = parse_id(dataset_id)?;
        let row = sqlx::query(
            "SELECT COUNT(*) AS n FROM recorded_launch_dataset_verifications WHERE dataset_id = $1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.get::<i64, _>("n"))
    }

    /// Delete a dataset row (chunk + verification rows cascade away).
    pub async fn delete(&self, dataset_id: &str) -> BotResult<()> {
        let id = parse_id(dataset_id)?;
        sqlx::query("DELETE FROM recorded_launch_datasets WHERE dataset_id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
