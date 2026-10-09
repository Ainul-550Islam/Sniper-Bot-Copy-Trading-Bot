//! Tenant trading data plane — service assembly (PROMPT 3/10 #60/#68).
//!
//! [`TenantTradingDataPlane`] is the ONLY object the tenant trading
//! handlers talk to: it owns the [`Database`] handle and exposes the
//! tenant repositories built by the `bot-core` `trading_repository`
//! module. Every repository call it forwards carries the
//! authenticated organization from the [`SaasContext`] — there is no
//! method on this struct that accepts "just an id".

use std::sync::Arc;

use bot_core::db::Database;
use bot_core::tenant::OrganizationId;

use super::market_service::MarketService;
use bot_core::trading_repository::copy::{TenantCopyEventRepo, TenantCopyRead, TenantCopyWrite};
use bot_core::trading_repository::executions::{
    TenantClaimRepo, TenantExecutionRead, TenantExecutionWrite, TenantIdempotencyRepo,
    TenantLifecycleRepo,
};
use bot_core::trading_repository::intent::{
    TenantIntentRead, TenantIntentWrite, TenantRecoveryRepo,
};
use bot_core::trading_repository::orders::{TenantOrderRead, TenantOrderWrite};
use bot_core::trading_repository::polymarket::{
    TenantPolyRead, TenantPolyReconRepo, TenantPolyWrite,
};
use bot_core::trading_repository::positions::{
    TenantBalanceRepo, TenantPositionRead, TenantPositionWrite, TenantTradeRepo,
};
use bot_core::trading_repository::query_scope::TradingQueryScope;
use bot_core::trading_repository::reporting::TenantReportingRepo;
use bot_core::trading_repository::worker_claim::TenantWorkerClaimRepo;
use bot_core::trading_repository::write_scope::{TenantWriteScope, WriteOrigin};

/// The tenant trading data plane: repositories over one database.
/// Shared as `Arc<TenantTradingDataPlane>` — every accessor borrows.
pub struct TenantTradingDataPlane {
    db: Arc<Database>,
    /// Live market-data aggregator (GAP-MAP P1): real provider feeds with
    /// a TTL cache; replaces the old static fake catalog.
    markets: Arc<MarketService>,
    orders_read: TenantOrderRead,
    orders_write: TenantOrderWrite,
    executions_read: TenantExecutionRead,
    executions_write: TenantExecutionWrite,
    claims: TenantClaimRepo,
    lifecycle: TenantLifecycleRepo,
    idempotency: TenantIdempotencyRepo,
    positions_read: TenantPositionRead,
    positions_write: TenantPositionWrite,
    trades: TenantTradeRepo,
    balances: TenantBalanceRepo,
    intents_read: TenantIntentRead,
    intents_write: TenantIntentWrite,
    recovery: TenantRecoveryRepo,
    copy_read: TenantCopyRead,
    copy_write: TenantCopyWrite,
    copy_events: TenantCopyEventRepo,
    poly_read: TenantPolyRead,
    poly_write: TenantPolyWrite,
    poly_recon: TenantPolyReconRepo,
    reporting: TenantReportingRepo,
    worker_claims: TenantWorkerClaimRepo,
}

impl TenantTradingDataPlane {
    /// Build the data plane over an attached database.
    pub fn new(db: Arc<Database>, cfg: &bot_core::config::Config) -> Self {
        TenantTradingDataPlane {
            markets: MarketService::from_config(cfg),
            orders_read: TenantOrderRead::new(db.clone()),
            orders_write: TenantOrderWrite::new(db.clone()),
            executions_read: TenantExecutionRead::new(db.clone()),
            executions_write: TenantExecutionWrite::new(db.clone()),
            claims: TenantClaimRepo::new(db.clone()),
            lifecycle: TenantLifecycleRepo::new(db.clone()),
            idempotency: TenantIdempotencyRepo::new(db.clone()),
            positions_read: TenantPositionRead::new(db.clone()),
            positions_write: TenantPositionWrite::new(db.clone()),
            trades: TenantTradeRepo::new(db.clone()),
            balances: TenantBalanceRepo::new(db.clone()),
            intents_read: TenantIntentRead::new(db.clone()),
            intents_write: TenantIntentWrite::new(db.clone()),
            recovery: TenantRecoveryRepo::new(db.clone()),
            copy_read: TenantCopyRead::new(db.clone()),
            copy_write: TenantCopyWrite::new(db.clone()),
            copy_events: TenantCopyEventRepo::new(db.clone()),
            poly_read: TenantPolyRead::new(db.clone()),
            poly_write: TenantPolyWrite::new(db.clone()),
            poly_recon: TenantPolyReconRepo::new(db.clone()),
            reporting: TenantReportingRepo::new(db.clone()),
            worker_claims: TenantWorkerClaimRepo::new(db.clone()),
            db,
        }
    }

    /// The read scope for the authenticated tenant.
    pub fn read_scope(&self, organization_id: OrganizationId) -> TradingQueryScope {
        TradingQueryScope::new(organization_id)
    }

    /// The write scope for the authenticated principal.
    pub fn write_scope(
        &self,
        organization_id: OrganizationId,
        actor: &str,
    ) -> Result<TenantWriteScope, bot_core::error::BotError> {
        TenantWriteScope::new(organization_id, actor, WriteOrigin::Http)
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    pub fn db(&self) -> &Arc<Database> {
        &self.db
    }

    /// The live market-data aggregator (GAP-MAP P1).
    pub fn markets(&self) -> &Arc<MarketService> {
        &self.markets
    }

    pub fn orders_read(&self) -> &TenantOrderRead {
        &self.orders_read
    }

    pub fn orders_write(&self) -> &TenantOrderWrite {
        &self.orders_write
    }

    pub fn executions_read(&self) -> &TenantExecutionRead {
        &self.executions_read
    }

    pub fn executions_write(&self) -> &TenantExecutionWrite {
        &self.executions_write
    }

    pub fn claims(&self) -> &TenantClaimRepo {
        &self.claims
    }

    pub fn lifecycle(&self) -> &TenantLifecycleRepo {
        &self.lifecycle
    }

    pub fn idempotency(&self) -> &TenantIdempotencyRepo {
        &self.idempotency
    }

    pub fn positions_read(&self) -> &TenantPositionRead {
        &self.positions_read
    }

    pub fn positions_write(&self) -> &TenantPositionWrite {
        &self.positions_write
    }

    pub fn trades(&self) -> &TenantTradeRepo {
        &self.trades
    }

    pub fn balances(&self) -> &TenantBalanceRepo {
        &self.balances
    }

    pub fn intents_read(&self) -> &TenantIntentRead {
        &self.intents_read
    }

    pub fn intents_write(&self) -> &TenantIntentWrite {
        &self.intents_write
    }

    pub fn recovery(&self) -> &TenantRecoveryRepo {
        &self.recovery
    }

    pub fn copy_read(&self) -> &TenantCopyRead {
        &self.copy_read
    }

    pub fn copy_write(&self) -> &TenantCopyWrite {
        &self.copy_write
    }

    pub fn copy_events(&self) -> &TenantCopyEventRepo {
        &self.copy_events
    }

    pub fn poly_read(&self) -> &TenantPolyRead {
        &self.poly_read
    }

    pub fn poly_write(&self) -> &TenantPolyWrite {
        &self.poly_write
    }

    pub fn poly_recon(&self) -> &TenantPolyReconRepo {
        &self.poly_recon
    }

    pub fn reporting(&self) -> &TenantReportingRepo {
        &self.reporting
    }

    pub fn worker_claims(&self) -> &TenantWorkerClaimRepo {
        &self.worker_claims
    }
}
