//! Leader ↔ follower reconciliation through the real bot (TASK 3 test 17):
//! links follow the position book, leader exits observed while we could not
//! mirror them are flagged (and, when enabled, sold through the normal exit
//! path), quantity drift refreshes the link, orphans and ambiguous entries
//! are surfaced, and every finding is metered and audited.

mod common;

use common::*;

use bot_core::execution::ExecutionIntent;
use bot_core::models::{BotModule, PositionStatus};
use bot_core::obs::metrics;
use module_copy::event::{CopyStage, RejectReason};
use module_copy::reconcile::{FindingKind, ReconAction};
use module_copy::recovery::{link_for, CopyStore, MemoryCopyStore};
use std::sync::Arc;

#[tokio::test]
async fn clean_state_reconciles_without_findings() {
    let mut w = CopyWorld::new(copy_config()).await;
    let store = Arc::new(MemoryCopyStore::new());
    w.attach_store(store.clone()).await;
    assert_eq!(w.process(&w.buy(1)).await.stage, CopyStage::Filled);
    let cfg = w.state.config_snapshot().await;
    let report = w.bot.reconcile_once(&cfg).await;
    assert!(report.is_clean(), "{:?}", report.findings);
    assert_eq!(report.links_checked, 1);
    assert_eq!(report.positions_checked, 1);
    // Per-leader exposure gauge published.
    let milli = metrics::global()
        .gauge("copy_leader_exposure_sol_milli", "", &[("leader", LEADER)])
        .get();
    assert!((40..=60).contains(&milli), "≈0.05 SOL, got {milli} milli");
}

#[tokio::test]
async fn sweeper_closed_positions_close_their_links() {
    let mut w = CopyWorld::new(copy_config()).await;
    let store = Arc::new(MemoryCopyStore::new());
    w.attach_store(store.clone()).await;
    assert_eq!(w.process(&w.buy(2)).await.stage, CopyStage::Filled);
    let p = w.state.open_positions_for(BotModule::Copy).await[0].clone();
    // Our own TP/SL sweeper (or an operator) closed the position.
    w.state
        .close_position(&p.id, PositionStatus::StoppedOut, "stop loss")
        .await;
    let cfg = w.state.config_snapshot().await;
    let mut events = w.state.events.subscribe();
    let report = w.bot.reconcile_once(&cfg).await;
    assert_eq!(report.count(FindingKind::LinkWithoutPosition), 1);
    assert_eq!(store.link(&p.id).unwrap().status, "closed");
    let audits = copy_audits(&mut events);
    assert!(audits
        .iter()
        .any(|(a, o)| a == "copy.recon.link_without_position" && o.contains("action=close_link")));
    // Second pass: nothing left to do.
    assert!(w.bot.reconcile_once(&cfg).await.is_clean());
}

#[tokio::test]
async fn leader_exit_seen_while_paused_is_flagged_then_mirrored_when_enabled() {
    let mut w = CopyWorld::new(copy_config()).await;
    let store = Arc::new(MemoryCopyStore::new());
    w.attach_store(store.clone()).await;
    assert_eq!(w.process(&w.buy(3)).await.stage, CopyStage::Filled);
    let p = w.state.open_positions_for(BotModule::Copy).await[0].clone();

    // mirror_exits off: the leader's sell is observed and journaled, not
    // acted on.
    let mut cfg = w.cfg.clone();
    cfg.copy.mirror_exits = false;
    w.reload(cfg.clone()).await;
    let sell = w.sell(4);
    assert_eq!(
        w.process(&sell).await.rejection.unwrap().reason,
        RejectReason::MirrorExitsDisabled
    );
    assert_eq!(store.event(&sell.event_id).unwrap().side, "sell");

    // Reconciliation flags it (auto exit off by default).
    let report = w.bot.reconcile_once(&cfg).await;
    assert_eq!(report.count(FindingKind::LeaderExitedWeHold), 1);
    let f = &report.findings[0];
    assert_eq!(f.action, ReconAction::Flag);
    assert_eq!(f.position_id.as_deref(), Some(p.id.as_str()));
    assert!(w
        .state
        .find_open(BotModule::Copy, &w.mint.to_string())
        .await
        .is_some());
    assert!(
        metrics::global()
            .counter(
                "copy_recon_findings_total",
                "",
                &[("kind", "leader_exited_we_hold")]
            )
            .get()
            >= 1
    );

    // Auto exit requires mirror_exits too: still a flag.
    cfg.copy.reconcile_auto_exit = true;
    w.reload(cfg.clone()).await;
    let report = w.bot.reconcile_once(&cfg).await;
    assert_eq!(report.findings[0].action, ReconAction::Flag);

    // Both on: the position is sold through the normal exit path and the
    // link closes with the finding as note.
    cfg.copy.mirror_exits = true;
    w.reload(cfg.clone()).await;
    let mut events = w.state.events.subscribe();
    let report = w.bot.reconcile_once(&cfg).await;
    assert!(matches!(
        report.findings[0].action,
        ReconAction::MirrorExit { fraction, .. } if fraction == 1.0
    ));
    assert!(w
        .state
        .find_open(BotModule::Copy, &w.mint.to_string())
        .await
        .is_none());
    let link = store.link(&p.id).unwrap();
    assert_eq!(link.status, "closed");
    assert!(link.note.as_deref().unwrap_or("").contains("leader sold"));
    let audits = copy_audits(&mut events);
    assert!(audits
        .iter()
        .any(|(a, _)| a == "copy.recon.mirror_exit_done"));
    assert!(w.bot.reconcile_once(&cfg).await.is_clean());
}

#[tokio::test]
async fn leader_rebuy_after_exit_cancels_the_finding() {
    let mut w = CopyWorld::new(copy_config()).await;
    let store = Arc::new(MemoryCopyStore::new());
    w.attach_store(store.clone()).await;
    assert_eq!(w.process(&w.buy(5)).await.stage, CopyStage::Filled);
    let mut cfg = w.cfg.clone();
    cfg.copy.mirror_exits = false;
    w.reload(cfg.clone()).await;
    let mut sell = w.sell(6);
    sell.observed_at = chrono::Utc::now() + chrono::Duration::milliseconds(10);
    w.process(&sell).await;
    let mut rebuy = w.buy(7);
    rebuy.observed_at = chrono::Utc::now() + chrono::Duration::milliseconds(20);
    // We already hold → ALREADY_MIRRORING, but the leader's re-entry is
    // journaled as a buy after the sell.
    assert_eq!(
        w.process(&rebuy).await.rejection.unwrap().reason,
        RejectReason::AlreadyMirroring
    );
    let report = w.bot.reconcile_once(&cfg).await;
    assert_eq!(
        report.count(FindingKind::LeaderExitedWeHold),
        0,
        "{:?}",
        report.findings
    );
}

#[tokio::test]
async fn quantity_drift_orphans_and_ambiguous_entries_are_surfaced() {
    let mut w = CopyWorld::new(copy_config()).await;
    let store = Arc::new(MemoryCopyStore::new());
    w.attach_store(store.clone()).await;
    assert_eq!(w.process(&w.buy(8)).await.stage, CopyStage::Filled);
    let p = w.state.open_positions_for(BotModule::Copy).await[0].clone();
    // A partial exit outside the copy engine's knowledge halves the position.
    w.state
        .with_position(&p.id, |pos| {
            pos.qty /= 2.0;
        })
        .await;
    // A copy position nobody can attribute, and one whose entry is still
    // live in the ledger.
    let mut orphan = bot_core::models::Position::new(
        "p-orphan-recon".into(),
        bot_core::models::TradeSource::Copy,
        bot_core::models::Venue::PumpFun,
        bot_core::models::ExecutionMode::Paper,
        "OrphanMint".into(),
        "ORPH".into(),
        "SOL".into(),
    );
    orphan.apply_buy(1.0, 0.001, 0.001);
    w.state.upsert_position(orphan).await;
    let sig = signature(99);
    bot_core::execution::ledger()
        .begin(ExecutionIntent {
            intent_id: "int_copy_recon_ambiguous".into(),
            module: "copy".into(),
            label: "copy-AMBIG".into(),
            wallet: w.wallet.pubkey.to_string(),
            symbol: "AmbigMint".into(),
        })
        .await
        .unwrap();
    bot_core::execution::ledger()
        .attach_submission(
            "int_copy_recon_ambiguous",
            &sig,
            Some("hash".into()),
            Some(1),
            0,
        )
        .await
        .unwrap();
    let mut amb = bot_core::models::Position::new(
        "p-ambiguous-recon".into(),
        bot_core::models::TradeSource::Copy,
        bot_core::models::Venue::PumpFun,
        bot_core::models::ExecutionMode::Live,
        "AmbigMint".into(),
        "AMB".into(),
        "SOL".into(),
    );
    amb.apply_buy(1.0, 0.001, 0.001);
    amb.copied_wallet = Some(LEADER.into());
    amb.entry_signature = Some(sig.clone());
    w.state.upsert_position(amb).await;
    store
        .upsert_link(link_for(
            "p-ambiguous-recon",
            LEADER,
            "AmbigMint",
            "ev-amb",
            &sig,
            Some("int_copy_recon_ambiguous"),
            1.0,
            1.0,
        ))
        .await;

    let cfg = w.state.config_snapshot().await;
    let report = w.bot.reconcile_once(&cfg).await;
    assert_eq!(
        report.count(FindingKind::QuantityMismatch),
        1,
        "{:?}",
        report.findings
    );
    assert_eq!(report.count(FindingKind::OrphanPosition), 1);
    assert_eq!(report.count(FindingKind::AmbiguousEntry), 1);
    let link = store.link(&p.id).unwrap();
    assert!(
        (link.follower_qty - p.qty / 2.0).abs() < 1e-9,
        "link refreshed"
    );
    assert!(link.last_reconciled_at.is_some());
    // The drift is fixed; orphan + ambiguous stay flagged (no action taken).
    let again = w.bot.reconcile_once(&cfg).await;
    assert_eq!(again.count(FindingKind::QuantityMismatch), 0);
    assert_eq!(again.count(FindingKind::OrphanPosition), 1);
    assert_eq!(again.count(FindingKind::AmbiguousEntry), 1);
    assert_eq!(
        w.state.open_positions_for(BotModule::Copy).await.len(),
        3,
        "reconciliation never sells on flags"
    );
}

#[tokio::test]
async fn unfollowed_leader_with_open_position_is_flagged_not_sold() {
    let mut w = CopyWorld::new(copy_config()).await;
    let store = Arc::new(MemoryCopyStore::new());
    w.attach_store(store.clone()).await;
    assert_eq!(w.process(&w.buy(9)).await.stage, CopyStage::Filled);
    let mut cfg = w.cfg.clone();
    cfg.copy.wallets.clear();
    cfg.copy.reconcile_auto_exit = true;
    w.reload(cfg.clone()).await;
    let report = w.bot.reconcile_once(&cfg).await;
    assert_eq!(report.count(FindingKind::LeaderRemovedWeHold), 1);
    assert_eq!(report.findings[0].action, ReconAction::Flag);
    assert_eq!(w.state.open_positions_for(BotModule::Copy).await.len(), 1);
}
