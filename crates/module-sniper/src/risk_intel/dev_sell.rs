//! Dev-sell producer (PART 03 K2): the real signal behind
//! `sniper.advanced_exit.dev_sell_ttl_secs`.
//!
//! [`crate::exit_policy`] already flattens a position when the creator/dev
//! wallet sold inside the configured window, and [`RiskIntel::publish_dev_sell`]
//! already carries the observation to every subscribed sweeper — but nothing in
//! production ever called it, so the feature was inert. This module is that
//! producer and the ONLY sender on the dev-sell bus: `Sniper::run` attaches one
//! bus, the sweeper clones subscribe to it, and only this watcher publishes.
//!
//! # What it watches
//! One `logsSubscribe` with a `mentions` filter per creator wallet of an OPEN
//! sniper position. The creator address comes from
//! [`RiskIntelService::creator_for_mint`] — the creator the launch actually
//! reported for the position that was booked — so a position whose creator was
//! never observed is not watched at all (unknown is not guessed). The
//! subscription set is reconciled against the open positions every
//! [`RECONCILE_INTERVAL`], so closing a position releases its subscription.
//!
//! # Never published from log text alone
//! A notification only makes a transaction a *candidate*. Before anything is
//! published the transaction is fetched and verified on chain:
//!
//! 1. the transaction succeeded (`meta.err` absent);
//! 2. the creator is a **required signer** of that message;
//! 3. the message carries a sell instruction — decoded with the same
//!    [`crate::risk_intel::first_slot::classify_swap`] the first-slot producer
//!    uses, so the workspace has ONE owner of venue instruction classification
//!    and no second decoder — whose `user` account IS the creator;
//! 4. the mint that sell moved is one of the positions currently watched for
//!    that creator.
//!
//! Any failed step means no observation. An unreadable transaction is
//! *unknown*, never "verified with a guess".
//!
//! # Off means off
//! `sniper.advanced_exit` absent, or `dev_sell_ttl_secs == 0` → the watcher is
//! never started: no websocket, no subscription, no RPC.
//!
//! # Reconnects
//! Owned by `solana-kit`'s websocket supervisor: backoff, stale-connection
//! detection and resubscribe all follow [`WsPolicy::from_network`]. This module
//! adds no retry logic of its own and only counts what it could not verify.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::Utc;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use bot_core::config::Config;
use bot_core::models::BotModule;
use bot_core::state::Shared;
use solana_kit::consts::{PUMPSWAP_PROGRAM_ID, PUMP_PROGRAM_ID};
use solana_kit::decode::decode_instructions;
use solana_kit::rpc::Rpc;
use solana_kit::ws::{LogsFilter, SolanaWs, SubscriptionHandle, WsHandle, WsMessage, WsPolicy};

use crate::exit_policy::DevSellBus;
use crate::risk_intel::service::RiskIntelService;
use crate::risk_intel::{DevSellObservation, RiskIntel};

/// How often the watcher reconciles its subscriptions with the set of open
/// sniper positions. Cheap and offline: an `open_positions_for` read plus a
/// map comparison.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(2);

/// Upper bound on simultaneous creator subscriptions. They all share one
/// websocket, but each costs a server-side subscription; a deployment holding
/// more positions than this is served most-positions-first (ties broken by
/// address, so the same state always subscribes to the same set) and the
/// remainder is reported, never silently dropped.
const MAX_WATCHED_CREATORS: usize = 8;

/// Bounded candidate queue between the subscription forwarders and the
/// verification loop. Overflow is a gap: counted and dropped — back-pressure
/// here would put a risk-signal reader in the middle of the launch feeds.
const CANDIDATE_BUFFER: usize = 256;

/// `true` when the operator enabled the dev-sell trigger.
///
/// The single reading point of that switch: the same
/// `sniper.advanced_exit.dev_sell_ttl_secs` the exit policy engine consumes.
pub fn enabled(cfg: &Config) -> bool {
    ttl_secs(cfg) > 0
}

/// The configured dev-sell window in seconds (`0` = off, including when
/// `sniper.advanced_exit` is absent entirely).
pub fn ttl_secs(cfg: &Config) -> u64 {
    cfg.sniper
        .advanced_exit
        .as_ref()
        .map(|advanced| advanced.dev_sell_ttl_secs)
        .unwrap_or(0)
}

/// A transaction that mentioned a watched creator and could be a sell, before
/// on-chain verification.
#[derive(Debug, Clone)]
struct Candidate {
    creator: Pubkey,
    signature: String,
}

/// One live subscription plus the task that feeds candidates from it.
///
/// It deliberately carries no mint set: which positions a creator is watched
/// for is re-read at verification time from the owners of that state (shared
/// state + [`RiskIntelService`]), so a subscription can never act on a stale
/// snapshot of the position book.
struct Watch {
    /// Held so the subscription can be CANCELLED when the last position for
    /// this creator closes: `SubscriptionHandle::cancel` awaits the registry
    /// removal and the server-side `logsUnsubscribe` (its `Drop` only signals
    /// the supervisor, which is a backstop, not the path this module relies
    /// on).
    subscription: SubscriptionHandle,
    forwarder: tokio::task::JoinHandle<()>,
}

impl Watch {
    /// Stop feeding the verification queue, then unsubscribe.
    async fn release(self) {
        self.forwarder.abort();
        self.subscription.cancel().await;
    }
}

/// Start the dev-sell watcher.
///
/// `None` — no socket opened, no subscription sent, no RPC call — when
/// `sniper.advanced_exit.dev_sell_ttl_secs` is 0 or its block is absent.
/// `Some(handle)` is the task the sniper owns and joins on shutdown (see
/// `Sniper::run`); the endpoint it dials is the provider pool's own.
pub async fn spawn(
    state: Shared,
    rpc: Rpc,
    service: RiskIntelService,
    bus: DevSellBus,
) -> Option<tokio::task::JoinHandle<()>> {
    let cfg = state.config_snapshot().await;
    if !enabled(&cfg) {
        debug!(
            "dev-sell watcher not started (sniper.advanced_exit.dev_sell_ttl_secs = 0): \
             no websocket, no subscription, no RPC"
        );
        return None;
    }
    // The websocket endpoint is the pool's own (it derives one from the RPC URL
    // when `network.ws_url` is empty — `solana_kit::rpc::http_to_ws`), so this
    // module never guesses an endpoint of its own.
    let ws_url = rpc.ws_url().trim().to_string();
    let policy = WsPolicy::from_network(&cfg.network);
    Some(tokio::spawn(run(
        state,
        rpc,
        service,
        RiskIntel::with_bus(bus),
        ws_url,
        policy,
    )))
}

/// The watcher loop. Runs until the process shuts down.
async fn run(
    state: Shared,
    rpc: Rpc,
    service: RiskIntelService,
    intel: RiskIntel,
    ws_url: String,
    policy: WsPolicy,
) {
    info!("sniper dev-sell watcher started");
    let ws = SolanaWs::with_policy(ws_url, policy);
    // Owns the supervisor task: dropping `connection` tears the socket down,
    // which together with dropping every subscription handle is the entire
    // teardown this loop needs.
    let connection: WsHandle = ws.spawn();
    let (tx, mut rx) = mpsc::channel::<Candidate>(CANDIDATE_BUFFER);
    let mut watches: HashMap<Pubkey, Watch> = HashMap::new();
    // One observation per (creator, mint): a replayed notification after a
    // reconnect must never re-trigger a flatten, and an entry is retired as
    // soon as its position is gone.
    let mut published: HashSet<(Pubkey, Pubkey)> = HashSet::new();
    let mut ticker = tokio::time::interval(RECONCILE_INTERVAL);

    loop {
        tokio::select! {
            _ = state.wait_shutdown() => break,
            _ = ticker.tick() => {
                reconcile(&state, &ws, &service, &mut watches, &tx).await;
                let open: HashSet<Pubkey> = open_mints(&state).await.into_iter().collect();
                published.retain(|(_creator, mint)| open.contains(mint));
            }
            candidate = rx.recv() => {
                let Some(candidate) = candidate else { break };
                verify_and_publish(&state, &rpc, &service, &intel, candidate, &mut published).await;
            }
        }
    }

    // Close the queue first — a forwarder ends when the channel it feeds is
    // closed — then unsubscribe from every creator, then close the shared
    // socket. Same order the launch feeds use for their own forwarders.
    drop(tx);
    for (_creator, watch) in watches.drain() {
        watch.release().await;
    }
    connection.shutdown().await;
    info!("sniper dev-sell watcher stopped");
}

/// The open sniper positions that have an OBSERVED creator, grouped by
/// creator.
///
/// A position whose launch reported no creator is skipped: there is no wallet
/// to watch, and substituting one (the launch fee payer, say) would attribute a
/// stranger's sell to this launch.
async fn watch_targets(
    state: &Shared,
    service: &RiskIntelService,
) -> HashMap<Pubkey, HashSet<Pubkey>> {
    let mut by_creator: HashMap<Pubkey, HashSet<Pubkey>> = HashMap::new();
    for position in state.open_positions_for(BotModule::Sniper).await {
        let Ok(mint) = Pubkey::try_from(position.symbol.as_str()) else {
            continue;
        };
        let Some(creator) = service.creator_for_mint(&mint) else {
            debug!(
                mint = %position.symbol,
                "dev-sell watch skipped: the launch reported no creator for this position"
            );
            continue;
        };
        by_creator.entry(creator).or_default().insert(mint);
    }
    by_creator
}

/// The mints of every open sniper position (used to expire observations).
async fn open_mints(state: &Shared) -> Vec<Pubkey> {
    state
        .open_positions_for(BotModule::Sniper)
        .await
        .iter()
        .filter_map(|position| Pubkey::try_from(position.symbol.as_str()).ok())
        .collect()
}

/// Subscribe to what the open positions need, release what they do not.
async fn reconcile(
    state: &Shared,
    ws: &SolanaWs,
    service: &RiskIntelService,
    watches: &mut HashMap<Pubkey, Watch>,
    tx: &mpsc::Sender<Candidate>,
) {
    let mut wanted = watch_targets(state, service).await;
    // Release every creator whose last position closed: the forwarder is
    // abandoned first and the subscription is then cancelled, so a sell that
    // arrives afterwards cannot be verified against a watch that no longer
    // exists (and no server-side subscription is left dangling).
    let stale: Vec<Pubkey> = watches
        .keys()
        .filter(|creator| !wanted.contains_key(creator))
        .copied()
        .collect();
    for creator in stale {
        if let Some(watch) = watches.remove(&creator) {
            info!(%creator, "dev-sell subscription released (no open position for this creator)");
            watch.release().await;
        }
    }
    if wanted.len() > MAX_WATCHED_CREATORS {
        let mut ranked: Vec<Pubkey> = wanted.keys().copied().collect();
        ranked.sort_by(|a, b| {
            let la = wanted.get(a).map(HashSet::len).unwrap_or(0);
            let lb = wanted.get(b).map(HashSet::len).unwrap_or(0);
            lb.cmp(&la).then_with(|| a.cmp(b))
        });
        let keep: HashSet<Pubkey> = ranked.into_iter().take(MAX_WATCHED_CREATORS).collect();
        let skipped = wanted.len() - keep.len();
        wanted.retain(|creator, _| keep.contains(creator));
        // A creator that was watched and is now over the ceiling loses its
        // subscription too; it is not kept alive in case it is watched again.
        let over: Vec<Pubkey> = watches
            .keys()
            .filter(|creator| !wanted.contains_key(creator))
            .copied()
            .collect();
        for creator in over {
            if let Some(watch) = watches.remove(&creator) {
                debug!(%creator, "dev-sell subscription released (over the watched-creator ceiling)");
                watch.release().await;
            }
        }
        warn!(
            watched = wanted.len(),
            skipped,
            "dev-sell watcher is at its subscription ceiling; these creators are NOT watched \
             and their sells cannot flatten a position"
        );
    }
    for creator in wanted.keys().copied() {
        if watches.contains_key(&creator) {
            continue;
        }
        match ws
            .logs_subscribe(LogsFilter::default().mentions([creator.to_string()]))
            .await
        {
            Ok(mut subscription) => match subscription.receiver() {
                Ok(receiver) => {
                    let forwarder = tokio::spawn(forward(creator, receiver, tx.clone()));
                    watches.insert(
                        creator,
                        Watch {
                            subscription,
                            forwarder,
                        },
                    );
                    info!(%creator, "dev-sell watcher subscribed to the creator wallet");
                }
                Err(error) => {
                    warn!(%creator, %error, "dev-sell subscription receiver unavailable; this creator is not watched");
                }
            },
            Err(error) => {
                warn!(%creator, %error, "dev-sell subscription failed; the exit policy stays without this creator's signal");
            }
        }
    }
}

/// Copy one subscription's log notifications into the candidate queue.
async fn forward(creator: Pubkey, mut rx: mpsc::Receiver<WsMessage>, tx: mpsc::Sender<Candidate>) {
    while let Some(message) = rx.recv().await {
        let WsMessage::Logs {
            signature,
            logs,
            err,
            ..
        } = message
        else {
            continue;
        };
        if err.is_some() {
            // A failed transaction sold nothing.
            continue;
        }
        if !may_touch_a_launch_venue(&logs) {
            continue;
        }
        match tx.try_send(Candidate { creator, signature }) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(dropped)) => {
                count("queue_full");
                debug!(
                    creator = %dropped.creator,
                    signature = %dropped.signature,
                    "dev-sell candidate queue is full; this notification is not verified"
                );
            }
            Err(mpsc::error::TrySendError::Closed(_)) => break,
        }
    }
}

/// Sound, cheap prefilter: a sell of a launch token must execute inside the
/// pump.fun or the PumpSwap program, and a node logs every program it invokes,
/// so a frame with neither has nothing to fetch. An absent log array cannot
/// rule anything out and is verified anyway.
fn may_touch_a_launch_venue(logs: &[String]) -> bool {
    if logs.is_empty() {
        return true;
    }
    let pump = PUMP_PROGRAM_ID.to_string();
    let pumpswap = PUMPSWAP_PROGRAM_ID.to_string();
    logs.iter().any(|line| {
        let Some(rest) = line.strip_prefix("Program ") else {
            return false;
        };
        let invoked = rest.split(' ').next().unwrap_or_default();
        invoked == pump || invoked == pumpswap
    })
}

/// Verify one candidate against the open positions and publish what it proves.
async fn verify_and_publish(
    state: &Shared,
    rpc: &Rpc,
    service: &RiskIntelService,
    intel: &RiskIntel,
    candidate: Candidate,
    published: &mut HashSet<(Pubkey, Pubkey)>,
) {
    let Candidate { creator, signature } = candidate;
    let Ok(parsed) = signature.parse::<Signature>() else {
        debug!(%signature, "dev-sell candidate carries an unparsable signature; not verified");
        count("unverified");
        return;
    };
    let watched = watch_targets(state, service).await;
    let Some(mints) = watched.get(&creator) else {
        // The position closed (or was never attributable) while the
        // notification was in flight: nothing to flatten, nothing to publish.
        count("stale");
        return;
    };
    if mints.is_empty() {
        count("stale");
        return;
    }
    for observation in verified_creator_sells(rpc, &creator, &parsed).await {
        if !mints.contains(&observation.mint) {
            // The creator sold a token we never bought off this launch. That is
            // their business, not a signal for this position.
            continue;
        }
        if !published.insert((creator, observation.mint)) {
            count("duplicate");
            continue;
        }
        if intel.publish_dev_sell(observation) {
            count("published");
            warn!(
                mint = %observation.mint,
                %creator,
                "DEV SELL verified on chain — the exit policy flattens the position within its window"
            );
        } else {
            count("dropped");
            warn!(
                mint = %observation.mint,
                %creator,
                "verified dev sell but no exit sweeper is subscribed to the dev-sell bus; \
                 the observation is dropped, never buffered"
            );
        }
    }
}

/// Every mint `creator` provably sold in `signature`, as observations.
///
/// Empty for anything that is not proven: transaction absent or unreadable,
/// transaction failed on chain, creator not a required signer, no sell
/// instruction attributed to the creator. `observed_at` is the feed timestamp —
/// the transaction's block time when the node reports one, else the moment this
/// process read it. A block time that never existed is never invented.
pub async fn verified_creator_sells(
    rpc: &Rpc,
    creator: &Pubkey,
    signature: &Signature,
) -> Vec<DevSellObservation> {
    let budget = rpc.timeout();
    let confirmed = match tokio::time::timeout(budget, rpc.get_transaction(signature)).await {
        Ok(Ok(Some(confirmed))) => confirmed,
        Ok(Ok(None)) => return Vec::new(),
        Ok(Err(error)) => {
            debug!(%error, "dev-sell verification could not read the transaction; no signal is published");
            return Vec::new();
        }
        Err(_) => {
            debug!(
                "dev-sell verification timed out reading the transaction; no signal is published"
            );
            return Vec::new();
        }
    };
    let Some(meta) = confirmed.transaction.meta.as_ref() else {
        return Vec::new();
    };
    if meta.err.is_some() {
        return Vec::new();
    }
    // The creator must be a required signer of the very transaction that sold.
    // `decode()` answers for the base64 encoding `Rpc::get_transaction` asks
    // for; any other encoding is unverified, not "verified by assumption".
    let Some(decoded) = confirmed.transaction.transaction.decode() else {
        debug!("dev-sell verification: the transaction encoding is not decodable; not verified");
        return Vec::new();
    };
    let signer_count = decoded.signatures.len();
    let keys = decoded.message.static_account_keys();
    if !keys.iter().take(signer_count).any(|key| key == creator) {
        debug!(%creator, "the creator is not a signer of this transaction; no dev-sell signal");
        return Vec::new();
    }
    let Ok(instructions) = decode_instructions(&confirmed.transaction.transaction, meta) else {
        return Vec::new();
    };
    let observed_at = confirmed
        .block_time
        .and_then(crate::detect::unix_to_utc)
        .unwrap_or_else(Utc::now);
    let mut out: Vec<DevSellObservation> = instructions
        .iter()
        .filter_map(super::first_slot::classify_swap)
        .filter(|swap| !swap.is_buy && swap.user == *creator)
        .map(|swap| DevSellObservation {
            mint: swap.mint,
            observed_at,
            seller: *creator,
        })
        .collect();
    // Deterministic order for a multi-sell transaction, so the same fixture
    // always publishes in the same order.
    out.sort_by_key(|observation| observation.mint);
    out.dedup_by_key(|observation| observation.mint);
    out
}

/// Telemetry only: the outcome of one candidate, so an operator can see that
/// the feature is fed without turning on debug logs.
fn count(outcome: &'static str) {
    bot_core::obs::metrics::global()
        .counter(
            "sniper_dev_sell_signals_total",
            "Dev-sell candidates by outcome (published/unverified/stale/duplicate/dropped/queue_full).",
            &[("outcome", outcome)],
        )
        .inc();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config whose only interesting field is the dev-sell window.
    fn app_with_ttl(ttl: u64) -> bot_core::config::AppConfig {
        let mut app = bot_core::config::AppConfig::from_defaults();
        app.raw.sniper.advanced_exit = Some(bot_core::config::AdvancedExitConfig {
            ladder: Vec::new(),
            break_even: None,
            dev_sell_ttl_secs: ttl,
        });
        app
    }

    #[test]
    fn the_watcher_shares_the_exit_policys_own_switch() {
        // The producer must read the SAME switch the engine consumes, or the
        // operator turns on a trigger that nothing feeds (or a watcher with no
        // trigger). `None` (no advanced_exit block) is off.
        let absent = bot_core::config::AppConfig::from_defaults();
        assert_eq!(0, ttl_secs(&absent.raw));
        assert!(!enabled(&absent.raw));
        assert!(!enabled(&app_with_ttl(0).raw), "an explicit 0 is off");
        assert!(enabled(&app_with_ttl(90).raw));
        assert_eq!(90, ttl_secs(&app_with_ttl(90).raw));
    }

    #[test]
    fn the_prefilter_only_lets_launch_venue_frames_through() {
        let pump = format!("Program {} invoke [1]", *PUMP_PROGRAM_ID);
        let pumpswap_success = format!("Program {} success", *PUMPSWAP_PROGRAM_ID);
        assert!(may_touch_a_launch_venue(&[
            pump.clone(),
            pumpswap_success.clone()
        ]));
        assert!(
            may_touch_a_launch_venue(&[]),
            "a notification with no log array cannot rule a sell out"
        );
        assert!(!may_touch_a_launch_venue(&[
            "Program 11111111111111111111111111111111 invoke [1]".to_string()
        ]));
        assert!(!may_touch_a_launch_venue(&[
            "Program log: memo: nothing to see".to_string()
        ]));
        // A foreign program named in a `Program <id> invoke` frame is not a
        // launch venue, even though the line has the right shape.
        assert!(!may_touch_a_launch_venue(&[format!(
            "Program {} invoke [1]",
            *solana_kit::consts::RAYDIUM_AMM_V4
        )]));
    }

    #[test]
    fn a_candidate_carries_the_creator_and_the_signature_only() {
        // The queue element must stay small and free of chain-derived claims:
        // nothing here says "this was a sell".
        let creator = Pubkey::new_unique();
        let candidate = Candidate {
            creator,
            signature: "x".repeat(88),
        };
        assert_eq!(creator, candidate.creator);
        assert_eq!(88, candidate.signature.len());
    }

    #[tokio::test]
    async fn a_disabled_trigger_never_opens_a_socket() {
        // `spawn` is the only entry point. With the switch off it must return
        // None before touching the provider — the endpoint below is closed, so
        // any connection attempt would show up as a task, not as None.
        let state = bot_core::state::AppState::new(bot_core::config::AppConfig::from_defaults());
        let rpc = closed_rpc(&state);
        let bus: DevSellBus = tokio::sync::broadcast::channel(4).0;
        assert!(
            spawn(state, rpc, RiskIntelService::new(), bus)
                .await
                .is_none(),
            "ttl 0 must not start the watcher"
        );
    }

    #[tokio::test]
    async fn an_enabled_trigger_starts_on_the_pools_own_websocket() {
        // The switch on the other side: with a window configured the watcher
        // starts, and it dials the endpoint the provider pool reports — never
        // one this module derived itself (a wrong guess here means an
        // unreachable socket and a silently dead exit rule).
        let state = bot_core::state::AppState::new(app_with_ttl(60));
        let rpc = closed_rpc(&state);
        let expected = rpc.ws_url().to_string();
        assert!(
            expected.starts_with("ws"),
            "the pool must hand out a websocket endpoint: {expected}"
        );
        let bus: DevSellBus = tokio::sync::broadcast::channel(4).0;
        let task = spawn(state, rpc, RiskIntelService::new(), bus)
            .await
            .expect("an enabled trigger starts the watcher");
        // Nothing to watch yet (no open positions), so the task is idle; it
        // ends on its own once shutdown is requested.
        task.abort();
    }

    #[tokio::test]
    async fn an_unreadable_transaction_is_not_a_dev_sell() {
        // The negative that matters for the money path: with the HTTP endpoint
        // closed, verification returns NO observation — never a guess.
        let rpc = Rpc::with_urls(
            "http://127.0.0.1:1".to_string(),
            String::new(),
            Vec::new(),
            solana_sdk::commitment_config::CommitmentConfig::confirmed(),
            0,
            Duration::from_millis(50),
        )
        .expect("rpc builds");
        let creator = Pubkey::new_unique();
        let signature = Signature::from([7u8; 64]);
        assert!(
            verified_creator_sells(&rpc, &creator, &signature)
                .await
                .is_empty(),
            "an RPC error must yield no observation"
        );
    }

    /// An `Rpc` whose endpoints are all closed (nothing here may reach a
    /// network), built the way the config layer builds one.
    fn closed_rpc(state: &bot_core::state::Shared) -> Rpc {
        let _ = state;
        Rpc::with_urls(
            "http://127.0.0.1:1".to_string(),
            String::new(),
            Vec::new(),
            solana_sdk::commitment_config::CommitmentConfig::confirmed(),
            0,
            Duration::from_millis(50),
        )
        .expect("rpc builds")
    }
}
