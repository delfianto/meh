//! Pricing database and cost calculation.
//!
//! Calculates per-call and cumulative costs from token usage and model
//! pricing. Includes a known-pricing lookup table for major models and
//! display formatting with threshold-based color hints.

use crate::provider::{ModelInfo, UsageInfo};

/// Calculate cost for a single API call based on token usage and model pricing.
#[allow(clippy::cast_precision_loss)]
pub fn calculate_cost(usage: &UsageInfo, pricing: &ModelInfo) -> f64 {
    let input_cost = usage.input_tokens as f64 * pricing.input_price_per_mtok / 1_000_000.0;
    let output_cost = usage.output_tokens as f64 * pricing.output_price_per_mtok / 1_000_000.0;

    let cache_read_cost = usage
        .cache_read_tokens
        .zip(pricing.cache_read_price_per_mtok)
        .map_or(0.0, |(tokens, price)| tokens as f64 * price / 1_000_000.0);

    let cache_write_cost = usage
        .cache_write_tokens
        .zip(pricing.cache_write_price_per_mtok)
        .map_or(0.0, |(tokens, price)| tokens as f64 * price / 1_000_000.0);

    let thinking_cost = usage.thinking_tokens.map_or(0.0, |tokens| {
        let price = pricing
            .thinking_price_per_mtok
            .unwrap_or(pricing.output_price_per_mtok);
        tokens as f64 * price / 1_000_000.0
    });

    input_cost + output_cost + cache_read_cost + cache_write_cost + thinking_cost
}

/// Format cost for human-readable display.
pub fn format_cost(cost: f64) -> String {
    if cost < 0.01 {
        format!("${cost:.4}")
    } else if cost < 1.0 {
        format!("${cost:.3}")
    } else {
        format!("${cost:.2}")
    }
}

/// Returns the cost threshold category for color display.
pub enum CostLevel {
    /// Less than $0.10.
    Normal,
    /// $0.10 to $1.00.
    Moderate,
    /// Greater than $1.00.
    Expensive,
}

/// Determine cost level for color-coded display.
pub fn cost_level(cost: f64) -> CostLevel {
    if cost > 1.0 {
        CostLevel::Expensive
    } else if cost >= 0.10 {
        CostLevel::Moderate
    } else {
        CostLevel::Normal
    }
}

/// Look up known pricing for a model by ID.
///
/// Returns `(input_price_per_mtok, output_price_per_mtok)` if known.
pub fn get_known_pricing(model_id: &str) -> Option<(f64, f64)> {
    match model_id {
        "claude-opus-4-20250514" => Some((15.0, 75.0)),
        "claude-sonnet-4-20250514" => Some((3.0, 15.0)),
        "claude-haiku-4-5-20251001" => Some((0.80, 4.0)),
        "gpt-4.1" | "o3" => Some((2.0, 8.0)),
        "gpt-4.1-mini" => Some((0.4, 1.6)),
        "gpt-4.1-nano" => Some((0.1, 0.4)),
        "gpt-4o" => Some((2.5, 10.0)),
        "o4-mini" => Some((1.1, 4.4)),
        "gemini-2.5-pro" => Some((1.25, 10.0)),
        "gemini-2.5-flash" => Some((0.15, 0.60)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pricing(input: f64, output: f64) -> ModelInfo {
        ModelInfo {
            id: String::new(),
            name: String::new(),
            provider: String::new(),
            max_tokens: 0,
            context_window: 0,
            supports_tools: false,
            supports_thinking: false,
            supports_images: false,
            input_price_per_mtok: input,
            output_price_per_mtok: output,
            cache_read_price_per_mtok: None,
            cache_write_price_per_mtok: None,
            thinking_price_per_mtok: None,
        }
    }

    #[test]
    fn calculate_cost_basic() {
        let usage = UsageInfo {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            ..Default::default()
        };
        let pricing = make_pricing(3.0, 15.0);
        let cost = calculate_cost(&usage, &pricing);
        assert!((cost - 10.5).abs() < 0.001);
    }

    #[test]
    fn calculate_cost_with_cache() {
        let usage = UsageInfo {
            input_tokens: 100_000,
            output_tokens: 50_000,
            cache_read_tokens: Some(80_000),
            cache_write_tokens: Some(20_000),
            ..Default::default()
        };
        let mut pricing = make_pricing(3.0, 15.0);
        pricing.cache_read_price_per_mtok = Some(0.30);
        pricing.cache_write_price_per_mtok = Some(3.75);
        let cost = calculate_cost(&usage, &pricing);
        let expected = 0.3 + 0.75 + 0.024 + 0.075;
        assert!((cost - expected).abs() < 0.001);
    }

    #[test]
    fn calculate_cost_with_thinking() {
        let usage = UsageInfo {
            input_tokens: 100_000,
            output_tokens: 50_000,
            thinking_tokens: Some(200_000),
            ..Default::default()
        };
        let pricing = make_pricing(3.0, 15.0);
        let cost = calculate_cost(&usage, &pricing);
        let expected = 0.3 + 0.75 + 3.0;
        assert!((cost - expected).abs() < 0.001);
    }

    #[test]
    fn calculate_cost_with_custom_thinking_price() {
        let usage = UsageInfo {
            input_tokens: 0,
            output_tokens: 0,
            thinking_tokens: Some(1_000_000),
            ..Default::default()
        };
        let mut pricing = make_pricing(3.0, 15.0);
        pricing.thinking_price_per_mtok = Some(5.0);
        let cost = calculate_cost(&usage, &pricing);
        assert!((cost - 5.0).abs() < 0.001);
    }

    #[test]
    fn calculate_cost_zero_tokens() {
        let usage = UsageInfo::default();
        let pricing = make_pricing(3.0, 15.0);
        assert!((calculate_cost(&usage, &pricing)).abs() < f64::EPSILON);
    }

    #[test]
    fn format_cost_small() {
        assert_eq!(format_cost(0.001), "$0.0010");
        assert_eq!(format_cost(0.0055), "$0.0055");
    }

    #[test]
    fn format_cost_medium() {
        assert_eq!(format_cost(0.05), "$0.050");
        assert_eq!(format_cost(0.135), "$0.135");
    }

    #[test]
    fn format_cost_large() {
        assert_eq!(format_cost(1.5), "$1.50");
        assert_eq!(format_cost(12.34), "$12.34");
    }

    #[test]
    fn known_pricing_anthropic() {
        let (input, output) = get_known_pricing("claude-sonnet-4-20250514").unwrap();
        assert!((input - 3.0).abs() < f64::EPSILON);
        assert!((output - 15.0).abs() < f64::EPSILON);
    }

    #[test]
    fn known_pricing_openai() {
        assert!(get_known_pricing("gpt-4.1").is_some());
        assert!(get_known_pricing("o3").is_some());
    }

    #[test]
    fn known_pricing_gemini() {
        assert!(get_known_pricing("gemini-2.5-pro").is_some());
        assert!(get_known_pricing("gemini-2.5-flash").is_some());
    }

    #[test]
    fn known_pricing_unknown() {
        assert!(get_known_pricing("unknown-model").is_none());
    }

    #[test]
    fn cost_level_thresholds() {
        assert!(matches!(cost_level(0.05), CostLevel::Normal));
        assert!(matches!(cost_level(0.10), CostLevel::Moderate));
        assert!(matches!(cost_level(0.50), CostLevel::Moderate));
        assert!(matches!(cost_level(1.01), CostLevel::Expensive));
    }
}
