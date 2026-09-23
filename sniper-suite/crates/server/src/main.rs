//! sniper-suite — the control-plane binary.
//!
//! Loads config, opens the optional persistence backends (PostgreSQL,
//! Redis), builds the shared state + dedup + OMS + audit + auth layers,
//! spawns each enabled trading module (1 sniper, 2 copy, 3 polymarket,
//! 5 telegram) on its own task, runs the recovery/reconciliation workers,
//! and serves the Axum REST + WebSocket + dashboard control plane.
//! Module 4 (the staking program) is an on-chain program managed
//! separately; the server exposes its config but does not run it as a task.
//!
//! Shutdown is orchestrated: SIGTERM/SIGINT → coordinator → HTTP stops
//! accepting → module loops drain → journals flush → workers stop → pools
//! close, each phase under its own deadline.
//!
//! Everything defaults to **paper** trading: no transaction is broadcast
//! unless `execution.mode = "live"` and `execution.allow_live_trading = true`.

mod accounting;
mod api;
mod dashboard;
mod ha;
mod obs;
mod persist;
mod recon;
pub mod saas;
// TASK 7B — response-header hardening and the authenticated, tenant-scoped
// event stream. The two files live under src/security/; this inline parent
// module keeps the mandated file tree (no extra security/mod.rs).
mod security {
    pub mod headers;
    pub mod websocket;
}
mod ws;

use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

use bot_core::audit::AuditTrail;
use bot_core::auth::{sha256_hex, Authenticator, RateLimiter};
use bot_core::config::{AppConfig, Config, ObservabilityConfig};
use bot_core::dedup::{DedupBackend, DedupStore};
use bot_core::error::BotResult;
use bot_core::events::AppEvent;
use bot_core::lifecycle::{install_signal_handlers, Shutdown};
use bot_core::models::{BotModule, ExecutionMode};
use bot_core::obs::health::HealthRegistry;
use bot_core::oms::OrderManager;
use bot_core::state::AppState;

use solana_kit::rpc::Rpc;
use solana_kit::signer::{build_signer_registry, SignerRegistry};
use solana_kit::tokens::Wallet;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ---- config (loaded first so it can drive log level/format) ----------
    let app = match AppConfig::load() {
        Ok(a) => a,
        Err(e) => {
            // Tracing is not initialised yet; stderr is the only channel.
            eprintln!("failed to load config; falling back to defaults: {e}");
            AppConfig::from_defaults()
        }
    };
    let cfg = app.raw.clone();
    init_tracing(&cfg.observability);
    for w in &app.warnings {
        warn!(warning = %w, "config");
    }

    // Inject secrets from config into the environment so modules that read
    // their key material from env (telegram token, polygon key) find it. Env
    // values already present win over config, letting operators override.
    seed_secret_env(&cfg);

    // ---- lifecycle --------------------------------------------------------
    let shutdown = Shutdown::new();
    install_signal_handlers(shutdown.clone());

    // ---- core -------------------------------------------------------------
    let state = AppState::new(app);
    state.attach_shutdown(shutdown.clone());
    let rpc = Rpc::new(&cfg.network)?;
    let wallet = Arc::new(load_wallet(&cfg)?);

    // ---- signing subsystem (key-custody boundary) ---------------------------
    // Fails startup (never falls back) when the configured provider is not
    // supported by this build or a configured identity cannot be resolved.
    // Registers the primary trading wallet plus every [[signing.identities]]
    // entry; logs identity -> pubkey pairs (public information only).
    let signers = Arc::new(build_signer_registry(&cfg, Arc::clone(&wallet))?);

    // Seed demo balances in paper mode so sizing works with no funds.
    if cfg.execution.mode == ExecutionMode::Paper {
        state.set_balances(Some(10.0), Some(1000.0)).await;
    }

    // ---- persistence backends (optional, degrade loudly) -------------------
    let db = bot_core::db::open(&cfg.database).await?.map(Arc::new);
    let redis = bot_core::redis_kv::open(&cfg.redis).await?;

    // Dedup facade: L1 memory always; L2 per config.
    let dedup_backend = DedupBackend::parse(&cfg.storage.dedup_backend);
    let dedup_ttl = if dedup_backend == DedupBackend::Redis {
        cfg.redis.dedup_ttl_secs
    } else {
        cfg.storage.dedup_ttl_secs
    };
    let dedup = DedupStore::new(
        dedup_backend,
        redis.clone(),
        db.clone(),
        Duration::from_secs(dedup_ttl),
        cfg.storage.max_dedup_entries,
    );
    state.attach_dedup(dedup.clone());

    // OMS ledger (bounded mirror + DB durability when attached).
    let orders = OrderManager::new(db.clone(), cfg.storage.max_dedup_entries.max(1024));
    state.attach_orders(orders.clone());

    // TASK 5 — global ledger + global risk journals (Postgres when attached,
    // memory otherwise). Installed BEFORE restore / modules so no financial
    // event or decision can be booked against the wrong journal.
    accounting::attach(&state, db.as_ref()).await;

    // TASK 6 — HA store (workers, leases with fencing, durable cursors,
    // recovery journal). Installed BEFORE registration and recovery.
    ha::attach(&state, db.as_ref()).await;

    // Audit trail (hash-chained when the DB is attached).
    let audit = AuditTrail::new(db.clone(), state.events.clone());

    // Execution lifecycle ledger → durable rows (when the DB is attached) +
    // audit records for every money-relevant transition. Installed BEFORE
    // restore/modules so no transition can be missed.
    persist::ExecutionLedgerSink::new(db.clone(), audit.clone())
        .install()
        .await;

    // ---- distributed execution ownership (Prompt 3 §B/§C/§D/§K) -----------
    // Store authority: Postgres (authoritative, durable audit) > Redis
    // (leases only, used when no DB) > Memory (process-local: single
    // instance / paper only — NEVER an HA safety mechanism).
    let claim_store: Arc<dyn bot_core::ownership::ClaimStore> = if let Some(d) = &db {
        Arc::new(bot_core::db::claims::PostgresClaimStore::new(d.clone()))
    } else if let Some(kv) = &redis {
        bot_core::redis_ownership::redis_claim_store(kv.clone())
    } else {
        warn!(
            "no shared ownership backend ([database] and [redis] both off) — using PROCESS-LOCAL              execution claims; run EXACTLY ONE replica in this configuration"
        );
        Arc::new(bot_core::ownership::MemoryClaimStore::new())
    };
    if claim_store.backend() == "memory" && cfg.execution.mode == ExecutionMode::Live {
        warn!(
            "LIVE trading on a PROCESS-LOCAL ownership store: a multi-replica deployment WILL              double-execute. Configure [database] (authoritative) or [redis] before scaling out."
        );
    }
    let ownership = Arc::new(bot_core::ownership::OwnershipRegistry::new(
        claim_store,
        state.replica_id(),
        Duration::from_secs(cfg.ha.claim_lease_secs),
        Duration::from_secs(cfg.ha.claim_handoff_grace_secs),
    ));
    bot_core::obs::metrics::global()
        .gauge(
            "bot_replica_info",
            "Static replica identity gauge (always 1; labels carry the identity).",
            &[
                ("replica", state.replica_id()),
                ("backend", ownership.backend()),
            ],
        )
        .set(1);
    info!(
        replica = state.replica_id(),
        backend = ownership.backend(),
        lease_secs = cfg.ha.claim_lease_secs,
        handoff_grace_secs = cfg.ha.claim_handoff_grace_secs,
        "execution ownership registry ready"
    );

    // ---- runtime-flag propagation (§Q): kill switch + module gates --------
    let (flags_writer, flags_reader): (
        Arc<dyn bot_core::ownership::RuntimeFlagsWriter>,
        Arc<dyn bot_core::ownership::RuntimeFlagsReader>,
    ) = if let Some(d) = &db {
        let f = Arc::new(bot_core::db::claims::PostgresFlags::new(d.clone()));
        (f.clone(), f)
    } else if let Some(kv) = &redis {
        let f = Arc::new(bot_core::redis_ownership::RedisFlags::new(kv.clone()));
        (f.clone(), f)
    } else {
        let f = Arc::new(bot_core::ownership::MemoryFlags::default());
        (f.clone(), f)
    };
    state.attach_flags_writer(flags_writer);

    // Cluster-wide risk view (§Q gap closure): with Postgres attached, the
    // risk engine consults the shared positions table for open-position
    // capacity and today's realized PnL on every check, so global limits
    // converge within one query instead of within `[ha].book_sync_secs`.
    // Oracle failures fall back to the local view (risk never depends on
    // store availability).
    if let Some(d) = &db {
        state.attach_risk_oracle(Arc::new(bot_core::db::claims::PostgresRiskOracle::new(
            d.clone(),
        )));
        info!("global risk oracle attached (postgres): capacity + daily-loss converge per check");
    }

    // RBAC registry: None when no keys are configured anywhere (loopback
    // dev mode); otherwise the legacy key is auto-registered as owner.
    let authenticator = Authenticator::from_config(&cfg);
    let auth = if authenticator.len().await == 0 {
        None
    } else {
        Some(Arc::new(authenticator))
    };
    let limiter = RateLimiter::new(cfg.api.rate_limit_rpm);

    // JSONL journal (crash-safe history, always on).
    let store = match bot_core::storage::Store::open(&cfg.storage).await {
        Ok(s) => Some(s),
        Err(e) => {
            warn!(error = %e, "could not open the JSONL journal directory — continuing without it");
            None
        }
    };

    // ---- config versioning (durable deployments record what is running) ---
    if let Some(db) = &db {
        record_config_version(db, &cfg).await;
        register_wallet(db, &wallet).await;
    }

    // ---- restart recovery (BEFORE modules start trading) -------------------
    if let Some(db) = &db {
        persist::restore(db, &state).await;
    }
    // TASK 5 — rebuild the global ledger / risk state from the journal
    // against the restored positions (replay-safe; gaps reported, never
    // synthesised), then one reconciliation pass and the portfolio gauges.
    let accounting_report = accounting::recover(&state).await;
    state.events.publish(AppEvent::Lifecycle {
        ts: chrono::Utc::now(),
        message: format!("global ledger recovered: {}", accounting_report.summary()),
    });
    // TASK 6 — register this worker life and journal one deterministic
    // recovery action per unfinished order (§6). The venue has not been read
    // yet at this point, so nothing is ever finalized here: ambiguous work is
    // held for reconciliation, provably-unsent work is closed.
    let ambiguous_intents: Vec<String> = bot_core::execution::ledger()
        .open()
        .await
        .into_iter()
        .filter(|r| {
            matches!(
                r.state,
                bot_core::execution::ExecutionState::Submitted
                    | bot_core::execution::ExecutionState::Pending
            )
        })
        .map(|r| r.intent_id)
        .collect();
    let worker_state =
        ha::register_and_recover(&state, env!("CARGO_PKG_VERSION"), &ambiguous_intents).await;
    info!(state = %worker_state, worker = %state.replica_id(), "worker ready");

    // ---- observability: event pump + state sampler ------------------------
    let health = Arc::new(HealthRegistry::new());
    obs::spawn_event_pump(state.clone(), bot_core::obs::metrics::global());
    obs::spawn_state_sampler(
        state.clone(),
        rpc.clone(),
        Arc::clone(&health),
        bot_core::obs::metrics::global(),
        Duration::from_millis(cfg.observability.sample_interval_ms),
    );
    obs::spawn_persistence_sampler(
        db.clone(),
        cfg.database.required,
        redis.clone(),
        cfg.redis.required,
        Arc::clone(&health),
        bot_core::obs::metrics::global(),
        Duration::from_secs(15),
    );

    // ---- journal + persistence pumps ---------------------------------------
    let mut pump_handles = Vec::new();
    if let Some(store) = &store {
        pump_handles.push(persist::JournalPump::spawn(
            store.clone(),
            state.clone(),
            shutdown.clone(),
        ));
    }
    if let Some(db) = &db {
        pump_handles.push(persist::PersistencePump::spawn(
            db.clone(),
            state.clone(),
            shutdown.clone(),
        ));
    }

    info!(
        mode = %cfg.execution.mode.as_str(),
        live_allowed = cfg.execution.live_allowed(),
        cluster = %cfg.network.cluster,
        wallet = %wallet.pubkey,
        database = if db.is_some() { "postgres" } else { "none" },
        redis = if redis.is_some() { "on" } else { "off" },
        dedup = dedup.backend().as_str(),
        auth_keys = auth.as_ref().map(|_a| "on").unwrap_or("off"),
        "starting sniper-suite"
    );
    state.events.publish(bot_core::events::AppEvent::Lifecycle {
        ts: chrono::Utc::now(),
        message: format!(
            "sniper-suite starting in {} mode",
            cfg.execution.mode.as_str()
        ),
    });
    audit
        .success("system", "startup", Some(cfg.execution.mode.as_str()))
        .await;

    // ---- recovery / reconciliation workers ---------------------------------
    let mut worker_handles = Vec::new();
    if let Some(db) = &db {
        use bot_core::recovery::{run_maintenance, startup_reconcile, RecoveryWorker};
        use recon::{IntentTruth, PolymarketOrderTruth, SolanaPositionTruth, SolanaTxTruth};

        let mut worker = RecoveryWorker::new(db.clone(), shutdown.clone());
        worker.register(Arc::new(IntentTruth::new(db.clone())));
        worker.register(Arc::new(SolanaTxTruth::new(
            rpc.clone(),
            state.clone(),
            db.clone(),
        )));
        worker.register(Arc::new(SolanaPositionTruth::new(
            rpc.clone(),
            state.clone(),
            db.clone(),
            wallet.pubkey.to_string(),
        )));
        if cfg.polymarket.enabled {
            if let Some(poly) = PolymarketOrderTruth::new(state.clone(), db.clone()).await {
                // Orders: CLOB status truth. Positions: settled CTF balance
                // truth (same PolyBot handle, same correction policy as the
                // Solana position source).
                worker.register(Arc::new(poly.position_truth()));
                worker.register(Arc::new(poly));
            }
        }
        let worker = Arc::new(worker);

        // ---- startup reconciliation gate (§H) ------------------------------
        // Resolve pending claims against external truth BEFORE any trading
        // module spawns, then disable the modules whose claims are still
        // unresolved: a module must never trade on state the venue has not
        // confirmed (e.g. book says 100 tokens, chain says 0).
        // Crash point C (§I): pre-broadcast intents never linked to a
        // signature become `intent` claims BEFORE the gate pass, so the gate
        // sees them and entry-gates their symbols. Anything still pending
        // from a previous life of this process is an orphan by definition.
        let orphans = bot_core::recovery::sweep_orphan_intents(db, Duration::from_secs(30)).await;
        if !orphans.is_empty() {
            warn!(
                count = orphans.len(),
                "orphaned pre-broadcast intents found at startup (ambiguous; gated, never resubmitted)"
            );
        }

        let report = startup_reconcile(
            &worker,
            cfg.recovery.startup_batch,
            Duration::from_secs(cfg.recovery.startup_reconcile_secs),
        )
        .await;
        state.set_recon_unresolved(report.unresolved.clone()).await;
        if cfg.recovery.block_modules_on_unresolved && report.total_unresolved() > 0 {
            block_for_unresolved(&state, db, &report.unresolved, &audit).await;
        }

        // TASK 6 — the venue/chain reconciliation worker is a CLUSTER
        // SINGLETON: exactly one worker may resolve claims, or two replicas
        // would race on the same reconciliation item. The lease is fenced
        // before every sweep; a worker that lost it steps down instead of
        // continuing to mutate shared state.
        {
            let w = worker.clone();
            worker_handles.push(
                ha::LeasedWorker::new(
                    state.clone(),
                    bot_core::ha::LeaseRole::Reconciliation,
                    Duration::from_secs(30),
                )
                .spawn(shutdown.clone(), move |_state, _guard| {
                    let w = w.clone();
                    async move {
                        w.run_once(32).await;
                    }
                }),
            );
        }

        // Keep the reconciliation backlog snapshot fresh for /api/status,
        // Telegram /status and the metrics gauge (60 s cadence, cheap count).
        {
            let db3 = db.clone();
            let state3 = state.clone();
            let shutdown3 = shutdown.clone();
            worker_handles.push(tokio::spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(60));
                ticker.tick().await; // first tick is immediate — skip it
                loop {
                    tokio::select! {
                        _ = shutdown3.wait() => break,
                        _ = ticker.tick() => {
                            use bot_core::db::repo::ReconRepo;
                            match ReconRepo::new(db3.clone()).unresolved_counts(false).await {
                                Ok(rows) => {
                                    bot_core::obs::metrics::global()
                                        .gauge(
                                            "bot_reconciliation_unresolved",
                                            "Reconciliation claims not yet resolved, by kind.",
                                            &[],
                                        )
                                        .set(rows.iter().map(|(_, n)| *n).sum::<i64>().max(0));
                                    state3.set_recon_unresolved(rows).await;
                                    // Per-symbol gate refresh: claims resolved
                                    // since the last sample unblock their
                                    // symbols automatically; new claims gate.
                                    match ReconRepo::new(db3.clone())
                                        .list_unresolved_items(true, 1000)
                                        .await
                                    {
                                        Ok(items) => {
                                            let mut syms: Vec<String> = Vec::new();
                                            for (kind, subj) in &items {
                                                if let Some(sym) =
                                                    symbol_for_claim(&db3, &state3, kind, subj).await
                                                {
                                                    if !syms.contains(&sym) {
                                                        syms.push(sym);
                                                    }
                                                }
                                            }
                                            syms.sort();
                                            state3.set_blocked_symbols(syms).await;
                                        }
                                        Err(e) => warn!(error = %e, "symbol gate refresh failed"),
                                    }
                                    // Runtime orphans (e.g. a failed link write)
                                    // join the queue with a wider age floor so
                                    // in-flight broadcasts are never swept.
                                    bot_core::recovery::sweep_orphan_intents(
                                        &db3,
                                        Duration::from_secs(120),
                                    )
                                    .await;
                                }
                                Err(e) => warn!(error = %e, "recon backlog sample failed"),
                            }
                        }
                    }
                }
            }));
        }

        // Periodic position re-verification (§J): re-arm a claim for every
        // open LIVE Solana position so the worker re-compares book quantity
        // against the aggregated on-chain balance. reopen_resolved only
        // touches resolved rows — parked (failed) claims stay with operators
        // and in-flight claims keep their backoff.
        {
            let db4 = db.clone();
            let state4 = state.clone();
            let shutdown4 = shutdown.clone();
            let recheck = Duration::from_secs(cfg.recovery.position_recheck_interval_secs.max(30));
            // Polymarket positions are verifiable on-chain only through the
            // CTF reader; without `[polymarket].ctf_rpc_url` nothing could
            // ever resolve such a claim, so none is raised.
            let poly_chain_truth =
                cfg.polymarket.enabled && !cfg.polymarket.ctf_rpc_url.trim().is_empty();
            // TASK 6 — cluster singleton under the `state_sync` lease: the
            // sweep enqueues reconciliation claims, and N replicas would
            // enqueue the same ones N times.
            worker_handles.push(
                ha::LeasedWorker::new(state4.clone(), bot_core::ha::LeaseRole::StateSync, recheck)
                    .spawn(shutdown4.clone(), move |state4, _guard| {
                        let db4 = db4.clone();
                        async move {
                            use bot_core::db::repo::ReconRepo;
                            use bot_core::models::Venue;
                            {
                                let repo = ReconRepo::new(db4.clone());
                                let mut queued = 0usize;
                                for p in state4.open_positions().await {
                                    if p.mode != ExecutionMode::Live {
                                        continue; // paper/simulate have no chain truth
                                    }
                                    let kind = match p.venue {
                                        Venue::Paper => continue, // no chain truth
                                        Venue::PolymarketClob => {
                                            if !poly_chain_truth {
                                                continue; // no CTF reader configured
                                            }
                                            "polymarket_position" // settled ERC-1155 balance
                                        }
                                        _ => "position", // Solana token balance
                                    };
                                    let _ = repo.reopen_resolved(kind, &p.id).await;
                                    if repo.enqueue(kind, &p.id).await.is_ok() {
                                        queued += 1;
                                    }
                                }
                                if queued > 0 {
                                    debug!(queued, "positions queued for on-chain re-verification");
                                }
                            }
                        }
                    }),
            );
        }

        // Hourly housekeeping (dedup TTLs, idempotency keys, aged events).
        // TASK 6 — cluster singleton under the `recovery` lease: retention
        // deletes must run once, not once per replica.
        {
            let db2 = db.clone();
            worker_handles.push(
                ha::LeasedWorker::new(
                    state.clone(),
                    bot_core::ha::LeaseRole::Recovery,
                    Duration::from_secs(3600),
                )
                .spawn(shutdown.clone(), move |_state, _guard| {
                    let db2 = db2.clone();
                    async move {
                        run_maintenance(&db2, 30).await;
                    }
                }),
            );
        }
    }

    // Runtime-flag sync (§Q): converge kill switch / module gates across
    // replicas. Staleness rules live in `AppState::apply_flag_sync`
    // (kill ON immediate; OFF/flags only when the shared row is newer than
    // the last local decision). Store failures keep the local view (§K:
    // a stale local view is safer than a reset one) and are metered.
    {
        let state2 = state.clone();
        let shutdown2 = shutdown.clone();
        let every = Duration::from_secs(cfg.ha.flag_sync_secs.max(1));
        worker_handles.push(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(every);
            ticker.tick().await; // first tick is immediate — skip it
            loop {
                tokio::select! {
                    _ = shutdown2.wait() => break,
                    _ = ticker.tick() => {
                        match flags_reader.read_all().await {
                            Ok(rows) => {
                                state2.apply_flag_sync(&rows).await;
                            }
                            Err(e) => {
                                bot_core::obs::metrics::global()
                                    .counter(
                                        "bot_distributed_flag_sync_errors_total",
                                        "Runtime-flag sync failures (local view kept).",
                                        &[],
                                    )
                                    .inc();
                                debug!(error = %e, "flag sync read failed — keeping local view");
                            }
                        }
                    }
                }
            }
            info!("runtime-flag sync stopped");
        }));
    }

    // Position-book sync (§Q): converge risk capacity across replicas.
    // Merge rule: insert positions this replica has never seen; overwrite a
    // local row only when the DB row is NEWER and the local row is not
    // terminal (never resurrect locally closed positions).
    if let Some(db2) = db.clone() {
        let state2 = state.clone();
        let shutdown2 = shutdown.clone();
        let every = Duration::from_secs(cfg.ha.book_sync_secs.max(5));
        worker_handles.push(tokio::spawn(async move {
            use bot_core::db::repo::PositionRepo;
            let repo = PositionRepo::new(db2);
            let mut ticker = tokio::time::interval(every);
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = shutdown2.wait() => break,
                    _ = ticker.tick() => {
                        match repo.list_open().await {
                            Ok(rows) => {
                                let (inserted, refreshed) = state2.merge_positions(rows).await;
                                if inserted + refreshed > 0 {
                                    debug!(inserted, refreshed, "position book synced from database");
                                }
                            }
                            Err(e) => {
                                debug!(error = %e, "book sync failed — keeping local view");
                            }
                        }
                    }
                }
            }
            info!("position book sync stopped");
        }));
    }

    // TASK 6 — worker heartbeat, stale-worker survey and readiness refresh.
    worker_handles.push(ha::spawn_heartbeat(
        state.clone(),
        shutdown.clone(),
        cfg.ha.heartbeat(),
    ));

    // TASK 5 + 6 — periodic accounting maintenance (flush pending journal
    // writes, reconcile module truth against the ledger, refresh gauges),
    // held by exactly ONE worker through the `accounting_maintenance`
    // singleton lease and fenced before every tick.
    if cfg.global_risk.accounting_reconcile_interval_secs > 0 {
        worker_handles.push(
            ha::LeasedWorker::new(
                state.clone(),
                bot_core::ha::LeaseRole::AccountingMaintenance,
                Duration::from_secs(cfg.global_risk.accounting_reconcile_interval_secs.max(5)),
            )
            .spawn(shutdown.clone(), |state, _guard| async move {
                accounting::maintenance_tick(&state).await;
            }),
        );
    }

    // ---- modules -----------------------------------------------------------
    // Write-ahead intent journal (§I crash point C): DB-backed sink handed to
    // the Solana trading modules when enabled and a durable backend exists.
    let intents: Option<Arc<dyn bot_core::recovery::IntentSink>> = if cfg.recovery.intent_journal {
        db.as_ref().map(|d| {
            Arc::new(recon::DbIntentSink::new(d.clone())) as Arc<dyn bot_core::recovery::IntentSink>
        })
    } else {
        None
    };
    // Durable copy journal (TASK 3): leaders, processed leader events and
    // leader ↔ follower links live in Postgres when a database is configured.
    let copy_store: Option<Arc<dyn module_copy::recovery::CopyStore>> = db.as_ref().map(|d| {
        Arc::new(recon::DbCopyStore::new(d.clone())) as Arc<dyn module_copy::recovery::CopyStore>
    });
    // Durable Polymarket journal (TASK 4): signals, venue orders, fills and
    // reconciliation findings live in Postgres when a database is configured.
    let poly_store: Option<Arc<dyn module_polymarket::store::PolyStore>> = db.as_ref().map(|d| {
        Arc::new(recon::DbPolyStore::new(d.clone())) as Arc<dyn module_polymarket::store::PolyStore>
    });
    let module_handles = spawn_modules(
        &state,
        &rpc,
        &wallet,
        &signers,
        &cfg,
        intents.as_ref(),
        copy_store.as_ref(),
        poly_store.as_ref(),
        &ownership,
    )
    .await;

    // ---- TASK 7A: SaaS control plane ---------------------------------------
    // The tenant store is created before the API so the deployment
    // organization exists on the first request. A single-tenant operator
    // keeps using their deployment key; it maps to this organization and
    // can never reach another one.
    let saas = Arc::new(saas::SaasStore::with_database(db.clone()).await?);
    if saas::ensure_deployment_organization(&saas).await.is_none() {
        anyhow::bail!("failed to initialize the deployment organization");
    }

    // ---- control plane -----------------------------------------------------
    let api_handle = if cfg.api.enabled {
        Some(serve_api(
            &state,
            &health,
            &cfg,
            auth,
            limiter,
            audit.clone(),
            db.clone(),
            store.clone(),
            Arc::clone(&saas),
            shutdown.clone(),
        )?)
    } else {
        info!("api disabled; idling (modules still run)");
        None
    };

    // Wait for the shutdown request (signals already wired to the
    // coordinator; the HTTP server also stops on it).
    shutdown.wait().await;
    let reason = shutdown.reason().unwrap_or_else(|| "unknown".into());
    info!(%reason, "sniper-suite shutting down");
    audit.success("system", "shutdown", Some(&reason)).await;

    // ---- ordered shutdown phases (each time-bounded) ------------------------
    // 1) HTTP server drains (its graceful-shutdown future is the coordinator).
    if let Some(handle) = api_handle {
        shutdown
            .run_phase("http-drain", Duration::from_secs(10), async move {
                let _ = handle.await;
            })
            .await;
    }
    // 2) Module loops observe the coordinator and finish in-flight work;
    //    recovery workers stop on the same signal.
    shutdown
        .run_phase("module-drain", Duration::from_secs(15), async move {
            for h in module_handles.into_iter().chain(worker_handles) {
                let _ = h.await;
            }
        })
        .await;
    // 2b) TASK 6 — HA drain: stop accepting, persist cursors, release every
    //     singleton lease so a standby takes over immediately, mark stopped.
    {
        let state_ha = state.clone();
        let reason_ha = reason.clone();
        shutdown
            .run_phase("ha-drain", Duration::from_secs(10), async move {
                ha::shutdown(&state_ha, &reason_ha).await;
            })
            .await;
    }
    // 3) Journal + persistence pumps flush their buffers and stop.
    shutdown
        .run_phase("pump-flush", Duration::from_secs(10), async move {
            for h in pump_handles {
                let _ = h.await;
            }
        })
        .await;
    // 4) Close the pools (waits for in-flight statements).
    if let Some(db) = &db {
        shutdown
            .run_phase("db-close", Duration::from_secs(10), db.close())
            .await;
    }

    info!("sniper-suite stopped cleanly");
    Ok(())
}

/// Record the running configuration (content-addressed) so every deployment
/// is answerable to "what config produced this behaviour?".
async fn record_config_version(db: &Arc<bot_core::db::Database>, cfg: &Config) {
    use bot_core::db::repo::{ConfigVersionRepo, SystemEventRepo};
    let snapshot = serde_json::to_value(cfg).unwrap_or_else(|_| serde_json::json!({}));
    // Hash the redacted snapshot: secrets are never in `Config` TOML fields,
    // but SecretConfig Option<String>s may hold env-injected values — strip.
    let mut redacted = snapshot.clone();
    if let Some(obj) = redacted.as_object_mut() {
        obj.insert("secrets".to_string(), serde_json::json!("<redacted>"));
    }
    let hash = sha256_hex(&redacted.to_string());
    let repo = ConfigVersionRepo::new(db.clone());
    match repo.record(&hash, &redacted, "startup", None).await {
        Ok(true) => info!(%hash, "config version recorded"),
        Ok(false) => debug_once("config unchanged since last startup"),
        Err(e) => warn!(error = %e, "config version recording failed"),
    }
    if let Err(e) = SystemEventRepo::new(db.clone())
        .append(
            "startup",
            None,
            "info",
            &format!("config sha256={hash}"),
            &serde_json::json!({}),
        )
        .await
    {
        warn!(error = %e, "startup system-event failed");
    }
}

fn debug_once(msg: &str) {
    tracing::debug!("{}", msg);
}

/// Register the hot wallet in the durable registry (public key only).
async fn register_wallet(db: &Arc<bot_core::db::Database>, wallet: &Wallet) {
    use bot_core::db::repo::WalletRepo;
    if let Err(e) = WalletRepo::new(db.clone())
        .upsert("hot", "solana", &wallet.pubkey.to_string(), "hot")
        .await
    {
        warn!(error = %e, "wallet registration failed");
    }
}

/// Spawn every enabled module on its own task. Returns the task handles so
/// the shutdown sequence can join (drain) them under a deadline.
#[allow(clippy::too_many_arguments)]
async fn spawn_modules(
    state: &bot_core::state::Shared,
    rpc: &Rpc,
    wallet: &Arc<Wallet>,
    signers: &Arc<SignerRegistry>,
    cfg: &Config,
    intents: Option<&Arc<dyn bot_core::recovery::IntentSink>>,
    copy_store: Option<&Arc<dyn module_copy::recovery::CopyStore>>,
    poly_store: Option<&Arc<dyn module_polymarket::store::PolyStore>>,
    ownership: &Arc<bot_core::ownership::OwnershipRegistry>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let mut handles = Vec::new();

    // Module 1 — sniper.
    if cfg.sniper.enabled {
        match module_sniper::Sniper::new(
            state.clone(),
            rpc.clone(),
            wallet.clone(),
            Some(Arc::clone(signers)),
        )
        .await
        {
            Ok(sniper) => {
                let sniper = match intents {
                    Some(sink) => sniper.with_intent_sink(Arc::clone(sink)),
                    None => sniper,
                };
                let sniper = sniper.with_ownership(Arc::clone(ownership));
                handles.push(tokio::spawn(async move { sniper.run().await }));
                info!("module 1 (sniper) spawned");
            }
            Err(e) => error!(error = %e, "module 1 (sniper) failed to initialise"),
        }
    }

    // Module 2 — copy trading.
    if cfg.copy.enabled {
        let mut copy = module_copy::CopyBot::new(
            state.clone(),
            rpc.clone(),
            wallet.clone(),
            Some(Arc::clone(signers)),
        )
        .await;
        if let Some(sink) = intents {
            copy = copy.with_intent_sink(Arc::clone(sink));
        }
        if let Some(store) = copy_store {
            copy = copy.with_copy_store(Arc::clone(store));
        }
        let mut copy = copy.with_ownership(Arc::clone(ownership));
        match copy.spawn_feed().await {
            Ok(feed) => {
                handles.push(tokio::spawn(async move {
                    if let Err(e) = copy.run(feed).await {
                        error!(error = %e, "module 2 (copy) stopped");
                    }
                }));
                info!("module 2 (copy) spawned");
            }
            Err(e) => error!(error = %e, "module 2 (copy) feed failed to start"),
        }
    }

    // Module 3 — polymarket.
    if cfg.polymarket.enabled {
        match module_polymarket::PolyBot::new(state.clone()).await {
            Ok(poly) => {
                let mut poly = poly.with_ownership(Arc::clone(ownership));
                if let Some(store) = poly_store {
                    poly = poly.with_store(Arc::clone(store));
                }
                handles.push(tokio::spawn(async move {
                    if let Err(e) = poly.run().await {
                        error!(error = %e, "module 3 (polymarket) stopped");
                    }
                }));
                info!("module 3 (polymarket) spawned");
            }
            Err(e) => error!(error = %e, "module 3 (polymarket) failed to initialise"),
        }
    }

    // Module 5 — telegram control.
    if cfg.telegram.enabled {
        match module_telegram::spawn(state.clone()).await {
            Ok(Some(handle)) => {
                handles.push(handle);
                info!("module 5 (telegram) spawned");
            }
            Ok(None) => info!("module 5 (telegram) disabled (no bot token)"),
            Err(e) => error!(error = %e, "module 5 (telegram) failed to initialise"),
        }
    }

    // Module 4 — staking program is on-chain; nothing to run here.
    if cfg.contract.program_id.is_some() {
        info!(program = ?cfg.contract.program_id, "module 4 (staking) program configured (managed off-process)");
    }

    handles
}

/// Is this bind host loopback-only (not reachable from other machines)?
fn is_loopback(host: &str) -> bool {
    matches!(
        host.trim(),
        "127.0.0.1" | "localhost" | "::1" | "[::1]" | ""
    )
}

/// Build and serve the Axum control plane. Returns the server handle; the
/// caller awaits it during the ordered shutdown (it stops on the
/// coordinator's signal via `with_graceful_shutdown`).
#[allow(clippy::too_many_arguments)]
fn serve_api(
    state: &bot_core::state::Shared,
    health: &Arc<HealthRegistry>,
    cfg: &Config,
    auth: Option<Arc<Authenticator>>,
    limiter: Arc<RateLimiter>,
    audit: Arc<AuditTrail>,
    db: Option<Arc<bot_core::db::Database>>,
    journal: Option<bot_core::storage::Store>,
    saas: Arc<saas::SaasStore>,
    shutdown: Arc<Shutdown>,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let api_key = bot_core::config::resolve_api_key(cfg);

    // Fail closed: never expose an unauthenticated control plane on a
    // reachable interface. Binding to a non-loopback host REQUIRES keys.
    if auth.is_none() && api_key.is_none() && !is_loopback(&cfg.api.bind_host) {
        anyhow::bail!(
            "refusing to bind the control API to non-loopback host '{}' without an API key; \
             set {} (or [[auth.keys]] / secrets.api_key), or bind to 127.0.0.1 behind a TLS reverse proxy",
            cfg.api.bind_host,
            cfg.api.api_key_env
        );
    }

    let api_state = api::ApiState {
        shared: state.clone(),
        api_key: api_key.clone(),
        auth,
        limiter,
        audit,
        db,
        journal,
        serve_dashboard: cfg.api.serve_dashboard,
        health: Arc::clone(health),
        metrics_enabled: cfg.observability.metrics_enabled,
        saas: Arc::clone(&saas),
    };
    let app = with_cors(api::router(api_state), &cfg.api.cors_origins);

    let addr = format!("{}:{}", cfg.api.bind_host, cfg.api.bind_port);
    let std_listener = std::net::TcpListener::bind(&addr)?;
    std_listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(std_listener)?;
    match &api_key {
        Some(_) => {
            info!(%addr, "control API + dashboard listening (role keys required for mutating routes and the event stream)")
        }
        None => {
            info!(%addr, "control API + dashboard listening (loopback-only, no API key set — dev mode)")
        }
    }

    let handle = tokio::spawn(async move {
        let svc = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
        if let Err(e) = axum::serve(listener, svc)
            .with_graceful_shutdown(async move { shutdown.wait().await })
            .await
        {
            error!(error = %e, "control API server stopped with an error");
        }
    });
    Ok(handle)
}

/// Attach a CORS layer from the configured origins.
fn with_cors(router: axum::Router, origins: &[String]) -> axum::Router {
    use tower_http::cors::{Any, CorsLayer};
    let layer = if origins.iter().any(|o| o.trim() == "*") || origins.is_empty() {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        let parsed: Vec<axum::http::HeaderValue> = origins
            .iter()
            .filter_map(|o| o.parse::<axum::http::HeaderValue>().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(parsed)
            .allow_methods(Any)
            .allow_headers(Any)
    };
    router.layer(layer)
}

/// Load the Solana wallet from config/env, or generate an ephemeral one.
fn load_wallet(cfg: &Config) -> BotResult<Wallet> {
    let spec = cfg
        .secrets
        .solana_keypair
        .clone()
        .or_else(|| std::env::var("SOLANA_KEYPAIR").ok())
        .unwrap_or_default();
    if spec.trim().is_empty() {
        warn!("no Solana keypair configured — generating an ephemeral wallet (paper only)");
        Ok(Wallet::generate())
    } else {
        Wallet::load(&spec)
    }
}

/// Push config secrets into the environment (without clobbering explicit env).
fn seed_secret_env(cfg: &Config) {
    if let Some(tok) = &cfg.secrets.telegram_bot_token {
        if std::env::var(&cfg.telegram.bot_token_env)
            .unwrap_or_default()
            .is_empty()
        {
            std::env::set_var(&cfg.telegram.bot_token_env, tok);
        }
    }
    if let Some(pk) = &cfg.secrets.polygon_private_key {
        if std::env::var(module_polymarket::PRIVATE_KEY_ENV)
            .unwrap_or_default()
            .is_empty()
        {
            std::env::set_var(module_polymarket::PRIVATE_KEY_ENV, pk);
        }
    }
    if let Some(key) = &cfg.secrets.solana_keypair {
        if std::env::var("SOLANA_KEYPAIR")
            .unwrap_or_default()
            .is_empty()
        {
            std::env::set_var("SOLANA_KEYPAIR", key);
        }
    }
}

/// Initialise tracing from the `[observability]` config section.
///
/// * `RUST_LOG` always wins over `observability.log_level` so operators can
///   override without touching files.
/// * `log_format = "json"` emits one structured JSON object per event (with
///   target/module and span fields such as `request_id`); `"text"` keeps the
///   human-readable development format.
/// * `EnvFilter` is not `Clone`, so the filter is built inside the closure and
///   each format arm gets its own instance.
fn init_tracing(cfg: &ObservabilityConfig) {
    let filter = || {
        EnvFilter::try_from_default_env()
            .or_else(|_| EnvFilter::try_new(cfg.log_level.trim()))
            .unwrap_or_else(|_| {
                eprintln!(
                    "invalid log filter (RUST_LOG / observability.log_level); falling back to info"
                );
                EnvFilter::new("info")
            })
    };
    let result = match cfg.log_format.as_str() {
        "json" => tracing_subscriber::fmt()
            .with_env_filter(filter())
            .json()
            .try_init(),
        _ => tracing_subscriber::fmt()
            .with_env_filter(filter())
            .with_target(false)
            .try_init(),
    };
    let _ = result;
}

/// Map a reconciliation claim kind to the trading modules that must not run
/// while claims of that kind are unresolved (§H). Conservative: unknown kinds
/// block every trading module.
/// Attribute one unresolved claim to a tradable symbol when possible:
/// intents carry their symbol; positions are keyed by symbol-bearing ids;
/// transactions / polymarket orders resolve through their order row.
/// `None` = not attributable (e.g. `balance:<addr>`) → module-level fallback.
async fn symbol_for_claim(
    db: &Arc<bot_core::db::Database>,
    state: &bot_core::state::Shared,
    kind: &str,
    subject: &str,
) -> Option<String> {
    use bot_core::db::repo::{IntentRepo, OrderRepo, PositionRepo, TransactionRepo};
    fn non_empty(s: String) -> Option<String> {
        if s.trim().is_empty() {
            None
        } else {
            Some(s)
        }
    }
    match kind {
        "intent" => IntentRepo::new(db.clone())
            .get(subject)
            .await
            .ok()
            .flatten()
            .map(|r| r.symbol)
            .and_then(non_empty),
        "position" | "polymarket_position" => {
            if let Some(p) = state.position(subject).await {
                if let Some(sym) = non_empty(p.symbol.clone()) {
                    return Some(sym);
                }
            }
            PositionRepo::new(db.clone())
                .get(subject)
                .await
                .ok()
                .flatten()
                .map(|p| p.symbol)
                .and_then(non_empty)
        }
        "transaction" => {
            let order_id = TransactionRepo::new(db.clone())
                .get_order_id(subject)
                .await
                .ok()
                .flatten()?;
            OrderRepo::new(db.clone())
                .get(&order_id)
                .await
                .ok()
                .flatten()
                .map(|o| o.symbol)
                .and_then(non_empty)
        }
        "polymarket_order" => OrderRepo::new(db.clone())
            .get(subject)
            .await
            .ok()
            .flatten()
            .map(|o| o.symbol)
            .and_then(non_empty),
        _ => None,
    }
}

/// Per-symbol startup gate (§H): every ACTIVE claim attributable to a symbol
/// gates only that symbol's ENTRIES (exits always allowed); only claims that
/// cannot be attributed fall back to the conservative module-wide disable.
/// Both paths are auditable (audit trail + event bus + logs).
async fn block_for_unresolved(
    state: &bot_core::state::Shared,
    db: &Arc<bot_core::db::Database>,
    unresolved: &[(String, i64)],
    audit: &AuditTrail,
) {
    let items = match bot_core::db::repo::ReconRepo::new(db.clone())
        .list_unresolved_items(true, 1000)
        .await
    {
        Ok(items) => items,
        Err(e) => {
            warn!(error = %e, "claim attribution failed — falling back to module-level blocking");
            block_modules_for_unresolved(state, unresolved, audit).await;
            return;
        }
    };
    let mut symbols: Vec<String> = Vec::new();
    let mut residual: Vec<(String, i64)> = Vec::new();
    for (kind, subject) in &items {
        match symbol_for_claim(db, state, kind, subject).await {
            Some(sym) => {
                if !symbols.contains(&sym) {
                    symbols.push(sym);
                }
            }
            None => {
                if let Some(entry) = residual.iter_mut().find(|(k, _)| k == kind) {
                    entry.1 += 1;
                } else {
                    residual.push((kind.clone(), 1));
                }
            }
        }
    }
    symbols.sort();
    state.set_blocked_symbols(symbols.clone()).await;
    if !symbols.is_empty() {
        warn!(symbols = ?symbols, "symbols entry-gated at startup: unresolved reconciliation claims");
        state.events.publish(AppEvent::Error {
            ts: chrono::Utc::now(),
            module: None,
            message: format!(
                "startup reconciliation: entries gated for {} symbol(s) [{}] until claims                  resolve (exits remain allowed)",
                symbols.len(),
                symbols.join(", ")
            ),
            fatal: false,
        });
        audit
            .denied(
                "recovery",
                "startup_symbol_gate",
                Some(&symbols.join(",")),
                "unresolved reconciliation claims (per-symbol entry gate)",
            )
            .await;
    }
    // Unattributable claims keep the conservative module-level behaviour.
    if !residual.is_empty() {
        block_modules_for_unresolved(state, &residual, audit).await;
    }
}

fn modules_for_recon_kind(kind: &str) -> &'static [BotModule] {
    match kind {
        "transaction" | "position" | "balance" | "intent" => &[BotModule::Sniper, BotModule::Copy],
        "polymarket_order" | "polymarket_position" => &[BotModule::Polymarket],
        // "order" (or anything future): cannot attribute to one module.
        _ => &[BotModule::Sniper, BotModule::Copy, BotModule::Polymarket],
    }
}

/// Disable trading modules with actively unresolved reconciliation claims so
/// they cannot trade on unverified state. Every block is auditable (audit
/// trail + event bus + log); re-enabling is an explicit operator action via
/// the API or Telegram once the backlog clears.
async fn block_modules_for_unresolved(
    state: &bot_core::state::Shared,
    unresolved: &[(String, i64)],
    audit: &AuditTrail,
) {
    let mut blocked: Vec<BotModule> = Vec::new();
    for (kind, count) in unresolved {
        if *count <= 0 {
            continue;
        }
        for m in modules_for_recon_kind(kind) {
            if !blocked.contains(m) {
                blocked.push(*m);
            }
        }
    }
    // Stable order for deterministic logs/audit (BotModule::ALL order).
    blocked.sort_by_key(|m| {
        BotModule::ALL
            .iter()
            .position(|a| a == m)
            .unwrap_or(usize::MAX)
    });
    for m in blocked {
        let was_enabled = state.set_enabled(m, false).await;
        let kinds: Vec<String> = unresolved
            .iter()
            .filter(|(k, n)| *n > 0 && modules_for_recon_kind(k).contains(&m))
            .map(|(k, n)| format!("{k}={n}"))
            .collect();
        let detail = kinds.join(", ");
        warn!(
            module = ?m,
            unresolved = %detail,
            previously_enabled = was_enabled,
            "module blocked at startup: unresolved reconciliation claims"
        );
        state.events.publish(AppEvent::Error {
            ts: chrono::Utc::now(),
            module: Some(m),
            message: format!(
                "startup reconciliation: module disabled until an operator re-enables it \
                 (unresolved claims: {detail})"
            ),
            fatal: false,
        });
        audit
            .denied(
                "recovery",
                "startup_module_block",
                Some(m.as_str()),
                &format!("unresolved reconciliation claims: {detail}"),
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::is_loopback;
    use super::{block_modules_for_unresolved, modules_for_recon_kind};
    use bot_core::audit::AuditTrail;
    use bot_core::config::{AppConfig, Config};
    use bot_core::models::BotModule;
    use bot_core::state::AppState;

    #[tokio::test]
    async fn startup_gate_blocks_only_affected_modules() {
        // §V24-style: unresolved Solana transaction claims block Sniper+Copy
        // but must leave unrelated modules (Polymarket) alone.
        let state = AppState::new(AppConfig {
            raw: Config::default(),
            source_path: None,
            warnings: Vec::new(),
        });
        for m in [BotModule::Sniper, BotModule::Copy, BotModule::Polymarket] {
            state.set_enabled(m, true).await;
        }
        let audit = AuditTrail::new(None, state.events.clone());
        block_modules_for_unresolved(
            &state,
            &[
                ("transaction".to_string(), 2i64),
                ("position".to_string(), 1i64),
            ],
            &audit,
        )
        .await;
        assert!(!state.module_status(BotModule::Sniper).await.enabled);
        assert!(!state.module_status(BotModule::Copy).await.enabled);
        assert!(
            state.module_status(BotModule::Polymarket).await.enabled,
            "unrelated module must keep running"
        );
    }

    #[tokio::test]
    async fn polymarket_claims_block_only_polymarket() {
        let state = AppState::new(AppConfig {
            raw: Config::default(),
            source_path: None,
            warnings: Vec::new(),
        });
        for m in [BotModule::Sniper, BotModule::Copy, BotModule::Polymarket] {
            state.set_enabled(m, true).await;
        }
        let audit = AuditTrail::new(None, state.events.clone());
        block_modules_for_unresolved(&state, &[("polymarket_order".to_string(), 1i64)], &audit)
            .await;
        assert!(state.module_status(BotModule::Sniper).await.enabled);
        assert!(state.module_status(BotModule::Copy).await.enabled);
        assert!(!state.module_status(BotModule::Polymarket).await.enabled);

        // An unresolved settled-balance claim is Module 3's alone as well.
        state.set_enabled(BotModule::Polymarket, true).await;
        block_modules_for_unresolved(&state, &[("polymarket_position".to_string(), 1i64)], &audit)
            .await;
        assert!(state.module_status(BotModule::Sniper).await.enabled);
        assert!(state.module_status(BotModule::Copy).await.enabled);
        assert!(!state.module_status(BotModule::Polymarket).await.enabled);
    }

    #[test]
    fn unknown_claim_kinds_block_everything_conservatively() {
        // An unattributable kind ("order" or future kinds) must fail safe.
        assert_eq!(modules_for_recon_kind("order").len(), 3);
        assert_eq!(modules_for_recon_kind("something_new").len(), 3);
        assert_eq!(modules_for_recon_kind("transaction").len(), 2);
        assert_eq!(modules_for_recon_kind("polymarket_order").len(), 1);
        // Polymarket position claims (settled CTF balance) gate only Module 3.
        assert_eq!(
            modules_for_recon_kind("polymarket_position"),
            &[BotModule::Polymarket]
        );
        // Intent claims (crash point C) are Solana-side money movement.
        assert_eq!(modules_for_recon_kind("intent").len(), 2);
    }

    #[test]
    fn loopback_hosts_are_recognised() {
        for h in [
            "127.0.0.1",
            "localhost",
            "::1",
            "[::1]",
            "",
            "  127.0.0.1  ",
        ] {
            assert!(is_loopback(h), "{h:?} should be treated as loopback");
        }
    }

    #[test]
    fn reachable_hosts_are_not_loopback() {
        // Every non-loopback bind must read as reachable so the control plane
        // fails closed (refuses to start) unless an API key is configured.
        for h in [
            "0.0.0.0",
            "::",
            "10.0.0.5",
            "192.168.1.10",
            "172.16.0.1",
            "8.8.8.8",
            "example.com",
            "127.0.0.2",
        ] {
            assert!(!is_loopback(h), "{h:?} must NOT be treated as loopback");
        }
    }
}
