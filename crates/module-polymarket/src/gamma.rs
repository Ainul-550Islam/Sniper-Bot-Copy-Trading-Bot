//! Gamma API client — Polymarket's market/event metadata catalogue.
//!
//! Gamma returns markets with several fields encoded as *JSON strings*
//! (`outcomes`, `clobTokenIds`, `outcomePrices`), so [`GammaMarket::to_poly`]
//! parses those into the typed [`PolyMarket`] the rest of the bot uses.

use serde::{Deserialize, Serialize};

use bot_core::models::{PolyMarket, PolyOutcome};

use crate::error::{PolyError, PolyResult};

/// Client for the Gamma REST API.
#[derive(Clone)]
pub struct GammaClient {
    base_url: String,
    http: reqwest::Client,
}

/// Query parameters for `GET /markets`.
#[derive(Debug, Clone, Default)]
pub struct MarketQuery {
    /// Filter: only active markets.
    pub active: Option<bool>,
    /// Filter: only closed markets.
    pub closed: Option<bool>,
    /// Page size.
    pub limit: Option<usize>,
    /// Pagination offset.
    pub offset: Option<usize>,
    /// Sort field (e.g. `volume24hr`, `liquidity`).
    pub order: Option<String>,
    /// Sort direction (`true` = ascending).
    pub ascending: Option<bool>,
    /// Filter by Gamma market id.
    pub id: Option<String>,
    /// Filter by market slug (URL name).
    pub slug: Option<String>,
    /// Filter by tag id.
    pub tag_id: Option<String>,
    /// Minimum 24h volume, as a string (Gamma accepts numeric strings).
    pub volume_num_min: Option<f64>,
    /// Minimum liquidity.
    pub liquidity_num_min: Option<f64>,
}

impl MarketQuery {
    /// Encode as URL query pairs (skipping `None`s).
    pub fn to_pairs(&self) -> Vec<(String, String)> {
        let mut p = Vec::new();
        if let Some(v) = self.active {
            p.push(("active".into(), v.to_string()));
        }
        if let Some(v) = self.closed {
            p.push(("closed".into(), v.to_string()));
        }
        if let Some(v) = self.limit {
            p.push(("limit".into(), v.to_string()));
        }
        if let Some(v) = self.offset {
            p.push(("offset".into(), v.to_string()));
        }
        if let Some(v) = &self.order {
            p.push(("order".into(), v.clone()));
        }
        if let Some(v) = self.ascending {
            p.push(("ascending".into(), v.to_string()));
        }
        if let Some(v) = &self.id {
            p.push(("id".into(), v.clone()));
        }
        if let Some(v) = &self.slug {
            p.push(("slug".into(), v.clone()));
        }
        if let Some(v) = &self.tag_id {
            p.push(("tag_id".into(), v.clone()));
        }
        if let Some(v) = self.volume_num_min {
            p.push(("volume_num_min".into(), v.to_string()));
        }
        if let Some(v) = self.liquidity_num_min {
            p.push(("liquidity_num_min".into(), v.to_string()));
        }
        p
    }
}

/// A market as returned by Gamma. Fields are optional because Gamma's schema
/// evolves and different endpoints omit different keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GammaMarket {
    /// On-chain condition id (`0x…` hex).
    #[serde(default)]
    pub condition_id: Option<String>,
    /// Market question text.
    #[serde(default)]
    pub question: Option<String>,
    /// URL slug (`polymarket.com/event/<slug>`).
    #[serde(default)]
    pub slug: Option<String>,
    /// Market uses the neg-risk adapter (multi-outcome) — its orders route
    /// to the NegRiskCtfExchange instead of the plain CTF exchange.
    #[serde(default)]
    pub neg_risk: Option<bool>,
    /// Market is live.
    #[serde(default)]
    pub active: Option<bool>,
    /// Market is closed (resolved/finalized).
    #[serde(default)]
    pub closed: Option<bool>,
    /// CLOB currently accepts orders.
    #[serde(default)]
    pub accepting_orders: Option<bool>,
    /// Scheduled market end (ISO-8601 string).
    #[serde(default)]
    pub end_date: Option<String>,
    /// May be a number or a numeric string.
    #[serde(default)]
    pub volume: Option<serde_json::Value>,
    /// Reported liquidity (number or numeric string, USDC).
    #[serde(default)]
    pub liquidity: Option<serde_json::Value>,
    /// JSON-encoded array string, e.g. `"[\"Yes\",\"No\"]"`.
    #[serde(default)]
    pub outcomes: Option<String>,
    /// JSON-encoded array string of token ids.
    #[serde(default)]
    pub clob_token_ids: Option<String>,
    /// JSON-encoded array string of prices.
    #[serde(default)]
    pub outcome_prices: Option<String>,
}

/// Parse a Gamma "array-as-string" field into a `Vec<String>`.
fn parse_string_array(raw: &Option<String>) -> Vec<String> {
    let Some(s) = raw else { return Vec::new() };
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    match serde_json::from_str::<Vec<String>>(trimmed) {
        Ok(v) => v,
        Err(_) => match serde_json::from_str::<Vec<serde_json::Value>>(trimmed) {
            Ok(vals) => vals
                .into_iter()
                .map(|v| match v {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                })
                .collect(),
            Err(_) => Vec::new(),
        },
    }
}

/// Coerce a Gamma numeric-or-string value to f64.
fn to_f64(v: &Option<serde_json::Value>) -> f64 {
    match v {
        Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(serde_json::Value::String(s)) => s.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    }
}

impl GammaMarket {
    /// Convert to the typed [`PolyMarket`]. Returns `None` when the market has
    /// no condition id or no CLOB token ids (nothing to trade).
    pub fn to_poly(&self) -> Option<PolyMarket> {
        let condition_id = self.condition_id.clone()?;
        let outcomes = parse_string_array(&self.outcomes);
        let token_ids = parse_string_array(&self.clob_token_ids);
        let prices = parse_string_array(&self.outcome_prices);
        if token_ids.is_empty() {
            return None;
        }
        let poly_outcomes: Vec<PolyOutcome> = token_ids
            .iter()
            .enumerate()
            .map(|(i, tid)| PolyOutcome {
                outcome: outcomes
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("Outcome {i}")),
                token_id: tid.clone(),
                price: prices
                    .get(i)
                    .and_then(|p| p.trim().parse::<f64>().ok())
                    .unwrap_or(0.0),
                winner: None,
            })
            .collect();

        Some(PolyMarket {
            condition_id,
            question: self.question.clone().unwrap_or_default(),
            slug: self.slug.clone().unwrap_or_default(),
            neg_risk: self.neg_risk.unwrap_or(false),
            active: self.active.unwrap_or(true),
            closed: self.closed.unwrap_or(false),
            // Gamma omits this on some endpoints; default to "open" so the
            // strategy can still evaluate it (the CLOB is authoritative).
            accepting_orders: self.accepting_orders.unwrap_or(true),
            end_date: self
                .end_date
                .as_ref()
                .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
                .map(|d| d.with_timezone(&chrono::Utc)),
            volume: to_f64(&self.volume),
            liquidity: to_f64(&self.liquidity),
            outcomes: poly_outcomes,
        })
    }
}

impl GammaClient {
    /// Create a client for a Gamma base URL.
    pub fn new(base_url: impl Into<String>) -> PolyResult<Self> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| PolyError::http(format!("gamma http client: {e}")))?;
        Ok(GammaClient {
            base_url: base_url.into(),
            http,
        })
    }

    /// `GET /markets` with the given query, returning typed markets.
    pub async fn markets(&self, query: &MarketQuery) -> PolyResult<Vec<PolyMarket>> {
        let url = format!("{}/markets", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .get(&url)
            .query(&query.to_pairs())
            .send()
            .await?
            .error_for_status()?;
        let raw: Vec<GammaMarket> = resp.json().await?;
        Ok(raw.into_iter().filter_map(|m| m.to_poly()).collect())
    }

    /// `GET /markets?id=...` for a single condition/market id.
    pub async fn market_by_id(&self, id: &str) -> PolyResult<Option<PolyMarket>> {
        let query = MarketQuery {
            id: Some(id.to_string()),
            limit: Some(1),
            ..Default::default()
        };
        let mut markets = self.markets(&query).await?;
        Ok(markets.pop())
    }

    /// `GET /markets?condition_ids=...` — the market for one CLOB condition
    /// id (restart recovery re-hydrates adopted orders with the question /
    /// outcome names this way). `Ok(None)` when Gamma does not know it.
    pub async fn market_by_condition(&self, condition_id: &str) -> PolyResult<Option<PolyMarket>> {
        let url = format!("{}/markets", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .get(&url)
            .query(&[("condition_ids", condition_id), ("limit", "1")])
            .send()
            .await?
            .error_for_status()?;
        let raw: Vec<GammaMarket> = resp.json().await?;
        Ok(raw
            .into_iter()
            .filter_map(|m| m.to_poly())
            .find(|m| m.condition_id.eq_ignore_ascii_case(condition_id)))
    }
}

/// Seconds until `market` resolves at `now` (`None` = no end date known).
/// Negative when the end date has passed.
pub fn seconds_to_resolution(
    market: &PolyMarket,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<i64> {
    market
        .end_date
        .map(|end| end.signed_duration_since(now).num_seconds())
}

/// Human label for an outcome token inside `market` (falls back to the id).
pub fn outcome_label(market: &PolyMarket, token_id: &str) -> String {
    market
        .outcomes
        .iter()
        .find(|o| o.token_id == token_id)
        .map(|o| o.outcome.clone())
        .unwrap_or_else(|| token_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gamma_array_strings() {
        let raw = r#"{
            "conditionId": "0xabc",
            "question": "Will X happen?",
            "slug": "will-x-happen",
            "negRisk": false,
            "active": true,
            "closed": false,
            "acceptingOrders": true,
            "endDate": "2026-12-31T00:00:00Z",
            "volume": "12345.67",
            "liquidity": 890.5,
            "outcomes": "[\"Yes\",\"No\"]",
            "clobTokenIds": "[\"111\",\"222\"]",
            "outcomePrices": "[\"0.42\",\"0.58\"]"
        }"#;
        let gm: GammaMarket = serde_json::from_str(raw).unwrap();
        let pm = gm.to_poly().unwrap();
        assert_eq!(pm.condition_id, "0xabc");
        assert_eq!(pm.outcomes.len(), 2);
        assert_eq!(pm.outcomes[0].outcome, "Yes");
        assert_eq!(pm.outcomes[0].token_id, "111");
        assert!((pm.outcomes[0].price - 0.42).abs() < 1e-9);
        assert_eq!(pm.outcomes[1].token_id, "222");
        assert!((pm.volume - 12345.67).abs() < 1e-6);
        assert!((pm.liquidity - 890.5).abs() < 1e-6);
        assert!(pm.end_date.is_some());
    }

    #[test]
    fn missing_token_ids_yields_none() {
        let raw = r#"{ "conditionId": "0xabc", "question": "Q" }"#;
        let gm: GammaMarket = serde_json::from_str(raw).unwrap();
        assert!(gm.to_poly().is_none());
    }

    #[test]
    fn missing_condition_id_yields_none() {
        let raw = r#"{ "question": "Q", "clobTokenIds": "[\"1\"]" }"#;
        let gm: GammaMarket = serde_json::from_str(raw).unwrap();
        assert!(gm.to_poly().is_none());
    }

    #[test]
    fn outcome_names_default_when_shorter_than_tokens() {
        let raw = r#"{
            "conditionId": "0x1",
            "outcomes": "[\"Yes\"]",
            "clobTokenIds": "[\"10\",\"20\"]",
            "outcomePrices": "[\"0.5\"]"
        }"#;
        let gm: GammaMarket = serde_json::from_str(raw).unwrap();
        let pm = gm.to_poly().unwrap();
        assert_eq!(pm.outcomes[0].outcome, "Yes");
        assert_eq!(pm.outcomes[1].outcome, "Outcome 1");
        assert_eq!(pm.outcomes[1].price, 0.0);
    }

    #[test]
    fn query_pairs_skip_nones() {
        let q = MarketQuery {
            active: Some(true),
            limit: Some(5),
            ..Default::default()
        };
        let pairs = q.to_pairs();
        assert!(pairs.contains(&("active".to_string(), "true".to_string())));
        assert!(pairs.contains(&("limit".to_string(), "5".to_string())));
        assert!(!pairs.iter().any(|(k, _)| k == "closed"));
    }

    #[test]
    fn resolution_and_outcome_helpers() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-21T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let mut m = PolyMarket {
            condition_id: "0xc".into(),
            question: "q".into(),
            slug: "q".into(),
            neg_risk: false,
            active: true,
            closed: false,
            accepting_orders: true,
            end_date: Some(now + chrono::Duration::seconds(90)),
            volume: 0.0,
            liquidity: 0.0,
            outcomes: vec![PolyOutcome {
                outcome: "Yes".into(),
                token_id: "111".into(),
                price: 0.5,
                winner: None,
            }],
        };
        assert_eq!(seconds_to_resolution(&m, now), Some(90));
        assert_eq!(
            seconds_to_resolution(&m, now + chrono::Duration::seconds(100)),
            Some(-10)
        );
        m.end_date = None;
        assert_eq!(seconds_to_resolution(&m, now), None);
        assert_eq!(outcome_label(&m, "111"), "Yes");
        assert_eq!(outcome_label(&m, "222"), "222");
    }
}
