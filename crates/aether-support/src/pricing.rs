//! User-declared model prices and per-turn cost (docs/41 §4).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::TokenUsage;

/// Price currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Currency {
    /// US dollar.
    #[serde(rename = "USD")]
    #[ts(rename = "USD")]
    Usd,
    /// Chinese yuan.
    #[serde(rename = "CNY")]
    #[ts(rename = "CNY")]
    Cny,
}

impl Currency {
    /// Wire and storage code (`USD` / `CNY`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Usd => "USD",
            Self::Cny => "CNY",
        }
    }

    /// Inverse of [`Self::as_str`].
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "USD" => Some(Self::Usd),
            "CNY" => Some(Self::Cny),
            _ => None,
        }
    }
}

/// Prices per million tokens for rounds whose input reaches `min_input_tokens`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct PricingTier {
    /// Lowest round input (cache included) this tier applies to; the first tier is 0.
    #[ts(type = "number")]
    pub min_input_tokens: u64,
    /// Uncached input price.
    pub input: f64,
    /// Output price (reasoning included).
    pub output: f64,
    /// Cache-read price; `None` = `input`.
    pub cache_read: Option<f64>,
    /// Cache-write price; `None` = `input`.
    pub cache_write: Option<f64>,
}

/// Tiered prices of one model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ModelPricing {
    /// Currency of every price.
    pub currency: Currency,
    /// Tiers by strictly increasing `min_input_tokens`, starting at 0.
    pub tiers: Vec<PricingTier>,
}

/// Cost of one turn, fixed when the turn finishes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Cost {
    /// Amount in `currency`.
    pub amount: f64,
    /// Currency of the pricing used.
    pub currency: Currency,
}

impl ModelPricing {
    /// 1–8 tiers from 0, strictly increasing, every price finite in `0..=1e6`.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let price = |p: f64| p.is_finite() && (0.0..=1e6).contains(&p);
        !self.tiers.is_empty()
            && self.tiers.len() <= 8
            && self.tiers[0].min_input_tokens == 0
            && self
                .tiers
                .windows(2)
                .all(|pair| pair[0].min_input_tokens < pair[1].min_input_tokens)
            && self.tiers.iter().all(|t| {
                price(t.input)
                    && price(t.output)
                    && t.cache_read.is_none_or(price)
                    && t.cache_write.is_none_or(price)
            })
    }
}

/// Turn cost summed per upstream round; `None` without pricing, without rounds, or when any
/// round lacks input or output tokens.
#[must_use]
pub fn cost(pricing: Option<&ModelPricing>, rounds: &[TokenUsage]) -> Option<Cost> {
    let pricing = pricing?;
    if rounds.is_empty() {
        return None;
    }
    let mut amount = 0.0;
    for round in rounds {
        let (input, output) = (round.input_tokens?, round.output_tokens?);
        let tier = pricing
            .tiers
            .iter()
            .rfind(|tier| tier.min_input_tokens <= u64::from(input))
            .or(pricing.tiers.first())?;
        let read = round.cache_read_tokens.unwrap_or(0);
        let write = round.cache_write_tokens.unwrap_or(0);
        let uncached = input.saturating_sub(read).saturating_sub(write);
        amount += (f64::from(uncached) * tier.input
            + f64::from(read) * tier.cache_read.unwrap_or(tier.input)
            + f64::from(write) * tier.cache_write.unwrap_or(tier.input)
            + f64::from(output) * tier.output)
            / 1e6;
    }
    Some(Cost {
        amount,
        currency: pricing.currency,
    })
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        clippy::panic,
        reason = "a failed assertion is a failed test"
    )]
    use super::*;

    #[derive(Deserialize)]
    struct Case {
        name: String,
        pricing: ModelPricing,
        rounds: Vec<TokenUsage>,
        cost: Option<f64>,
    }

    /// Same fixture as the web worker test (contract §4).
    #[test]
    fn shared_fixture_costs_match() {
        let text = include_str!("../../../packages/ui/fixtures/pricing-cost.json");
        let cases: Vec<Case> = serde_json::from_str(text).unwrap();
        assert!(cases.len() >= 6);
        for case in cases {
            assert!(case.pricing.is_valid(), "{}", case.name);
            let got = cost(Some(&case.pricing), &case.rounds);
            match (got, case.cost) {
                (None, None) => {}
                (Some(got), Some(want)) => {
                    assert!((got.amount - want).abs() < 1e-9, "{}: {got:?}", case.name);
                    assert_eq!(got.currency, case.pricing.currency, "{}", case.name);
                }
                (got, want) => panic!("{}: {got:?} != {want:?}", case.name),
            }
        }
        assert_eq!(cost(None, &[TokenUsage::default()]), None);
    }
}
