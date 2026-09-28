//! Property tests for the pure parts of the sniper engine (TASK 2 §P.6).
//!
//! Every case draws many random inputs from a deterministic xorshift PRNG
//! (reproducible, no extra dependency) and checks an invariant that must hold
//! for *all* of them — the unit tests in the crate pin exact values, these
//! pin the shape of the rules:
//!
//! * event identity is a pure function of protocol + mint + pool + on-chain
//!   reference, and of nothing else;
//! * the dedup key never collides across protocols;
//! * the stage machine has no escape from a terminal stage and no shortcut
//!   past a stage;
//! * the safety gates are complete, ordered, monotone in their thresholds
//!   and strict-mode only ever adds failures;
//! * the slippage engine never asks for more than the hard maximum in an
//!   adaptive mode, and the price-impact model is bounded and monotone.

use chrono::{DateTime, Duration, TimeZone, Utc};
use solana_sdk::pubkey::Pubkey;

use bot_core::config::SniperConfig;
use bot_core::models::{LaunchFeed, TokenLaunch};

use module_sniper::event::{EventDefect, LaunchEvent, LaunchProtocol, SequenceTracker};
use module_sniper::gates::{self, GateOutcome, MarketSnapshot, ALL_GATES};
use module_sniper::pipeline::{Lifecycle, RejectReason, SniperStage};
use module_sniper::slippage::{self, SlippageError, SlippageInputs, SlippageMode};

const SOL: u64 = 1_000_000_000;
const ITERATIONS: usize = 400;

/// xorshift64* — deterministic, good enough to spray the input space.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }

    fn chance(&mut self, one_in: u64) -> bool {
        self.below(one_in) == 0
    }

    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn pubkey(&mut self) -> Pubkey {
        let mut b = [0u8; 32];
        for chunk in b.chunks_mut(8) {
            let v = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
        Pubkey::new_from_array(b)
    }

    fn signature(&mut self) -> String {
        let mut b = [0u8; 64];
        for chunk in b.chunks_mut(8) {
            let v = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
        bs58::encode(b).into_string()
    }

    fn protocol(&mut self) -> LaunchProtocol {
        match self.below(3) {
            0 => LaunchProtocol::PumpFun,
            1 => LaunchProtocol::PumpSwap,
            _ => LaunchProtocol::RaydiumAmmV4,
        }
    }

    fn feed(&mut self) -> LaunchFeed {
        match self.below(4) {
            0 => LaunchFeed::PumpPortal,
            1 => LaunchFeed::SolanaLogs,
            2 => LaunchFeed::TransactionSubscribe,
            _ => LaunchFeed::Manual,
        }
    }

    fn stage(&mut self) -> SniperStage {
        SniperStage::ALL[self.below(SniperStage::ALL.len() as u64) as usize]
    }
}

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 3, 1, 12, 0, 0).unwrap()
}

fn launch(rng: &mut Rng, mint: Pubkey, observed_at: DateTime<Utc>) -> TokenLaunch {
    TokenLaunch {
        mint: mint.to_string(),
        name: format!("Prop {}", rng.below(1_000_000)),
        symbol: format!("P{}", rng.below(10_000)),
        uri: None,
        creator: rng.pubkey().to_string(),
        pool: "bonding-curve".into(),
        initial_buy_sol: rng.unit() * 5.0,
        market_cap_sol: 20.0 + rng.unit() * 50.0,
        market_cap_usd: None,
        total_supply: Some(1_000_000_000.0),
        slot: Some(1 + rng.below(400_000_000)),
        signature: Some(rng.signature()),
        tx_type: Some("create".into()),
        observed_at,
        feed: rng.feed(),
        socials: None,
    }
}

/// A well-formed random event of a random protocol.
fn random_event(rng: &mut Rng) -> LaunchEvent {
    let mint = rng.pubkey();
    let mut ev = LaunchEvent::from_token_launch(
        launch(rng, mint, t0()),
        1 + rng.below(10_000),
        format!("raw-{:x}", rng.next_u64()),
    );
    ev.protocol = rng.protocol();
    if ev.protocol.requires_pool() {
        ev.pool = Some(rng.pubkey().to_string());
    }
    ev.event_ts = Some(t0() - Duration::milliseconds(rng.below(20_000) as i64));
    ev.source = rng.feed();
    ev.event_id = ev.compute_event_id();
    ev
}

fn snapshot(rng: &mut Rng) -> MarketSnapshot {
    let protocol = rng.protocol();
    let quote = if rng.chance(8) {
        0
    } else {
        rng.below(200 * SOL)
    };
    MarketSnapshot {
        protocol,
        pool: Some(rng.pubkey().to_string()),
        quote_reserve_lamports: quote,
        pricing_quote_reserve_lamports: if rng.chance(10) { 0 } else { quote + 30 * SOL },
        base_reserve_raw: rng.below(1_000_000_000_000_000),
        base_decimals: if rng.chance(12) {
            13 + rng.below(6) as u8
        } else {
            rng.below(13) as u8
        },
        total_supply_raw: if rng.chance(4) {
            None
        } else {
            Some(1_000_000_000_000_000)
        },
        fee_bps: rng.below(300),
        tradable: !rng.chance(6),
        tradable_detail: String::new(),
        pool_open_time: match (protocol, rng.below(3)) {
            (LaunchProtocol::RaydiumAmmV4, 0) => None,
            (LaunchProtocol::RaydiumAmmV4, 1) => Some((t0().timestamp() + 600) as u64),
            (LaunchProtocol::RaydiumAmmV4, _) => Some((t0().timestamp() - 600) as u64),
            _ => None,
        },
        mint_authority_revoked: match rng.below(3) {
            0 => None,
            1 => Some(false),
            _ => Some(true),
        },
        freeze_authority_revoked: match rng.below(3) {
            0 => None,
            1 => Some(false),
            _ => Some(true),
        },
        creator_initial_buy_sol: if rng.chance(3) {
            None
        } else {
            Some(rng.unit() * 10.0)
        },
        spot_price_sol: match rng.below(10) {
            0 => 0.0,
            1 => f64::NAN,
            _ => rng.unit() * 1e-6,
        },
        fetched_at: t0() - Duration::milliseconds(rng.below(5_000) as i64),
    }
}

fn gate_config(rng: &mut Rng) -> SniperConfig {
    SniperConfig {
        require_mint_authority_revoked: rng.chance(2),
        require_freeze_authority_revoked: rng.chance(2),
        min_liquidity_sol: if rng.chance(3) {
            0.0
        } else {
            rng.unit() * 50.0
        },
        max_creator_initial_buy_sol: if rng.chance(3) {
            0.0
        } else {
            rng.unit() * 10.0
        },
        min_pool_supply_fraction: if rng.chance(3) { 0.0 } else { rng.unit() },
        max_snapshot_age_ms: 1 + rng.below(6_000),
        strict_gates: rng.chance(2),
        ..SniperConfig::default()
    }
}

// ---------------------------------------------------------------- events --

#[test]
fn event_id_ignores_everything_but_identity_and_reference() {
    let mut rng = Rng::new(0x5EED_0001);
    for _ in 0..ITERATIONS {
        let base = random_event(&mut rng);
        assert_eq!(base.event_id, base.compute_event_id());
        assert_eq!(
            base.validate_shape(t0()),
            Ok(()),
            "well-formed event: {base:?}"
        );

        // Presentation-only fields do not move the id.
        let mut other = base.clone();
        other.source = rng.feed();
        other.observed_at = t0() + Duration::milliseconds(rng.below(60_000) as i64);
        other.source_ts = Some(t0());
        other.event_ts = Some(t0() - Duration::seconds(rng.below(20) as i64));
        other.raw_hash = format!("other-{:x}", rng.next_u64());
        other.launch.name = "renamed".into();
        other.launch.symbol = "RN".into();
        other.launch.market_cap_sol += 1.0;
        other.creator = rng.pubkey().to_string();
        other.liquidity_quote_lamports = Some(rng.below(SOL));
        other.initial_price_sol = Some(rng.unit());
        // With a signature present the source sequence is irrelevant too.
        other.source_seq = base.source_seq + 1 + rng.below(1_000);
        assert_eq!(
            other.compute_event_id(),
            base.event_id,
            "id drifted on a non-identity field"
        );
        assert!(base.consistent_with(&other) && other.consistent_with(&base));

        // Identity fields do.
        let mut m = base.clone();
        m.mint = rng.pubkey().to_string();
        m.base_mint = m.mint.clone();
        assert_ne!(
            m.compute_event_id(),
            base.event_id,
            "mint must be part of the id"
        );

        let mut s = base.clone();
        s.signature = Some(rng.signature());
        assert_ne!(
            s.compute_event_id(),
            base.event_id,
            "signature must be part of the id"
        );

        let mut p = base.clone();
        p.protocol = match base.protocol {
            LaunchProtocol::PumpFun => LaunchProtocol::PumpSwap,
            LaunchProtocol::PumpSwap => LaunchProtocol::RaydiumAmmV4,
            LaunchProtocol::RaydiumAmmV4 => LaunchProtocol::PumpFun,
        };
        assert_ne!(
            p.compute_event_id(),
            base.event_id,
            "protocol must be part of the id"
        );

        let mut pool = base.clone();
        pool.pool = Some(rng.pubkey().to_string());
        assert_ne!(
            pool.compute_event_id(),
            base.event_id,
            "pool must be part of the id"
        );

        // A tampered id is a malformed event, not a new identity.
        let mut tampered = base.clone();
        tampered.event_id = format!("evt_{:032x}", rng.next_u64());
        assert_eq!(
            tampered.validate_shape(t0()),
            Err(EventDefect::EventIdMismatch)
        );
    }
}

#[test]
fn event_reference_kind_is_part_of_the_identity() {
    let mut rng = Rng::new(0x5EED_0002);
    for _ in 0..ITERATIONS {
        let mut with_sig = random_event(&mut rng);
        with_sig.slot = Some(1 + rng.below(1_000_000));
        with_sig.event_id = with_sig.compute_event_id();

        let mut slot_only = with_sig.clone();
        slot_only.signature = None;
        slot_only.event_id = slot_only.compute_event_id();

        let mut seq_only = slot_only.clone();
        seq_only.slot = None;
        seq_only.event_id = seq_only.compute_event_id();

        assert_ne!(with_sig.event_id, slot_only.event_id);
        assert_ne!(slot_only.event_id, seq_only.event_id);
        assert_ne!(with_sig.event_id, seq_only.event_id);

        // Same slot → same slot-only id, regardless of the sequence number.
        let mut same_slot = slot_only.clone();
        same_slot.source_seq += 1 + rng.below(100);
        assert_eq!(same_slot.compute_event_id(), slot_only.event_id);

        // Sequence-only ids are per source AND per sequence: bumping either
        // yields a different event (nothing else identifies it).
        let mut next_seq = seq_only.clone();
        next_seq.source_seq += 1;
        assert_ne!(next_seq.compute_event_id(), seq_only.event_id);

        // Every variant still validates on its own.
        for ev in [&with_sig, &slot_only, &seq_only, &same_slot] {
            assert_eq!(ev.validate_shape(t0()), Ok(()));
        }
    }
}

#[test]
fn dedup_key_is_the_mint_for_pump_fun_and_namespaced_for_amm_launches() {
    let mut rng = Rng::new(0x5EED_0003);
    for _ in 0..ITERATIONS {
        let ev = random_event(&mut rng);
        let key = ev.dedup_key();
        match ev.protocol {
            LaunchProtocol::PumpFun => assert_eq!(key, ev.mint),
            other => assert_eq!(key, format!("{}:{}", other.as_str(), ev.mint)),
        }
        // The same mint on another protocol is a different decision.
        let mut on_other = ev.clone();
        on_other.protocol = match ev.protocol {
            LaunchProtocol::PumpFun => LaunchProtocol::PumpSwap,
            _ => LaunchProtocol::PumpFun,
        };
        assert_ne!(on_other.dedup_key(), key);
        // …while the feed, the signature and the timing never change it.
        let mut same = ev.clone();
        same.source = rng.feed();
        same.signature = Some(rng.signature());
        same.observed_at = t0() + Duration::seconds(1);
        assert_eq!(same.dedup_key(), key);
    }
}

#[test]
fn staleness_is_a_pure_threshold_on_the_effective_timestamp() {
    let mut rng = Rng::new(0x5EED_0004);
    for _ in 0..ITERATIONS {
        let mut ev = random_event(&mut rng);
        let age_ms = rng.below(120_000) as i64;
        ev.event_ts = Some(t0() - Duration::milliseconds(age_ms));
        ev.event_id = ev.compute_event_id();
        let max_secs = rng.below(90) as i64;
        let stale = ev.is_stale(t0(), max_secs);
        if max_secs <= 0 {
            assert!(!stale, "a non-positive threshold disables the check");
        } else {
            assert_eq!(
                stale,
                age_ms > max_secs * 1_000,
                "age {age_ms} ms vs {max_secs} s"
            );
        }
        assert_eq!(ev.age_ms(t0()), age_ms as u64);
        // Without a chain timestamp the observation time is the reference.
        ev.event_ts = None;
        ev.source_ts = None;
        assert_eq!(ev.age_ms(t0()), 0);
        assert_eq!(ev.age_ms(t0() - Duration::seconds(5)), 0, "never negative");
    }
}

#[test]
fn sequence_tracker_counts_exactly_the_slot_regressions() {
    let mut rng = Rng::new(0x5EED_0005);
    for _ in 0..ITERATIONS {
        let mut tracker = SequenceTracker::default();
        let mut highest = 0u64;
        let mut regressions = 0u64;
        let n = 1 + rng.below(40);
        for i in 0..n {
            assert_eq!(
                tracker.next_seq(),
                i + 1,
                "sequence numbers are 1-based and dense"
            );
            let slot = if rng.chance(5) {
                None
            } else if rng.chance(4) {
                Some(0)
            } else {
                Some(1 + rng.below(1_000))
            };
            let regressed = tracker.note_slot(slot);
            match slot {
                Some(s) if s > 0 && s < highest => {
                    regressions += 1;
                    assert!(regressed);
                }
                Some(s) if s > 0 => {
                    highest = s;
                    assert!(!regressed);
                }
                _ => assert!(!regressed, "missing / zero slots are never regressions"),
            }
        }
        assert_eq!(tracker.out_of_order(), regressions);
        assert_eq!(tracker.issued(), n);
        if highest > 0 {
            assert_eq!(tracker.highest_slot(), Some(highest));
        } else {
            assert_eq!(tracker.highest_slot(), None);
        }
    }
}

// ---------------------------------------------------------- stage machine --

fn ordinal(stage: SniperStage) -> usize {
    SniperStage::ALL.iter().position(|s| *s == stage).unwrap()
}

#[test]
fn stage_machine_has_no_escape_from_terminal_and_no_shortcut_forward() {
    let mut rng = Rng::new(0x5EED_0006);
    let forward = [
        SniperStage::Detected,
        SniperStage::Validated,
        SniperStage::RiskApproved,
        SniperStage::ExecutionReady,
        SniperStage::Submitted,
        SniperStage::Confirmed,
    ];
    for _ in 0..ITERATIONS {
        let from = rng.stage();
        let to = rng.stage();
        let legal = from.can_transition_to(to);
        if from.is_terminal() {
            assert!(!legal, "{from} is terminal and moved to {to}");
            continue;
        }
        match to {
            SniperStage::Rejected => {
                assert_eq!(legal, ordinal(from) <= ordinal(SniperStage::ExecutionReady));
            }
            SniperStage::Failed => {
                assert_eq!(
                    legal,
                    matches!(from, SniperStage::ExecutionReady | SniperStage::Submitted)
                );
            }
            _ => {
                let fi = forward.iter().position(|s| *s == from).unwrap();
                let ti = forward.iter().position(|s| *s == to).unwrap();
                assert_eq!(legal, ti == fi + 1, "{from} -> {to}");
            }
        }
    }
}

#[test]
fn lifecycle_random_walks_stay_consistent() {
    let mut rng = Rng::new(0x5EED_0007);
    for _ in 0..ITERATIONS {
        let mut lc = Lifecycle::start(
            format!("evt_{:x}", rng.next_u64()),
            rng.protocol(),
            t0() - Duration::milliseconds(50),
            t0(),
            t0(),
        );
        let mut now = t0();
        let mut steps = 1usize;
        while !lc.stage.is_terminal() && steps < 32 {
            now += Duration::milliseconds(1 + rng.below(20) as i64);
            let before = lc.clone();
            let next = rng.stage();
            match lc.advance(next, now) {
                Ok(()) => {
                    assert!(before.stage.can_transition_to(next));
                    assert_eq!(lc.stage, next);
                    assert_eq!(lc.history.len(), before.history.len() + 1);
                    steps += 1;
                }
                Err(rejection) => {
                    assert_eq!(rejection.reason, RejectReason::InvalidState);
                    assert_eq!(rejection.stage, before.stage);
                    assert_eq!(
                        lc, before,
                        "an illegal transition must not mutate the lifecycle"
                    );
                }
            }
            if rng.chance(6) {
                let stage_before = lc.stage;
                let r = lc.reject(RejectReason::RiskRejected, "random walk", now);
                assert_eq!(r.stage, stage_before);
                assert_eq!(r.reason, RejectReason::RiskRejected);
                if stage_before.can_transition_to(SniperStage::Rejected) {
                    assert_eq!(lc.stage, SniperStage::Rejected);
                } else if stage_before.can_transition_to(SniperStage::Failed) {
                    assert_eq!(lc.stage, SniperStage::Failed);
                } else {
                    assert_eq!(
                        lc.stage, stage_before,
                        "terminal stages are never rewritten"
                    );
                }
                assert_eq!(lc.rejection.as_ref(), Some(&r));
            }
        }
        // The recorded path is the history, in order, and timestamps never
        // go backwards.
        let path = lc.path();
        assert_eq!(path.split('>').count(), lc.history.len());
        assert!(path.starts_with("DETECTED"));
        assert!(lc.history.windows(2).all(|w| w[0].1 <= w[1].1));
        // The timeline marks are filled exactly for the stages visited.
        let visited = |s: SniperStage| lc.history.iter().any(|(h, _)| *h == s);
        assert_eq!(
            lc.timeline.validated_at.is_some(),
            visited(SniperStage::Validated)
        );
        assert_eq!(
            lc.timeline.risk_at.is_some(),
            visited(SniperStage::RiskApproved)
        );
        assert_eq!(
            lc.timeline.built_at.is_some(),
            visited(SniperStage::ExecutionReady)
        );
        assert_eq!(
            lc.timeline.submitted_at.is_some(),
            visited(SniperStage::Submitted)
        );
        assert_eq!(
            lc.timeline.confirmed_at.is_some(),
            visited(SniperStage::Confirmed)
        );
        if let Some(total) = lc.timeline.total_ms() {
            assert!(total <= (now - t0()).num_milliseconds() as u64);
        }
    }
}

// ------------------------------------------------------------------ gates --

#[test]
fn gate_reports_are_complete_ordered_and_strict_only_adds_failures() {
    let mut rng = Rng::new(0x5EED_0008);
    for _ in 0..ITERATIONS {
        let snap = snapshot(&mut rng);
        let cfg = gate_config(&mut rng);
        let report = gates::evaluate(&snap, &cfg, t0());

        let names: Vec<&str> = report.results.iter().map(|(g, _)| g.as_str()).collect();
        assert_eq!(names, ALL_GATES.to_vec(), "every gate, once, in order");

        let lenient = report.first_failure(false);
        let strict = report.first_failure(true);
        if let Some((gate, _)) = &lenient {
            let (sgate, _) = strict
                .as_ref()
                .expect("strict mode cannot pass when lenient fails");
            let li = ALL_GATES.iter().position(|g| g == gate).unwrap();
            let si = ALL_GATES.iter().position(|g| g == sgate).unwrap();
            assert!(
                si <= li,
                "strict mode fails at the same gate or an earlier skip"
            );
            assert!(
                gates::reason_for_gate(gate) != RejectReason::InvalidState,
                "every gate maps to a specific reason"
            );
        }
        if strict.is_none() {
            assert!(lenient.is_none());
            assert!(
                report.skipped().is_empty(),
                "no skips can remain under a strict pass"
            );
        }
        assert_eq!(report.passed(true), strict.is_none());
        assert_eq!(report.passed(false), lenient.is_none());

        // The summary lists every gate with its outcome.
        let summary = report.summary();
        for (gate, outcome) in &report.results {
            assert!(summary.contains(&format!("{gate}={}", outcome.as_str())));
        }
        // Skips only ever come from gates that can lack their datum.
        for skipped in report.skipped() {
            assert!(
                matches!(
                    skipped,
                    gates::GATE_POOL_OPEN_TIME
                        | gates::GATE_MINT_AUTHORITY
                        | gates::GATE_FREEZE_AUTHORITY
                        | gates::GATE_CREATOR_CONCENTRATION
                        | gates::GATE_POOL_SUPPLY_FRACTION
                ),
                "{skipped} can never be skipped"
            );
        }
    }
}

#[test]
fn gates_pass_when_every_datum_is_healthy_and_every_threshold_is_off() {
    let mut rng = Rng::new(0x5EED_0009);
    for _ in 0..ITERATIONS {
        let mut snap = snapshot(&mut rng);
        snap.tradable = true;
        snap.pool_open_time = snap.pool_open_time.map(|_| (t0().timestamp() - 1) as u64);
        if snap.protocol == LaunchProtocol::RaydiumAmmV4 {
            snap.pool_open_time = Some((t0().timestamp() - 1) as u64);
        }
        snap.mint_authority_revoked = Some(true);
        snap.freeze_authority_revoked = Some(true);
        snap.quote_reserve_lamports = 1 + rng.below(100 * SOL);
        snap.pricing_quote_reserve_lamports = snap.quote_reserve_lamports + 30 * SOL;
        snap.base_decimals = rng.below(13) as u8;
        snap.spot_price_sol = 1e-9 + rng.unit();
        snap.creator_initial_buy_sol = Some(rng.unit());
        snap.total_supply_raw = Some(1_000_000_000_000_000);
        snap.fetched_at = t0();

        let mut cfg = gate_config(&mut rng);
        cfg.min_liquidity_sol = 0.0;
        cfg.max_creator_initial_buy_sol = 0.0;
        cfg.min_pool_supply_fraction = 0.0;
        cfg.max_snapshot_age_ms = 1_000;

        let report = gates::evaluate(&snap, &cfg, t0());
        assert!(report.passed(true), "{}", report.summary());
        assert!(report.results.iter().all(|(_, o)| *o == GateOutcome::Pass));
    }
}

#[test]
fn gate_thresholds_are_monotone() {
    let mut rng = Rng::new(0x5EED_000A);
    for _ in 0..ITERATIONS {
        let snap = snapshot(&mut rng);
        let mut low = gate_config(&mut rng);
        low.strict_gates = false;
        let mut high = low.clone();
        // Tighten every threshold: nothing that failed may start passing.
        high.min_liquidity_sol = low.min_liquidity_sol + rng.unit() * 20.0;
        high.min_pool_supply_fraction = (low.min_pool_supply_fraction + rng.unit()).min(1.0);
        high.max_creator_initial_buy_sol = if low.max_creator_initial_buy_sol > 0.0 {
            (low.max_creator_initial_buy_sol * rng.unit()).max(1e-9)
        } else {
            0.0
        };
        high.max_snapshot_age_ms = 1 + rng.below(low.max_snapshot_age_ms);
        high.require_mint_authority_revoked = true;
        high.require_freeze_authority_revoked = true;

        let loose = gates::evaluate(&snap, &low, t0());
        let tight = gates::evaluate(&snap, &high, t0());
        for ((gate, loose_out), (_, tight_out)) in loose.results.iter().zip(tight.results.iter()) {
            if matches!(loose_out, GateOutcome::Fail(_)) {
                assert!(
                    matches!(tight_out, GateOutcome::Fail(_)),
                    "{gate}: tightening turned a failure into {tight_out:?}"
                );
            }
        }
        if !loose.passed(false) {
            assert!(!tight.passed(false));
        }
    }
}

// --------------------------------------------------------------- slippage --

fn slippage_inputs(rng: &mut Rng, mode: SlippageMode) -> SlippageInputs {
    let reserve = match rng.below(6) {
        0 => 0,
        1 => 1 + rng.below(1_000),
        2 => u64::MAX - rng.below(1_000),
        _ => rng.below(500 * SOL),
    };
    let trade = match rng.below(6) {
        0 => 0,
        1 => 1 + rng.below(1_000),
        2 => u64::MAX - rng.below(1_000),
        _ => rng.below(5 * SOL),
    };
    SlippageInputs {
        mode,
        strategy_bps: rng.below(12_000),
        protocol_bps: if rng.chance(2) {
            None
        } else {
            Some(rng.below(12_000))
        },
        token_bps: if rng.chance(3) {
            Some(rng.below(12_000))
        } else {
            None
        },
        hard_max_bps: rng.below(12_000),
        quote_reserve_lamports: reserve,
        trade_lamports: trade,
    }
}

#[test]
fn adaptive_slippage_never_exceeds_the_hard_maximum_and_fixed_passes_the_base_through() {
    let mut rng = Rng::new(0x5EED_000B);
    for _ in 0..ITERATIONS * 4 {
        let mode = match rng.below(3) {
            0 => SlippageMode::Fixed,
            1 => SlippageMode::LiquidityAware,
            _ => SlippageMode::PriceImpact,
        };
        let inputs = slippage_inputs(&mut rng, mode);
        let hard_max = inputs.hard_max_bps.min(10_000);
        let base = inputs
            .token_bps
            .or(inputs.protocol_bps)
            .unwrap_or(inputs.strategy_bps)
            .min(10_000);
        let impact =
            slippage::price_impact_bps(inputs.trade_lamports, inputs.quote_reserve_lamports);
        assert!(impact <= 10_000);

        match slippage::decide(&inputs) {
            Ok(d) => {
                assert_eq!(d.mode, mode);
                assert_eq!(d.price_impact_bps, impact);
                match mode {
                    SlippageMode::Fixed => {
                        assert_eq!(d.bps, base, "fixed mode is the base, unclamped");
                        assert!(!d.clamped);
                    }
                    SlippageMode::LiquidityAware | SlippageMode::PriceImpact => {
                        assert!(d.bps <= hard_max, "{d:?} exceeds hard max {hard_max}");
                        assert!(d.bps >= impact, "tolerance must cover the modelled impact");
                        assert!(inputs.trade_lamports > 0 && inputs.quote_reserve_lamports > 0);
                        if d.clamped {
                            assert_eq!(d.bps, hard_max);
                        } else {
                            let wanted = match mode {
                                SlippageMode::LiquidityAware => base.max(impact.saturating_mul(2)),
                                _ => impact.saturating_add(base),
                            };
                            assert_eq!(d.bps, wanted);
                        }
                    }
                }
            }
            Err(e) => {
                assert_ne!(mode, SlippageMode::Fixed, "fixed mode never refuses");
                match e {
                    SlippageError::ZeroTrade => assert_eq!(inputs.trade_lamports, 0),
                    SlippageError::ZeroLiquidity => {
                        assert_eq!(inputs.quote_reserve_lamports, 0);
                        assert!(inputs.trade_lamports > 0);
                    }
                    SlippageError::ImpactExceedsHardMax {
                        price_impact_bps,
                        hard_max_bps,
                    } => {
                        assert_eq!(price_impact_bps, impact);
                        assert_eq!(hard_max_bps, hard_max);
                        assert!(impact > hard_max);
                    }
                }
            }
        }
    }
}

#[test]
fn price_impact_model_is_bounded_and_monotone() {
    let mut rng = Rng::new(0x5EED_000C);
    for _ in 0..ITERATIONS * 4 {
        let reserve = rng.below(u64::MAX);
        let trade = rng.below(u64::MAX);
        let impact = slippage::price_impact_bps(trade, reserve);
        assert!(impact <= 10_000);
        if trade == 0 {
            assert_eq!(impact, 0);
        }
        if reserve == 0 && trade > 0 {
            assert_eq!(impact, 10_000);
        }
        // More trade → at least as much impact; more reserve → at most as much.
        let bigger_trade = trade.saturating_add(1 + rng.below(SOL));
        assert!(slippage::price_impact_bps(bigger_trade, reserve) >= impact);
        let deeper = reserve.saturating_add(1 + rng.below(SOL));
        assert!(slippage::price_impact_bps(trade, deeper) <= impact);
    }
    // Percent ↔ bps round trip on the grid the config uses.
    for _ in 0..ITERATIONS {
        let bps = rng.below(10_001);
        assert_eq!(slippage::pct_to_bps(slippage::bps_to_pct(bps)), bps);
    }
}
