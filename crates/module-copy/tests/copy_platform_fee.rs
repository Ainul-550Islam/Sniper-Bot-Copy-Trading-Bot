//! Direct-route fee instruction preparation; not a live-chain receipt test.
use bot_core::config::{AppConfig, PlatformFeeConfig};
use bot_core::events::AppEvent;
use bot_core::state::AppState;
use solana_kit::fee_transfer::{compute_fee, FeeSkipReason};
use solana_sdk::pubkey::Pubkey;

fn config(vault: String, bps: u64, min: u64) -> PlatformFeeConfig {
    PlatformFeeConfig {
        jupiter_fee_account: vault.clone(),
        vault,
        bps,
        min_fee_lamports: min,
    }
}

#[tokio::test]
async fn buy_and_sell_prepare_atomic_sol_transfer_and_audit() {
    let state = AppState::new(AppConfig::from_defaults());
    let payer = Pubkey::new_unique();
    let vault = Pubkey::new_unique();
    let cfg = config(vault.to_string(), 100, 1);
    let mut events = state.events.subscribe();
    for (intent, notional) in [("buy-intent", 1_000_000_000), ("sell-intent", 500_000_000)] {
        let ix = module_copy::platform_fee::direct_fee_instruction(
            &state, &payer, notional, &cfg, intent,
        )
        .unwrap()
        .unwrap();
        assert_eq!(ix.accounts[0].pubkey, payer);
        assert_eq!(ix.accounts[1].pubkey, vault);
        assert_eq!(ix.program_id, solana_sdk::system_program::id());
        match events.recv().await.unwrap().as_ref() {
            AppEvent::Audit {
                target, outcome, ..
            } => {
                assert_eq!(target.as_deref(), Some(intent));
                assert!(outcome.contains("Charged"));
                assert!(outcome.contains("status=prepared"));
            }
            e => panic!("unexpected event {e:?}"),
        }
    }
}

#[tokio::test]
async fn dust_and_disabled_are_audited_without_instruction() {
    let state = AppState::new(AppConfig::from_defaults());
    let payer = Pubkey::new_unique();
    let mut events = state.events.subscribe();
    let dust = config(Pubkey::new_unique().to_string(), 100, 100);
    assert_eq!(compute_fee(10, &dust).reason, FeeSkipReason::Dust);
    assert!(
        module_copy::platform_fee::direct_fee_instruction(&state, &payer, 10, &dust, "dust")
            .unwrap()
            .is_none()
    );
    let disabled = config("".into(), 0, 0);
    assert!(module_copy::platform_fee::direct_fee_instruction(
        &state, &payer, 10, &disabled, "disabled"
    )
    .unwrap()
    .is_none());
    for label in ["Dust", "Disabled"] {
        match events.recv().await.unwrap().as_ref() {
            AppEvent::Audit { outcome, .. } => assert!(outcome.contains(label)),
            e => panic!("unexpected event {e:?}"),
        }
    }
}

#[tokio::test]
async fn invalid_vault_cannot_silently_drop_a_due_fee() {
    let state = AppState::new(AppConfig::from_defaults());
    let mut events = state.events.subscribe();
    let bad = config("invalid vault".into(), 100, 1);
    let result = module_copy::platform_fee::direct_fee_instruction(
        &state,
        &Pubkey::new_unique(),
        1_000_000,
        &bad,
        "intent",
    );
    assert!(result.is_err());
    match events.recv().await.unwrap().as_ref() {
        AppEvent::Audit {
            target, outcome, ..
        } => {
            assert_eq!(target.as_deref(), Some("intent"));
            assert!(outcome.contains("reason=Charged"));
            assert!(outcome.contains("status=rejected_invalid_vault"));
            assert!(!outcome.contains("status=prepared"));
        }
        event => panic!("unexpected event {event:?}"),
    }
}

#[tokio::test]
async fn direct_fee_rejects_payer_as_vault_and_audits_the_reason() {
    let state = AppState::new(AppConfig::from_defaults());
    let payer = Pubkey::new_unique();
    let mut events = state.events.subscribe();
    let cfg = config(payer.to_string(), 100, 1);
    assert!(module_copy::platform_fee::direct_fee_instruction(
        &state,
        &payer,
        1_000_000,
        &cfg,
        "self-vault-intent"
    )
    .is_err());
    match events.recv().await.unwrap().as_ref() {
        AppEvent::Audit {
            target, outcome, ..
        } => {
            assert_eq!(target.as_deref(), Some("self-vault-intent"));
            assert!(outcome.contains("status=rejected_self_transfer"));
            assert!(!outcome.contains("status=prepared"));
        }
        event => panic!("unexpected event {event:?}"),
    }
}

#[test]
fn jupiter_fee_account_must_be_initialized_wsol_token_account() {
    use solana_sdk::account::Account;
    let mut data = vec![0u8; 165];
    data[..32].copy_from_slice(solana_kit::consts::WSOL_MINT.as_ref());
    data[108] = 1;
    let mut account = Account {
        owner: *solana_kit::consts::TOKEN_PROGRAM,
        data,
        ..Account::default()
    };
    assert!(module_copy::platform_fee::validate_wsol_fee_account(&account).is_ok());
    account.data[108] = 2;
    assert!(module_copy::platform_fee::validate_wsol_fee_account(&account).is_err());
    account.data[108] = 1;
    account.owner = Pubkey::new_unique();
    assert!(module_copy::platform_fee::validate_wsol_fee_account(&account).is_err());
    account.owner = *solana_kit::consts::TOKEN_PROGRAM;
    account.data[..32].copy_from_slice(Pubkey::new_unique().as_ref());
    assert!(module_copy::platform_fee::validate_wsol_fee_account(&account).is_err());
}

#[tokio::test]
async fn built_signed_transaction_contains_fee_transfer_for_buy_and_sell() {
    use solana_sdk::{
        hash::Hash,
        signature::{Keypair, Signer},
        transaction::Transaction,
    };
    let state = AppState::new(AppConfig::from_defaults());
    let payer = Keypair::new();
    let vault = Pubkey::new_unique();
    let cfg = config(vault.to_string(), 100, 1);
    for (intent, notional, expected) in [
        ("buy", 1_000_000_000, 10_000_000u64),
        ("sell", 500_000_000, 5_000_000u64),
    ] {
        // This is a signed offline fixture containing a simulated venue
        // instruction, not a production route or chain settlement proof.
        let venue_ix = solana_sdk::instruction::Instruction {
            program_id: Pubkey::new_unique(),
            accounts: vec![],
            data: vec![1],
        };
        let fee_ix = module_copy::platform_fee::direct_fee_instruction(
            &state,
            &payer.pubkey(),
            notional,
            &cfg,
            intent,
        )
        .unwrap()
        .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &[venue_ix, fee_ix],
            Some(&payer.pubkey()),
            &[&payer],
            Hash::new_unique(),
        );
        tx.verify().unwrap();
        assert_eq!(tx.message.instructions.len(), 2);
        let ix = &tx.message.instructions[1];
        let accounts = &tx.message.account_keys;
        assert_eq!(
            accounts[ix.program_id_index as usize],
            solana_sdk::system_program::id()
        );
        assert_eq!(accounts[ix.accounts[0] as usize], payer.pubkey());
        assert_eq!(accounts[ix.accounts[1] as usize], vault);
        assert_eq!(&ix.data[..4], &2u32.to_le_bytes());
        assert_eq!(&ix.data[4..], &expected.to_le_bytes());
    }
}

#[tokio::test]
async fn invalid_jupiter_account_is_audited_as_rejected_for_each_actor() {
    use bot_core::config::NetworkConfig;
    use solana_kit::jupiter::QuoteRequest;

    let state = AppState::new(AppConfig::from_defaults());
    let rpc = solana_kit::Rpc::new(&NetworkConfig::default()).unwrap();
    let mut events = state.events.subscribe();
    let mut cfg = config(Pubkey::new_unique().to_string(), 100, 1);
    cfg.jupiter_fee_account = "not-a-pubkey".into();
    for actor in ["copy", "telegram"] {
        let request = QuoteRequest::new(
            *solana_kit::consts::WSOL_MINT,
            Pubkey::new_unique(),
            1_000_000,
        );
        let result = module_copy::platform_fee::jupiter_fee_request(
            &state,
            &rpc,
            request,
            1_000_000,
            &cfg,
            "test-intent",
            actor,
        )
        .await;
        assert!(result.is_err());
        match events.recv().await.unwrap().as_ref() {
            AppEvent::Audit {
                action,
                target,
                outcome,
                ..
            } => {
                assert_eq!(action, &format!("{actor}.platform_fee.assessed"));
                assert_eq!(target.as_deref(), Some("test-intent"));
                assert!(outcome.contains("reason=Charged"));
                assert!(outcome.contains("status=rejected_jupiter_fee_preparation"));
                assert!(!outcome.contains("status=prepared"));
            }
            event => panic!("unexpected event {event:?}"),
        }
    }
}

#[path = "common/mod.rs"]
mod common;

#[tokio::test]
async fn real_copy_curve_pipeline_builds_fee_leg_for_buy_and_sell_on_mock_rpc() {
    use base64::Engine;
    use common::{live_copy_config, CopyWorld, SendBehaviour};
    use module_copy::event::CopyStage;
    use solana_sdk::{pubkey::Pubkey, transaction::VersionedTransaction};
    use std::sync::atomic::Ordering;

    fn fee_amount(tx: &VersionedTransaction, payer: Pubkey, vault: Pubkey) -> u64 {
        let keys = tx.message.static_account_keys();
        let fee_legs: Vec<_> = tx
            .message
            .instructions()
            .iter()
            .filter(|ix| {
                keys.get(ix.program_id_index as usize) == Some(&solana_sdk::system_program::id())
                    && ix.data.len() == 12
                    && ix.data[..4] == 2u32.to_le_bytes()
                    && ix.accounts.len() == 2
                    && keys.get(ix.accounts[0] as usize) == Some(&payer)
                    && keys.get(ix.accounts[1] as usize) == Some(&vault)
            })
            .collect();
        assert_eq!(
            fee_legs.len(),
            1,
            "exactly one atomic platform-fee transfer"
        );
        u64::from_le_bytes(fee_legs[0].data[4..12].try_into().unwrap())
    }
    fn audit_fee(
        events: &mut tokio::sync::broadcast::Receiver<std::sync::Arc<AppEvent>>,
    ) -> (u64, u64) {
        while let Ok(event) = events.try_recv() {
            if let AppEvent::Audit {
                action, outcome, ..
            } = event.as_ref()
            {
                if action == "copy.platform_fee.assessed" && outcome.contains("status=prepared") {
                    let get = |key: &str| {
                        outcome
                            .split_whitespace()
                            .find_map(|v| v.strip_prefix(key))
                            .unwrap()
                            .parse::<u64>()
                            .unwrap()
                    };
                    return (get("notional_lamports="), get("fee_lamports="));
                }
            }
        }
        panic!("missing prepared fee audit")
    }
    fn last_tx(world: &CopyWorld) -> VersionedTransaction {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(
                world
                    .node
                    .last_sent_transaction_b64()
                    .expect("mock node received transaction"),
            )
            .unwrap();
        bincode::deserialize(&raw).expect("valid signed transaction")
    }
    async fn verify_mock_confirmation(world: &CopyWorld, vault: Pubkey, expected: u64) {
        let signed = last_tx(world);
        let signature = signed.signatures[0];
        let rpc = common::mock_rpc(&world.url);
        let response = rpc.get_transaction(&signature).await.unwrap().unwrap();
        let proof = solana_kit::confirmed_fee::verify_confirmed_direct_fee(
            &response,
            &signature,
            &world.wallet.pubkey,
            &vault,
            expected,
        )
        .unwrap();
        assert_eq!(proof.lamports, expected);
    }

    let vault = Pubkey::new_unique();
    let mut cfg = live_copy_config();
    cfg.platform_fee = config(vault.to_string(), 100, 1);
    let mut world = CopyWorld::new(cfg.clone()).await;
    let mut events = world.state.events.subscribe();
    world.node.set_send(SendBehaviour::Accept);
    world.node.confirm.store(true, Ordering::SeqCst);
    let buy = world.buy(21);
    let outcome = world.process(&buy).await;
    assert_eq!(outcome.stage, CopyStage::Filled, "{:?}", outcome.rejection);
    let (notional, expected) = audit_fee(&mut events);
    assert_eq!(expected, compute_fee(notional, &cfg.platform_fee).lamports);
    assert_eq!(
        fee_amount(&last_tx(&world), world.wallet.pubkey, vault),
        expected
    );
    verify_mock_confirmation(&world, vault, expected).await;

    let sell = world.sell(22);
    let outcome = world.process(&sell).await;
    assert_eq!(
        outcome.stage,
        CopyStage::ExitMirrored,
        "{:?}",
        outcome.rejection
    );
    let (notional, expected) = audit_fee(&mut events);
    assert_eq!(expected, compute_fee(notional, &cfg.platform_fee).lamports);
    assert_eq!(
        fee_amount(&last_tx(&world), world.wallet.pubkey, vault),
        expected
    );
    verify_mock_confirmation(&world, vault, expected).await;

    let sends = world.node.sends.load(Ordering::SeqCst);
    let replay = world.process(&sell).await;
    assert_eq!(replay.stage, CopyStage::Rejected);
    assert_eq!(world.node.sends.load(Ordering::SeqCst), sends);
}

#[tokio::test]
async fn direct_verification_never_claims_unconfirmed_or_missing_transaction() {
    use common::{live_copy_config, CopyWorld};
    use solana_kit::execute::{ExecStatus, ExecutionResult};

    let world = CopyWorld::new(live_copy_config()).await;
    let rpc = common::mock_rpc(&world.url);
    let mut events = world.state.events.subscribe();
    let cfg = config(Pubkey::new_unique().to_string(), 100, 1);
    let mut result = ExecutionResult::empty("copy", "fee-intent", false);
    result.signature = common::signature(99);
    result.status = ExecStatus::SendUnknown;
    module_copy::platform_fee::audit_confirmed_direct_fee(
        &world.state,
        &rpc,
        &world.wallet.pubkey,
        1_000_000,
        &cfg,
        "fee-intent",
        &result,
    )
    .await;
    assert!(events.try_recv().is_err(), "ambiguous is not confirmed");
    result.status = ExecStatus::Confirmed;
    // A post-send kill switch changes the current state mode, not the
    // provenance of an already confirmed live execution.
    world.state.set_kill_switch(true, "after broadcast").await;
    while events.try_recv().is_ok() {}
    module_copy::platform_fee::audit_confirmed_direct_fee(
        &world.state,
        &rpc,
        &world.wallet.pubkey,
        1_000_000,
        &cfg,
        "fee-intent",
        &result,
    )
    .await;
    match events.try_recv().unwrap().as_ref() {
        AppEvent::Audit { outcome, .. } => {
            assert!(outcome.contains("status=unresolved_direct_leg"));
            assert!(!outcome.contains("verified_direct_leg"));
        }
        other => panic!("unexpected event {other:?}"),
    }
    assert!(
        world
            .state
            .ledger()
            .events()
            .await
            .iter()
            .all(|stored| stored.event.kind != bot_core::accounting::EventKind::Fee),
        "an unverified direct fee must not be posted to the ledger"
    );
}

#[tokio::test]
async fn confirmed_curve_buy_and_sell_audit_verified_fee_leg() {
    use common::{live_copy_config, CopyWorld, SendBehaviour};
    use module_copy::event::CopyStage;
    use std::sync::atomic::Ordering;

    let mut cfg = live_copy_config();
    cfg.platform_fee = config(Pubkey::new_unique().to_string(), 100, 1);
    let mut world = CopyWorld::new(cfg).await;
    let mut events = world.state.events.subscribe();
    world.node.set_send(SendBehaviour::Accept);
    world.node.confirm.store(true, Ordering::SeqCst);
    for is_buy in [true, false] {
        let input = if is_buy {
            world.buy(91)
        } else {
            world.sell(92)
        };
        let result = world.process(&input).await;
        assert_eq!(
            result.stage,
            if is_buy {
                CopyStage::Filled
            } else {
                CopyStage::ExitMirrored
            },
            "{:?}",
            result.rejection
        );
        let mut verified = false;
        let mut verified_lamports = None;
        while let Ok(event) = events.try_recv() {
            if let AppEvent::Audit {
                action, outcome, ..
            } = event.as_ref()
            {
                if action == "copy.platform_fee.confirmation"
                    && outcome.contains("status=verified_direct_leg")
                {
                    assert!(
                        outcome.contains("ledger=new"),
                        "verified fee must be posted to the global ledger: {outcome}"
                    );
                    verified = true;
                    verified_lamports = outcome
                        .split_whitespace()
                        .find_map(|value| value.strip_prefix("fee_lamports="))
                        .and_then(|value| value.parse::<u64>().ok());
                }
            }
        }
        assert!(
            verified,
            "confirmed direct fee leg must be independently checked"
        );
        let fee_events: Vec<_> = world
            .state
            .ledger()
            .events()
            .await
            .into_iter()
            .filter(|stored| stored.event.kind == bot_core::accounting::EventKind::Fee)
            .collect();
        assert_eq!(
            fee_events.len(),
            if is_buy { 1 } else { 2 },
            "a verified direct fee must produce exactly one ledger fee event per confirmed transaction"
        );
        let fee_sol = bot_core::maths::lamports_to_sol(
            verified_lamports.expect("verified fee amount must be audited"),
        );
        assert!(
            (fee_events.last().unwrap().event.fee - fee_sol).abs() < 1e-12,
            "ledger fee must use the exact verified lamports, not an estimate"
        );
    }
}

#[tokio::test]
async fn curve_pipeline_refuses_self_vault_before_buy_or_sell_broadcast() {
    use common::{live_copy_config, CopyWorld, SendBehaviour};
    use module_copy::event::CopyStage;
    use std::sync::atomic::Ordering;

    let mut world = CopyWorld::new(live_copy_config()).await;
    world.node.set_send(SendBehaviour::Accept);
    world.node.confirm.store(true, Ordering::SeqCst);
    let mut cfg = world.cfg.clone();
    cfg.platform_fee = config(world.wallet.pubkey.to_string(), 100, 1);
    world.reload(cfg.clone()).await;
    let rejected_buy = world.process(&world.buy(80)).await;
    assert_eq!(rejected_buy.stage, CopyStage::Failed);
    assert_eq!(world.node.sends.load(Ordering::SeqCst), 0);

    cfg.platform_fee = config(Pubkey::new_unique().to_string(), 100, 1);
    world.reload(cfg.clone()).await;
    assert_eq!(world.process(&world.buy(81)).await.stage, CopyStage::Filled);
    let sends = world.node.sends.load(Ordering::SeqCst);
    cfg.platform_fee = config(world.wallet.pubkey.to_string(), 100, 1);
    world.reload(cfg).await;
    let rejected_sell = world.process(&world.sell(82)).await;
    assert_eq!(rejected_sell.stage, CopyStage::Failed);
    assert_eq!(world.node.sends.load(Ordering::SeqCst), sends);
}

#[tokio::test]
async fn curve_pipeline_skips_disabled_and_dust_fees_without_fee_transfer() {
    use base64::Engine;
    use common::{live_copy_config, CopyWorld};
    use module_copy::event::CopyStage;
    use solana_sdk::transaction::VersionedTransaction;

    for (bps, min_fee, reason) in [(0, 1, "Disabled"), (100, u64::MAX, "Dust")] {
        let vault = Pubkey::new_unique();
        let mut cfg = live_copy_config();
        cfg.platform_fee = config(vault.to_string(), bps, min_fee);
        let mut world = CopyWorld::new(cfg).await;
        let mut events = world.state.events.subscribe();
        for is_buy in [true, false] {
            let event = if is_buy {
                world.buy(41)
            } else {
                world.sell(42)
            };
            let result = world.process(&event).await;
            assert_eq!(
                result.stage,
                if is_buy {
                    CopyStage::Filled
                } else {
                    CopyStage::ExitMirrored
                },
                "{:?}",
                result.rejection
            );
            let raw = base64::engine::general_purpose::STANDARD
                .decode(world.node.last_sent_transaction_b64().unwrap())
                .unwrap();
            let tx: VersionedTransaction = bincode::deserialize(&raw).unwrap();
            let keys = tx.message.static_account_keys();
            assert!(
                !tx.message.instructions().iter().any(|ix| {
                    keys.get(ix.program_id_index as usize)
                        == Some(&solana_sdk::system_program::id())
                        && ix.data.len() == 12
                        && ix.data[..4] == 2u32.to_le_bytes()
                        && ix.accounts.len() == 2
                        && keys.get(ix.accounts[1] as usize) == Some(&vault)
                }),
                "skipped fee must not send money to fee vault"
            );
            let mut found = false;
            while let Ok(event) = events.try_recv() {
                if let AppEvent::Audit {
                    action, outcome, ..
                } = event.as_ref()
                {
                    if action == "copy.platform_fee.assessed"
                        && outcome.contains(&format!("reason={reason}"))
                    {
                        found = true;
                    }
                }
            }
            assert!(found, "each skipped fee must emit its reason");
        }
    }
}
