//! Portfolio view (TASK 5 §3): exposure, PnL, fees and capital utilization
//! aggregated over the book, per venue / wallet / strategy / asset / module.
//!
//! Every figure is expressed in the REFERENCE currency configured in
//! `[global_risk]` (`reference_asset`, default `USD`) by multiplying each
//! quote asset's native figure with its operator-configured reference rate
//! (`[global_risk.reference_rates]`, e.g. `SOL = 150.0`, `USDC = 1.0`). The
//! suite fetches no prices for this: a quote asset without a rate is listed
//! in [`PortfolioView::missing_rates`], its positions are still counted in
//! native units ([`PortfolioView::native`]) and the global risk engine
//! refuses new entries on it when a reference-denominated limit is on.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use super::book::PositionBook;
use super::ledger::RealizedSeries;

/// Inputs the view needs beyond the book.
#[derive(Debug, Clone, Default)]
pub struct PortfolioInputs {
    /// Latest mark per base asset (from the modules' operational positions);
    /// assets without a mark use their last fill price.
    pub marks: HashMap<String, f64>,
    /// Reference rate per quote asset (`quote → reference`).
    pub rates: HashMap<String, f64>,
    /// Operator-declared capital in reference units (`0` = unknown).
    pub capital_base_ref: f64,
    /// UTC day (`YYYY-MM-DD`) the daily figures refer to.
    pub day: String,
}

impl PortfolioInputs {
    /// Reference rate for a quote asset, `None` when not configured.
    pub fn rate(&self, quote_asset: &str) -> Option<f64> {
        self.rates
            .get(quote_asset)
            .copied()
            .filter(|r| r.is_finite() && *r > 0.0)
    }
}

/// One aggregated slice (per venue, wallet, strategy, asset or module).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ExposureSlice {
    /// Exposure in reference units (positions on quote assets without a
    /// rate are excluded here and reported under `missing_rates`).
    pub exposure_ref: f64,
    /// Unrealized PnL in reference units.
    pub unrealized_ref: f64,
    /// Gross realized PnL in reference units.
    pub realized_ref: f64,
    /// Fees in reference units.
    pub fees_ref: f64,
    /// Open positions in the slice.
    pub open_positions: usize,
}

impl ExposureSlice {
    /// Realized + unrealized − fees.
    pub fn net_ref(&self) -> f64 {
        self.realized_ref + self.unrealized_ref - self.fees_ref
    }
}

/// Native-unit totals per quote asset (always available, no rate needed).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct NativeSlice {
    /// Exposure in the quote asset.
    pub exposure: f64,
    /// Unrealized PnL in the quote asset.
    pub unrealized: f64,
    /// Gross realized PnL in the quote asset.
    pub realized: f64,
    /// Fees in the quote asset.
    pub fees: f64,
    /// Net realized for the current UTC day in the quote asset.
    pub realized_today: f64,
    /// Open positions.
    pub open_positions: usize,
}

/// The aggregated portfolio.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PortfolioView {
    /// Reference currency label.
    pub reference_asset: String,
    /// Total exposure in reference units.
    pub total_exposure_ref: f64,
    /// Gross realized PnL (all time) in reference units.
    pub realized_ref: f64,
    /// Unrealized PnL in reference units.
    pub unrealized_ref: f64,
    /// Fees (all time) in reference units.
    pub fees_ref: f64,
    /// `realized + unrealized − fees`.
    pub net_pnl_ref: f64,
    /// Net realized for the current UTC day in reference units.
    pub realized_today_ref: f64,
    /// Peak of the cumulative net realized in reference units.
    pub peak_realized_ref: f64,
    /// Cumulative net realized in reference units.
    pub cumulative_realized_ref: f64,
    /// Open positions across the book.
    pub open_positions: usize,
    /// Operator-declared capital base (`0` = unknown).
    pub capital_base_ref: f64,
    /// `total_exposure_ref / capital_base_ref` (`0` when the base is unknown).
    pub utilization: f64,
    /// Per venue.
    pub by_venue: BTreeMap<String, ExposureSlice>,
    /// Per wallet.
    pub by_wallet: BTreeMap<String, ExposureSlice>,
    /// Per strategy.
    pub by_strategy: BTreeMap<String, ExposureSlice>,
    /// Per base asset.
    pub by_asset: BTreeMap<String, ExposureSlice>,
    /// Per module.
    pub by_module: BTreeMap<String, ExposureSlice>,
    /// Per quote asset in native units.
    pub native: BTreeMap<String, NativeSlice>,
    /// Quote assets that carry open exposure but have no reference rate.
    pub missing_rates: Vec<String>,
}

impl PortfolioView {
    /// Compute the view. Pure.
    pub fn compute(book: &PositionBook, series: &RealizedSeries, input: &PortfolioInputs) -> Self {
        let mut view = PortfolioView {
            reference_asset: String::new(),
            capital_base_ref: input.capital_base_ref.max(0.0),
            ..Default::default()
        };
        let mut missing: Vec<String> = Vec::new();
        for p in book.positions() {
            let mark = input.marks.get(&p.key.asset).copied();
            let exposure = p.exposure(mark);
            let unrealized = p.unrealized(mark);
            let native = view.native.entry(p.key.quote_asset.clone()).or_default();
            native.exposure += exposure;
            native.unrealized += unrealized;
            native.realized += p.realized;
            native.fees += p.fees;
            if p.is_open() {
                native.open_positions += 1;
            }
            let Some(rate) = input.rate(&p.key.quote_asset) else {
                if p.is_open() && !missing.contains(&p.key.quote_asset) {
                    missing.push(p.key.quote_asset.clone());
                }
                continue;
            };
            let slice = ExposureSlice {
                exposure_ref: exposure * rate,
                unrealized_ref: unrealized * rate,
                realized_ref: p.realized * rate,
                fees_ref: p.fees * rate,
                open_positions: usize::from(p.is_open()),
            };
            for (map, key) in [
                (&mut view.by_venue, p.key.venue.as_str().to_string()),
                (&mut view.by_wallet, p.key.wallet.clone()),
                (&mut view.by_strategy, p.key.strategy.clone()),
                (&mut view.by_asset, p.key.asset.clone()),
                (&mut view.by_module, p.key.module.as_str().to_string()),
            ] {
                let e = map.entry(key).or_default();
                e.exposure_ref += slice.exposure_ref;
                e.unrealized_ref += slice.unrealized_ref;
                e.realized_ref += slice.realized_ref;
                e.fees_ref += slice.fees_ref;
                e.open_positions += slice.open_positions;
            }
            view.total_exposure_ref += slice.exposure_ref;
            view.realized_ref += slice.realized_ref;
            view.unrealized_ref += slice.unrealized_ref;
            view.fees_ref += slice.fees_ref;
            if p.is_open() {
                view.open_positions += 1;
            }
        }
        for (wallet_quote, fee) in book.standalone_fees() {
            let native = view.native.entry(wallet_quote.1.clone()).or_default();
            native.fees += fee;
            if let Some(rate) = input.rate(&wallet_quote.1) {
                view.fees_ref += fee * rate;
                view.by_wallet
                    .entry(wallet_quote.0.clone())
                    .or_default()
                    .fees_ref += fee * rate;
            }
        }
        for ((day, quote), net) in &series.by_day {
            if day == &input.day {
                view.native.entry(quote.clone()).or_default().realized_today += net;
                if let Some(rate) = input.rate(quote) {
                    view.realized_today_ref += net * rate;
                }
            }
        }
        for (quote, total) in &series.total {
            if let Some(rate) = input.rate(quote) {
                view.cumulative_realized_ref += total * rate;
            }
        }
        for (quote, peak) in &series.peak {
            if let Some(rate) = input.rate(quote) {
                view.peak_realized_ref += peak * rate;
            }
        }
        // Open positions on quote assets without a rate still count.
        view.open_positions = book.open_count();
        view.net_pnl_ref = view.realized_ref + view.unrealized_ref - view.fees_ref;
        view.utilization = if view.capital_base_ref > 0.0 {
            view.total_exposure_ref / view.capital_base_ref
        } else {
            0.0
        };
        missing.sort();
        view.missing_rates = missing;
        view
    }

    /// Current drawdown from the peak cumulative net realized, including
    /// today's unrealized (`>= 0`).
    pub fn drawdown_ref(&self) -> f64 {
        (self.peak_realized_ref - (self.cumulative_realized_ref + self.unrealized_ref)).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounting::event::{fill_event, EventSide};
    use crate::models::{BotModule, ExecutionMode, Venue};
    use chrono::Utc;

    fn inputs() -> PortfolioInputs {
        PortfolioInputs {
            marks: HashMap::new(),
            rates: HashMap::from([("SOL".to_string(), 100.0), ("USDC".to_string(), 1.0)]),
            capital_base_ref: 1_000.0,
            day: Utc::now().format("%Y-%m-%d").to_string(),
        }
    }

    #[test]
    fn slices_and_totals_convert_with_reference_rates() {
        let mut book = PositionBook::new();
        let mut series = RealizedSeries::default();
        let sol = fill_event(
            BotModule::Sniper,
            Venue::PumpFun,
            "w1",
            "sniper",
            "MINT",
            "SOL",
            EventSide::Buy,
            100.0,
            0.01,
            1.0,
            0.0,
            ExecutionMode::Paper,
            "a",
            None,
            None,
            Utc::now(),
            "",
        );
        book.apply(&sol, "a");
        let usdc = fill_event(
            BotModule::Polymarket,
            Venue::PolymarketClob,
            "0xabc",
            "value",
            "TOKEN",
            "USDC",
            EventSide::Buy,
            50.0,
            0.4,
            20.0,
            0.0,
            ExecutionMode::Paper,
            "b",
            None,
            None,
            Utc::now(),
            "",
        );
        book.apply(&usdc, "b");
        series.total.insert("SOL".into(), 0.5);
        series.peak.insert("SOL".into(), 0.8);
        let v = PortfolioView::compute(&book, &series, &inputs());
        // 1 SOL × 100 + 20 USDC × 1 = 120.
        assert!((v.total_exposure_ref - 120.0).abs() < 1e-9);
        assert_eq!(v.open_positions, 2);
        assert!((v.by_venue["pump.fun"].exposure_ref - 100.0).abs() < 1e-9);
        assert!((v.by_venue["polymarket"].exposure_ref - 20.0).abs() < 1e-9);
        assert!((v.by_wallet["w1"].exposure_ref - 100.0).abs() < 1e-9);
        assert!((v.by_strategy["value"].exposure_ref - 20.0).abs() < 1e-9);
        assert!((v.by_asset["MINT"].exposure_ref - 100.0).abs() < 1e-9);
        assert!((v.by_module["polymarket"].exposure_ref - 20.0).abs() < 1e-9);
        assert!((v.utilization - 0.12).abs() < 1e-9);
        assert!((v.cumulative_realized_ref - 50.0).abs() < 1e-9);
        assert!((v.peak_realized_ref - 80.0).abs() < 1e-9);
        assert!((v.drawdown_ref() - 30.0).abs() < 1e-9);
        assert!(v.missing_rates.is_empty());
        // Marks move unrealized and exposure.
        let mut i = inputs();
        i.marks.insert("MINT".into(), 0.03);
        let v = PortfolioView::compute(&book, &series, &i);
        assert!((v.by_asset["MINT"].unrealized_ref - 200.0).abs() < 1e-9);
        assert!((v.by_asset["MINT"].exposure_ref - 300.0).abs() < 1e-9);
        assert!((v.net_pnl_ref - (v.realized_ref + v.unrealized_ref - v.fees_ref)).abs() < 1e-9);
    }

    #[test]
    fn missing_rates_are_reported_not_guessed() {
        let mut book = PositionBook::new();
        let e = fill_event(
            BotModule::Sniper,
            Venue::PumpFun,
            "w1",
            "sniper",
            "MINT",
            "SOL",
            EventSide::Buy,
            100.0,
            0.01,
            1.0,
            0.0,
            ExecutionMode::Paper,
            "a",
            None,
            None,
            Utc::now(),
            "",
        );
        book.apply(&e, "a");
        let mut i = inputs();
        i.rates.clear();
        let v = PortfolioView::compute(&book, &RealizedSeries::default(), &i);
        assert_eq!(v.total_exposure_ref, 0.0);
        assert_eq!(v.missing_rates, vec!["SOL".to_string()]);
        assert!((v.native["SOL"].exposure - 1.0).abs() < 1e-9);
        assert_eq!(v.open_positions, 1);
        assert_eq!(v.utilization, 0.0);
    }
}
