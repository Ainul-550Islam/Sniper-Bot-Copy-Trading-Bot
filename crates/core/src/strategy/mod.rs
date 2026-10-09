//! Strategy domain module (SECOND.md §92).

pub mod model;

pub use model::{
    CopyStrategyParams, PolymarketStrategyParams, SniperStrategyParams, StrategyId, StrategyRecord,
    StrategyStatus,
};

use crate::error::{BotError, BotResult};

/// Validate strategy parameters according to financial risk constraints.
pub fn validate_strategy_params(
    module: crate::models::BotModule,
    config_json: &serde_json::Value,
) -> BotResult<()> {
    match module {
        crate::models::BotModule::Sniper => {
            let params: SniperStrategyParams = serde_json::from_value(config_json.clone())
                .map_err(|e| BotError::invalid(format!("invalid sniper params: {e}")))?;
            if params.entry_amount_lamports == 0 {
                return Err(BotError::invalid("entry amount must be positive"));
            }
            if params.max_slippage_bps > 5000 {
                return Err(BotError::invalid("slippage cannot exceed 5000 bps (50%)"));
            }
        }
        crate::models::BotModule::Copy => {
            let params: CopyStrategyParams = serde_json::from_value(config_json.clone())
                .map_err(|e| BotError::invalid(format!("invalid copy params: {e}")))?;
            if params.allocation_per_trade_lamports == 0 {
                return Err(BotError::invalid("allocation per trade must be positive"));
            }
        }
        crate::models::BotModule::Polymarket => {
            let params: PolymarketStrategyParams = serde_json::from_value(config_json.clone())
                .map_err(|e| BotError::invalid(format!("invalid polymarket params: {e}")))?;
            if params.max_position_size_usdc_units == 0 {
                return Err(BotError::invalid("max position size must be positive"));
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_sniper_bounds() {
        let good = serde_json::to_value(SniperStrategyParams::default()).unwrap();
        assert!(validate_strategy_params(crate::models::BotModule::Sniper, &good).is_ok());

        let mut bad_params = SniperStrategyParams::default();
        bad_params.entry_amount_lamports = 0;
        let bad = serde_json::to_value(bad_params).unwrap();
        assert!(validate_strategy_params(crate::models::BotModule::Sniper, &bad).is_err());
    }
}
