//! Leader discovery, stats and leaderboard (GAP-MAP v2, P2).
//!
//! Pure analytics over a feed of resolved leader trades. Nothing here
//! talks to the network: the server's data ingestion produces
//! [`LeaderTradeRecord`] rows (Polymarket data-api activity), and this
//! module turns them into per-wallet stats and a ranked leaderboard.
//!
//! Integrity rules:
//! * wallets with fewer than `min_trades` closed trades are UNRANKED — a
//!   one-lucky-trade wallet must never top a leaderboard;
//! * win rate and ROI are basis-point integers (no float comparison in
//!   ranking: rank keys are `(pnl_micro_usd, roi_bps, volume_micro_usd)`);
//! * PnL is integer micro-USDC throughout — the venue reports USDC
//!   quantities with 6 decimals, and float accumulation would drift.

use std::collections::HashMap;

/// One RESOLVED trade attributed to a wallet. (Open positions contribute
/// nothing to stats until they resolve: unrealized numbers are opinions,
/// not facts.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaderTradeRecord {
    /// The leader's proxy wallet (0x address, lowercased by ingestion).
    pub wallet: String,
    /// Net USDC result of the closed position, in micro-USDC (signed).
    pub pnl_micro_usd: i64,
    /// USDC committed, in micro-USDC (positive).
    pub stake_micro_usd: i64,
    /// Trade timestamp (unix seconds) — used only for the recency stat.
    pub at_unix: i64,
}

/// Aggregated stats for one wallet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaderStats {
    pub wallet: String,
    /// Closed trades counted.
    pub trades: u32,
    /// Trades with strictly positive PnL.
    pub wins: u32,
    /// Trades with strictly negative PnL (breakevens are neither).
    pub losses: u32,
    /// Sum of signed PnL, micro-USDC.
    pub pnl_micro_usd: i64,
    /// Sum of stakes, micro-USDC.
    pub volume_micro_usd: i64,
    /// Win rate in basis points (wins / trades), 0 when no trades.
    pub win_rate_bps: u32,
    /// Return on committed capital in bps: pnl / volume, 0 when volume 0.
    pub roi_bps: i64,
    /// Most recent trade timestamp (unix seconds), 0 when none.
    pub last_trade_unix: i64,
}

impl LeaderStats {
    fn new(wallet: &str) -> Self {
        Self {
            wallet: wallet.to_string(),
            trades: 0,
            wins: 0,
            losses: 0,
            pnl_micro_usd: 0,
            volume_micro_usd: 0,
            win_rate_bps: 0,
            roi_bps: 0,
            last_trade_unix: 0,
        }
    }

    fn absorb(&mut self, record: &LeaderTradeRecord) {
        self.trades += 1;
        if record.pnl_micro_usd > 0 {
            self.wins += 1;
        } else if record.pnl_micro_usd < 0 {
            self.losses += 1;
        }
        self.pnl_micro_usd = self
            .pnl_micro_usd
            .saturating_add(record.pnl_micro_usd);
        self.volume_micro_usd = self
            .volume_micro_usd
            .saturating_add(record.stake_micro_usd.max(0));
        if record.at_unix > self.last_trade_unix {
            self.last_trade_unix = record.at_unix;
        }
    }

    fn finalize(&mut self) {
        self.win_rate_bps = if self.trades == 0 {
            0
        } else {
            ((self.wins as u64 * 10_000) / self.trades as u64) as u32
        };
        self.roi_bps = if self.volume_micro_usd <= 0 {
            0
        } else {
            (self.pnl_micro_usd as i128 * 10_000 / self.volume_micro_usd as i128) as i64
        };
    }
}

/// One ranked leaderboard entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaderboardEntry {
    pub stats: LeaderStats,
    /// 1-based rank in the returned ordering.
    pub rank: u32,
}

/// Build per-wallet stats from raw resolved trades. Deterministic output
/// order: wallets are returned sorted by wallet address so callers can
/// diff runs.
pub fn compute_stats(records: &[LeaderTradeRecord]) -> Vec<LeaderStats> {
    let mut by_wallet: HashMap<&str, LeaderStats> = HashMap::new();
    for record in records {
        by_wallet
            .entry(record.wallet.as_str())
            .or_insert_with(|| LeaderStats::new(&record.wallet))
            .absorb(record);
    }
    let mut out: Vec<LeaderStats> = by_wallet.into_values().collect();
    for stats in &mut out {
        stats.finalize();
    }
    out.sort_by(|a, b| a.wallet.cmp(&b.wallet));
    out
}

/// Rank wallets into a leaderboard.
///
/// Ranking key (all integer, no float comparison): PnL desc, then ROI
/// desc, then volume desc, then wallet asc as the final tiebreak so the
/// output is a total order. Wallets with fewer than `min_trades` closed
/// trades are EXCLUDED — returning unranked noise is worse than a short
/// board.
pub fn leaderboard(records: &[LeaderTradeRecord], min_trades: u32) -> Vec<LeaderboardEntry> {
    let mut eligible: Vec<LeaderStats> = compute_stats(records)
        .into_iter()
        .filter(|s| s.trades >= min_trades)
        .collect();
    eligible.sort_by(|a, b| {
        b.pnl_micro_usd
            .cmp(&a.pnl_micro_usd)
            .then(b.roi_bps.cmp(&a.roi_bps))
            .then(b.volume_micro_usd.cmp(&a.volume_micro_usd))
            .then(a.wallet.cmp(&b.wallet))
    });
    eligible
        .into_iter()
        .enumerate()
        .map(|(i, stats)| LeaderboardEntry {
            stats,
            rank: (i + 1) as u32,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(wallet: &str, pnl: i64, stake: i64, at: i64) -> LeaderTradeRecord {
        LeaderTradeRecord {
            wallet: wallet.into(),
            pnl_micro_usd: pnl,
            stake_micro_usd: stake,
            at_unix: at,
        }
    }

    #[test]
    fn stats_aggregate_wins_losses_and_rates() {
        let records = vec![
            rec("0xa", 500_000, 1_000_000, 100),   // win +0.5
            rec("0xa", -200_000, 1_000_000, 300),  // loss -0.2
            rec("0xa", 0, 1_000_000, 200),         // breakeven: neither
            rec("0xb", 100_000, 400_000, 50),      // one win
        ];
        let stats = compute_stats(&records);
        assert_eq!(stats.len(), 2);
        let a = stats.iter().find(|s| s.wallet == "0xa").unwrap();
        assert_eq!(a.trades, 3);
        assert_eq!(a.wins, 1);
        assert_eq!(a.losses, 1);
        assert_eq!(a.pnl_micro_usd, 300_000);
        assert_eq!(a.volume_micro_usd, 3_000_000);
        assert_eq!(a.win_rate_bps, 3_333, "1/3 in bps");
        assert_eq!(a.roi_bps, 1_000, "300k/3M = 10%");
        assert_eq!(a.last_trade_unix, 300);
        let b = stats.iter().find(|s| s.wallet == "0xb").unwrap();
        assert_eq!(b.roi_bps, 2_500, "100k/400k = 25%");
    }

    #[test]
    fn empty_input_gives_empty_stats_not_zerofilled_wallets() {
        assert!(compute_stats(&[]).is_empty());
        assert!(leaderboard(&[], 1).is_empty());
    }

    #[test]
    fn leaderboard_excludes_thin_samples() {
        let records = vec![
            rec("0xlucky", 9_000_000, 1_000_000, 1), // one huge win only
            rec("0xsteady", 100_000, 1_000_000, 1),
            rec("0xsteady", 100_000, 1_000_000, 2),
            rec("0xsteady", 100_000, 1_000_000, 3),
        ];
        let board = leaderboard(&records, 3);
        assert_eq!(board.len(), 1, "one-trade wallet is unranked");
        assert_eq!(board[0].stats.wallet, "0xsteady");
        assert_eq!(board[0].rank, 1);
    }

    #[test]
    fn leaderboard_orders_pnl_then_roi_then_volume() {
        let records = vec![
            // same pnl as 0xb, worse roi -> below
            rec("0xa", 500_000, 10_000_000, 1),
            rec("0xb", 500_000, 1_000_000, 1),
            // best pnl -> first
            rec("0xc", 600_000, 5_000_000, 1),
        ];
        let board = leaderboard(&records, 1);
        let order: Vec<&str> = board.iter().map(|e| e.stats.wallet.as_str()).collect();
        assert_eq!(order, vec!["0xc", "0xb", "0xa"]);
        assert_eq!(board.iter().map(|e| e.rank).collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn ranking_is_a_total_order_with_wallet_tiebreak() {
        // Identical everything -> wallet address decides, deterministically.
        let records = vec![
            rec("0xz", 100_000, 1_000_000, 1),
            rec("0xa", 100_000, 1_000_000, 1),
        ];
        let board = leaderboard(&records, 1);
        assert_eq!(board[0].stats.wallet, "0xa");
        assert_eq!(board[1].stats.wallet, "0xz");
    }

    #[test]
    fn saturating_math_never_wraps() {
        let records = vec![
            rec("0xa", i64::MAX, i64::MAX, 1),
            rec("0xa", i64::MAX, i64::MAX, 2),
        ];
        let stats = compute_stats(&records);
        assert_eq!(stats[0].pnl_micro_usd, i64::MAX, "saturates, never wraps");
    }

    #[test]
    fn negative_stakes_are_clamped_not_counted() {
        // A corrupt feed must not create negative volume (division sign flips).
        let records = vec![rec("0xa", 100_000, -5_000_000, 1)];
        let stats = compute_stats(&records);
        assert_eq!(stats[0].volume_micro_usd, 0);
        assert_eq!(stats[0].roi_bps, 0, "no volume -> no roi");
    }
}
